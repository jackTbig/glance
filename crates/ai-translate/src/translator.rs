//! Translator：按场景路由后端、限并发、批量打包、重试
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
        Ok(Translator { inner: Arc::new(Inner { cfg, sem, openai, serial: Mutex::new(HashMap::new()) }) })
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
                let timeout = self.inner.cfg.base.timeout;
                with_retry(&self.inner.cfg.retry, || {
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
                let r = self.translate(scene, &it.text, None, &mut |_| {});
                results[i] = Some(ItemResult { id: it.id.clone(), result: r });
            }
            return results.into_iter().map(|r| r.unwrap()).collect();
        }
        let bs = self.inner.cfg.batch_size.max(1);
        let chunks: Vec<Vec<usize>> = pending.chunks(bs).map(|c| c.to_vec()).collect();
        let outs: Vec<Vec<(usize, anyhow::Result<String>)>> = std::thread::scope(|s| {
            let hs: Vec<_> = chunks
                .iter()
                .map(|chunk| {
                    let me = self.clone();
                    let items = &items;
                    s.spawn(move || me.translate_chunk(scene, items, chunk))
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

    fn translate_chunk(&self, scene: Scene, items: &[Item], idx: &[usize]) -> Vec<(usize, anyhow::Result<String>)> {
        let route = self.inner.cfg.route(scene);
        let client = self.inner.openai.as_ref().expect("openai client").with_model(&route.model);
        let timeout = self.inner.cfg.base.timeout;
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
        let reply = with_retry(&self.inner.cfg.retry, || {
            let _p = self.inner.sem.acquire();
            client.chat(&sys, &msg, timeout, &mut |_| {})
        });
        let parts: Vec<Option<String>> = match reply {
            Ok(r) => batch::unpack(&r, idx.len()),
            Err(e) => {
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
                    None => self.translate_single_fallback(scene, it),
                };
                (i, r)
            })
            .collect()
    }

    /// 单段兜底：html 段同样走占位符保护
    fn translate_single_fallback(&self, scene: Scene, it: &Item) -> anyhow::Result<String> {
        if !it.is_html {
            return self.translate(scene, &it.text, None, &mut |_| {});
        }
        let p = html_guard::protect(&it.text);
        let t = self.translate(scene, &p.text, None, &mut |_| {})?;
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
