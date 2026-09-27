//! Translator：按场景路由后端、限并发、批量打包、重试
use std::time::Duration;
use crate::backend::{Backend, BackendConfig};
use crate::batch;
use crate::engine::Engine;
use crate::glossary::Glossary;
use crate::html_guard;
use crate::openai::OpenAi;
use crate::prompts;
use crate::retry::{with_retry, RetryPolicy};
use crate::direction::Direction;
use crate::scene::Scene;
use crate::semaphore::Semaphore;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 某场景用哪个后端、哪个模型（model 空 = 用 BackendConfig 里该后端的默认模型）
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SceneRoute {
    pub backend: Backend,
    #[serde(default)]
    pub model: String,
}

impl Default for SceneRoute {
    fn default() -> Self {
        Self { backend: Backend::OpenAi, model: String::new() }
    }
}

#[derive(Debug, Clone)]
pub struct TranslatorConfig {
    /// 三后端的 bin / url / key / 超时（其中 backend 字段被 routes 覆盖，仅作兜底）
    pub base: BackendConfig,
    /// 缺的场景 → OpenAi + 默认模型
    pub routes: HashMap<Scene, SceneRoute>,
    /// 只对 OpenAi 生效
    pub concurrency: usize,
    /// 整页每请求段数
    pub batch_size: usize,
    pub retry: RetryPolicy,
    pub glossary: Glossary,
    /// 翻译方向（默认英译中）。影响提示词、「已经是目标语言」的判据与查词规则
    pub direction: Direction,
}

impl Default for TranslatorConfig {
    fn default() -> Self {
        Self {
            base: BackendConfig { backend: Backend::OpenAi, ..Default::default() },
            routes: HashMap::new(),
            concurrency: 8,
            batch_size: 10,
            retry: RetryPolicy::default(),
            glossary: Glossary::default(),
            direction: Direction::default(),
        }
    }
}

impl TranslatorConfig {
    pub fn route(&self, scene: Scene) -> SceneRoute {
        self.routes.get(&scene).cloned().unwrap_or_default()
    }

    /// 该场景的后端实例参数（后端与模型按路由覆盖）
    pub fn backend_cfg(&self, scene: Scene) -> BackendConfig {
        let r = self.route(scene);
        let mut c = self.base.clone();
        c.backend = r.backend;
        if !r.model.trim().is_empty() {
            match r.backend {
                Backend::Claude => c.claude_model = r.model.trim().into(),
                Backend::Codex => c.codex_model = r.model.trim().into(),
                Backend::OpenAi => c.openai_model = r.model.trim().into(),
            }
        }
        c
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub text: String,
    pub is_html: bool,
}

#[derive(Debug)]
pub struct ItemResult {
    pub id: String,
    pub result: anyhow::Result<String>,
}

/// 汉字数占字母数字汉字总数一半以上
pub fn is_mostly_chinese(text: &str) -> bool {
    let mut han = 0usize;
    let mut total = 0usize;
    for c in text.chars() {
        if c.is_alphanumeric() {
            total += 1;
            if ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c) {
                han += 1;
            }
        }
    }
    total > 0 && han * 2 > total
}

struct Inner {
    cfg: TranslatorConfig,
    sem: Arc<Semaphore>,
    openai: Option<OpenAi>,
    /// claude / codex：每场景一个串行引擎（系统提示按场景固定）
    serial: Mutex<HashMap<Scene, Engine>>,
    /// 反方向的翻译器，主窗口手动输入反向文字时才建（见 [`Translator::for_direction`]）
    flipped: std::sync::OnceLock<Translator>,
}

#[derive(Clone)]
pub struct Translator {
    inner: Arc<Inner>,
}

impl Translator {
    pub fn new(cfg: TranslatorConfig) -> anyhow::Result<Translator> {
        let needs_openai = Scene::ALL.iter().any(|s| cfg.route(*s).backend == Backend::OpenAi);
        let openai = if needs_openai {
            Some(OpenAi::new(&BackendConfig { backend: Backend::OpenAi, ..cfg.base.clone() })?)
        } else {
            None
        };
        let sem = Semaphore::new(cfg.concurrency);
        Ok(Translator {
            inner: Arc::new(Inner {
                cfg,
                sem,
                openai,
                serial: Mutex::new(HashMap::new()),
                flipped: std::sync::OnceLock::new(),
            }),
        })
    }

    /// 同一套配置、指定方向的翻译器。方向相同就是自己；反方向的第一次用时才建，之后复用。
    ///
    /// 挂在自己身上缓存，配置一改（重建翻译器）缓存就跟着作废，不会拿旧配置翻。
    /// claude / codex 后端的常驻子进程也是按需起的，不用反方向就不多占那 300 MB。
    pub fn for_direction(&self, dir: Direction) -> anyhow::Result<Translator> {
        if dir == self.inner.cfg.direction {
            return Ok(self.clone());
        }
        if let Some(t) = self.inner.flipped.get() {
            return Ok(t.clone());
        }
        let mut cfg = self.inner.cfg.clone();
        cfg.direction = dir;
        let t = Translator::new(cfg)?;
        Ok(self.inner.flipped.get_or_init(|| t).clone())
    }

    pub fn config(&self) -> &TranslatorConfig {
        &self.inner.cfg
    }

    fn user_message(&self, scene: Scene, text: &str, context: Option<&str>) -> String {
        let g = self.inner.cfg.glossary.prompt_block(text);
        prompts::user_message(scene, text, context, g.as_deref())
    }

    /// 单条（划词 / 悬停 / 截屏）：流式回调，带重试；claude / codex 失败后重建引擎再试一次
    pub fn translate(&self, scene: Scene, text: &str, context: Option<&str>, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        self.translate_with(scene, text, context, on_delta, &self.inner.cfg.retry)
    }

    /// 同 `translate`，重试策略由调用方定：截屏覆盖要快速失败，
    /// 网络坏时别让用户对着「翻译中」等三轮退避（Jack 2026-09-27 报延迟）
    pub fn translate_with(
        &self,
        scene: Scene,
        text: &str,
        context: Option<&str>,
        on_delta: &mut dyn FnMut(&str),
        retry: &RetryPolicy,
    ) -> anyhow::Result<String> {
        self.translate_tuned(scene, text, context, on_delta, retry, None)
    }

    /// `timeout` 覆盖配置里的单请求超时：截屏覆盖要短超时快速失败，
    /// 坏网络下别按 30 秒一轮地等（Jack 2026-09-27 实测一次故障等了 175 秒）
    pub fn translate_tuned(
        &self,
        scene: Scene,
        text: &str,
        context: Option<&str>,
        on_delta: &mut dyn FnMut(&str),
        retry: &RetryPolicy,
        timeout: Option<Duration>,
    ) -> anyhow::Result<String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(String::new());
        }
        let route = self.inner.cfg.route(scene);
        let sys = prompts::system_prompt(scene, None, self.inner.cfg.direction);
        let msg = self.user_message(scene, text, context);
        match route.backend {
            Backend::OpenAi => {
                let client = self.inner.openai.as_ref().expect("openai client").with_model(&route.model);
                let timeout = timeout.unwrap_or(self.inner.cfg.base.timeout);
                with_retry(retry, || {
                    let _p = self.inner.sem.acquire();
                    client.chat(&sys, &msg, timeout, on_delta)
                })
            }
            _ => self.translate_serial(scene, &sys, &msg, on_delta),
        }
    }

    fn translate_serial(&self, scene: Scene, sys: &str, msg: &str, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        let mut map = self.inner.serial.lock().unwrap();
        let bcfg = self.inner.cfg.backend_cfg(scene);
        let mut last_err = None;
        for attempt in 0..2 {
            if !map.contains_key(&scene) {
                match Engine::new(&bcfg, sys) {
                    Ok(e) => {
                        map.insert(scene, e);
                    }
                    Err(e) => {
                        last_err = Some(e);
                        continue;
                    }
                }
            }
            let engine = map.get_mut(&scene).unwrap();
            match engine.translate(msg, on_delta) {
                Ok(s) => return Ok(s),
                Err(e) => {
                    tracing::warn!("{} 后端第 {} 次失败，重建引擎：{e}", bcfg.backend.as_str(), attempt + 1);
                    map.remove(&scene);
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("翻译失败")))
    }

    /// 批量（整页）：纯中文段本地直接返回；其余按 batch_size 分块并发；缺段 / 还原失败的段再走单条兜底
    pub fn translate_batch(&self, scene: Scene, items: Vec<Item>) -> Vec<ItemResult> {
        let bs = self.inner.cfg.batch_size.max(1);
        self.translate_batch_tuned(scene, items, bs, &self.inner.cfg.retry)
    }

    /// 同 `translate_batch`，分块大小与重试策略由调用方定：
    /// 块越小首行上屏越快（各块并发），重试越少坏网络下等得越短
    pub fn translate_batch_tuned(
        &self,
        scene: Scene,
        items: Vec<Item>,
        chunk: usize,
        retry: &RetryPolicy,
    ) -> Vec<ItemResult> {
        self.translate_batch_full(scene, items, chunk, retry, None, true)
    }

    /// 全参数批量：`timeout` 覆盖单请求超时；`fallback_on_chunk_error` 控制整包**请求失败**时
    /// 要不要逐段串行兜底。整页翻译要兜底（模型排版乱了靠它救）；截屏覆盖不要——
    /// 网络坏时逐段兜底会把 N 行变成 N 次串行长超时（Jack 2026-09-27 的 175 秒就是这么来的）
    pub fn translate_batch_full(
        &self,
        scene: Scene,
        items: Vec<Item>,
        chunk: usize,
        retry: &RetryPolicy,
        timeout: Option<Duration>,
        fallback_on_chunk_error: bool,
    ) -> Vec<ItemResult> {
        let n = items.len();
        let mut results: Vec<Option<ItemResult>> = (0..n).map(|_| None).collect();
        let mut pending: Vec<usize> = Vec::new();
        for (i, it) in items.iter().enumerate() {
            if it.text.trim().is_empty() || self.inner.cfg.direction.already_target(&it.text) {
                results[i] = Some(ItemResult { id: it.id.clone(), result: Ok(it.text.clone()) });
            } else {
                pending.push(i);
            }
        }
        let route = self.inner.cfg.route(scene);
        if route.backend != Backend::OpenAi {
            // 常驻子进程后端：逐条串行
            for i in pending {
                let it = &items[i];
                let r = self.translate_tuned(scene, &it.text, None, &mut |_| {}, retry, timeout);
                results[i] = Some(ItemResult { id: it.id.clone(), result: r });
            }
            return results.into_iter().map(|r| r.unwrap()).collect();
        }
        let bs = chunk.max(1);
        let chunks: Vec<Vec<usize>> = pending.chunks(bs).map(|c| c.to_vec()).collect();
        let outs: Vec<Vec<(usize, anyhow::Result<String>)>> = std::thread::scope(|s| {
            let hs: Vec<_> = chunks
                .iter()
                .map(|chunk| {
                    let me = self.clone();
                    let items = &items;
                    s.spawn(move || {
                        me.translate_chunk(scene, items, chunk, retry, timeout, fallback_on_chunk_error)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().expect("chunk thread")).collect()
        });
        for out in outs {
            for (i, r) in out {
                results[i] = Some(ItemResult { id: items[i].id.clone(), result: r });
            }
        }
        results.into_iter().map(|r| r.unwrap()).collect()
    }

    fn translate_chunk(
        &self,
        scene: Scene,
        items: &[Item],
        idx: &[usize],
        retry: &RetryPolicy,
        timeout: Option<Duration>,
        fallback_on_chunk_error: bool,
    ) -> Vec<(usize, anyhow::Result<String>)> {
        let route = self.inner.cfg.route(scene);
        let client = self.inner.openai.as_ref().expect("openai client").with_model(&route.model);
        let timeout = timeout.unwrap_or(self.inner.cfg.base.timeout);
        let protected: Vec<html_guard::Protected> = idx
            .iter()
            .map(|&i| {
                let it = &items[i];
                if it.is_html {
                    html_guard::protect(&it.text)
                } else {
                    html_guard::Protected { text: it.text.clone(), tags: Vec::new() }
                }
            })
            .collect();
        let packed = batch::pack(&protected.iter().map(|p| p.text.as_str()).collect::<Vec<_>>());
        let sys = prompts::system_prompt(scene, None, self.inner.cfg.direction);
        let msg = self.user_message(scene, &packed, None);
        let reply = with_retry(retry, || {
            let _p = self.inner.sem.acquire();
            client.chat(&sys, &msg, timeout, &mut |_| {})
        });
        let parts: Vec<Option<String>> = match reply {
            Ok(r) => batch::unpack(&r, idx.len()),
            Err(e) => {
                if !fallback_on_chunk_error {
                    tracing::warn!("整包请求失败，不兜底直接报错：{e}");
                    return idx.iter().map(|&i| (i, Err(anyhow::anyhow!("翻译失败：{e}")))).collect();
                }
                tracing::warn!("整包失败，逐段兜底：{e}");
                vec![None; idx.len()]
            }
        };
        idx.iter()
            .enumerate()
            .map(|(k, &i)| {
                let it = &items[i];
                let restored = parts[k].as_deref().and_then(|t| {
                    if it.is_html {
                        html_guard::restore(t, &protected[k].tags).ok()
                    } else {
                        Some(t.to_string())
                    }
                });
                let r = match restored {
                    Some(s) => Ok(s),
                    None => self.translate_single_fallback(scene, it, retry, timeout),
                };
                (i, r)
            })
            .collect()
    }

    /// 单段兜底：html 段同样走占位符保护
    fn translate_single_fallback(
        &self,
        scene: Scene,
        it: &Item,
        retry: &RetryPolicy,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        if !it.is_html {
            return self.translate_tuned(scene, &it.text, None, &mut |_| {}, retry, Some(timeout));
        }
        let p = html_guard::protect(&it.text);
        let t = self.translate_tuned(scene, &p.text, None, &mut |_| {}, retry, Some(timeout))?;
        html_guard::restore(&t, &p.tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_detection() {
        assert!(is_mostly_chinese("这是一段中文，含 API 两个字母"));
        assert!(!is_mostly_chinese("Mostly English with 一个 word"));
        assert!(!is_mostly_chinese("   "));
    }

    #[test]
    fn route_overrides_model() {
        let mut cfg = TranslatorConfig::default();
        cfg.routes.insert(Scene::Hover, SceneRoute { backend: Backend::Claude, model: "haiku".into() });
        assert_eq!(cfg.backend_cfg(Scene::Hover).claude_model, "haiku");
        assert_eq!(cfg.backend_cfg(Scene::Selection).backend, Backend::OpenAi);
    }
}
