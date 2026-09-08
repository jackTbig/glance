//! 字幕专用的翻译线程。三后端封装已外置到共享 crate `ai-translate`（见 CLAUDE.md「翻译核心已外置」）。
use crate::config::TranslateCfg;
use crate::messages::{SegId, Stage, StageState, UiMsg};
use crossbeam_channel::{Receiver, Sender};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use ai_translate::claude::{parse_delta_line, parse_result_line, user_line, ResultEvent, CLAUDE_FIXED_ARGS};
pub use ai_translate::{Backend, BackendConfig, Engine};

pub const SYSTEM_PROMPT: &str = "你是英译中会议同传。用户消息里可能先给出若干句\"前文\"英中对照，只作参考、不要翻译；你只输出\"请翻译\"后面那句英文的简体中文译文，不加解释、不加引号、不加前缀。专有名词与产品名保留原文。如果注明是还没说完的半句，只译到目前为止的内容，不要补全。";

/// 字幕翻译线程的配置：后端参数（交给共享 crate）+ 字幕专有的会话轮换、积压、临时翻译、上下文句数
#[derive(Debug, Clone)]
pub struct TranslatorConfig {
    pub backend: Backend,
    pub claude_bin: String,
    pub claude_model: String,
    pub codex_bin: String,
    pub codex_model: String,
    pub openai_base_url: String,
    pub openai_api_key: String,
    pub openai_model: String,
    pub timeout: Duration,
    pub rotate_every: u32,
    pub max_backlog: usize,
    /// 测试用：整体替换 claude 命令（如 ["python", "tests/fake_claude.py"]）
    pub command_override: Vec<String>,
    /// 说话过程中每来一条新的中间英文就翻临时中文
    pub partial_translate: bool,
    pub context_sentences: usize,
}

impl TranslatorConfig {
    /// 交给共享 crate 起引擎用的那部分参数
    pub fn backend_cfg(&self) -> BackendConfig {
        BackendConfig {
            backend: self.backend,
            claude_bin: self.claude_bin.clone(),
            claude_model: self.claude_model.clone(),
            codex_bin: self.codex_bin.clone(),
            codex_model: self.codex_model.clone(),
            openai_base_url: self.openai_base_url.clone(),
            openai_api_key: self.openai_api_key.clone(),
            openai_model: self.openai_model.clone(),
            timeout: self.timeout,
            command_override: self.command_override.clone(),
        }
    }
    fn new_engine(&self) -> anyhow::Result<Engine> {
        Engine::new(&self.backend_cfg(), SYSTEM_PROMPT)
    }
}

impl From<&TranslateCfg> for TranslatorConfig {
    fn from(c: &TranslateCfg) -> Self {
        Self {
            backend: Backend::parse(&c.backend),
            claude_bin: c.claude_bin.clone(),
            claude_model: c.claude_model.clone(),
            codex_bin: c.codex_bin.clone(),
            codex_model: c.codex_model.clone(),
            openai_base_url: c.openai_base_url.clone(),
            openai_api_key: c.openai_api_key.clone(),
            openai_model: c.openai_model.clone(),
            timeout: Duration::from_secs(c.timeout_secs.max(1)),
            rotate_every: c.rotate_every.max(1),
            max_backlog: c.max_backlog.max(1),
            command_override: Vec::new(),
            partial_translate: c.partial_translate,
            context_sentences: c.context_sentences,
        }
    }
}

// ---------- 提示词与前文 ----------

pub const PROVISIONAL_NOTE: &str = "下面是还没说完的半句，只译到目前为止的内容，不要补全、不要猜后文。";
pub const CONTINUE_NOTE: &str = "英文有更新。输出更新后的完整译文：已译部分与英文仍一致的原样保留，不一致的改掉；不要重复已译内容，不要另起一遍。";

/// 三个后端共用的用户消息：可选前文对照 + 可选半句说明（含此前已译出的临时中文）+ 请翻译的句子
pub fn build_user_message(context: &[(String, String)], text: &str, provisional: bool, prev_zh: Option<&str>) -> String {
    let mut m = String::new();
    if !context.is_empty() {
        m.push_str("前文（仅供参考，不要翻译）：\n");
        for (en, zh) in context {
            m.push_str("EN: ");
            m.push_str(en);
            m.push_str("\nZH: ");
            m.push_str(zh);
            m.push('\n');
        }
        m.push('\n');
    }
    if provisional {
        m.push_str(PROVISIONAL_NOTE);
        m.push('\n');
        if let Some(prev) = prev_zh.map(str::trim).filter(|p| !p.is_empty()) {
            m.push_str("此前已译出：");
            m.push_str(prev);
            m.push('\n');
            m.push_str(CONTINUE_NOTE);
            m.push('\n');
        }
    }
    m.push_str("请翻译：");
    m.push_str(text);
    m
}

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// 两个翻译线程共享的状态：最近几句英中对照（定稿线程写、两边读）+ 最后一次临时翻译（临时线程写、定稿线程读）
#[derive(Debug, Default)]
pub struct Shared {
    pub recent: VecDeque<(String, String)>,
    /// (话轮 id, 送翻的英文, 译出的中文)
    pub last_provisional: Option<(SegId, String, String)>,
}

pub type Context = Arc<Mutex<Shared>>;

pub fn new_context() -> Context {
    Arc::new(Mutex::new(Shared::default()))
}

pub fn push_context(ctx: &Context, en: &str, zh: &str, max: usize) {
    let mut s = ctx.lock().unwrap();
    s.recent.push_back((en.to_string(), zh.to_string()));
    while s.recent.len() > max {
        s.recent.pop_front();
    }
}

pub fn snapshot_context(ctx: &Context) -> Vec<(String, String)> {
    ctx.lock().unwrap().recent.iter().cloned().collect()
}

pub fn set_last_provisional(ctx: &Context, id: SegId, en: &str, zh: &str) {
    ctx.lock().unwrap().last_provisional = Some((id, en.to_string(), zh.to_string()));
}

/// 两段英文是否同一句话：只比字母数字、忽略大小写与标点（定稿常只差 "everyone" 后面的感叹号）
pub fn same_words(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect::<String>()).filter(|w| !w.is_empty()).collect::<Vec<_>>();
    let (x, y) = (norm(a), norm(b));
    !x.is_empty() && x == y
}

/// 定稿英文与这个话轮最后一次临时翻译所用的英文相同，就直接拿临时译文当定稿（省一次调用、省约 0.7 s、灰白切换零跳动）
pub fn take_provisional_for(ctx: &Context, id: SegId, en: &str) -> Option<String> {
    let mut s = ctx.lock().unwrap();
    match &s.last_provisional {
        Some((pid, pen, zh)) if *pid == id && same_words(pen, en) => {
            let zh = zh.clone();
            s.last_provisional = None;
            Some(zh)
        }
        _ => None,
    }
}

/// 设置窗"测试翻译"：按给定配置起一次引擎，翻一句，返回译文与耗时（含冷启动）
pub fn test_translate(cfg: &TranslatorConfig, text: &str) -> anyhow::Result<(String, Duration)> {
    ai_translate::test_translate(&cfg.backend_cfg(), SYSTEM_PROMPT, text)
}

/// 翻译线程。cfg_rx 收到新配置即热切换后端；claude 会话启动即预热，轮换前 5 句预起备用会话。
pub fn spawn_translator(
    initial: TranslatorConfig,
    cfg_rx: Receiver<TranslatorConfig>,
    rx: Receiver<(SegId, String)>,
    ui: Sender<UiMsg>,
    context: Context,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("translator".into())
        .spawn(move || {
            let mut cfg = initial;
            let report_err = |ui: &Sender<UiMsg>, e: &anyhow::Error| {
                tracing::error!("{e}");
                let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state: StageState::Err(e.to_string()) });
            };
            let mut engine: Option<Engine> = cfg.new_engine().map_err(|e| report_err(&ui, &e)).ok();
            if engine.is_some() {
                let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state: StageState::Ok });
            }
            let mut standby: Option<Engine> = None;
            loop {
                let first = match rx.recv() {
                    Ok(x) => x,
                    Err(_) => return,
                };
                // 热切换：取最新一份配置
                let mut newest = None;
                while let Ok(c) = cfg_rx.try_recv() {
                    newest = Some(c);
                }
                if let Some(c) = newest {
                    tracing::info!("翻译后端切换为 {}", c.backend.as_str());
                    cfg = c;
                    engine = None;
                    standby = None;
                }

                let mut backlog = vec![first];
                while let Ok(x) = rx.try_recv() {
                    backlog.push(x);
                }
                if backlog.len() > cfg.max_backlog {
                    let drop_n = backlog.len() - cfg.max_backlog;
                    for (id, _) in backlog.drain(..drop_n) {
                        let _ = ui.send(UiMsg::Chinese { id, text: "（积压过多，已跳过）".into() });
                    }
                }
                let state = if backlog.len() > 3 {
                    StageState::Warn(format!("翻译积压 {} 句", backlog.len()))
                } else {
                    StageState::Ok
                };
                let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state });

                for (id, en) in backlog {
                    if let Some(zh) = take_provisional_for(&context, id, &en) {
                        tracing::info!("translate[{}] 复用临时译文 {zh:?}", cfg.backend.as_str());
                        push_context(&context, &en, &zh, cfg.context_sentences);
                        let _ = ui.send(UiMsg::Chinese { id, text: zh });
                        continue;
                    }
                    if standby.is_none() {
                        if let Some(e) = &engine {
                            if e.backend() == Backend::Claude && e.turns() + 5 >= cfg.rotate_every {
                                standby = cfg.new_engine().map_err(|e| tracing::warn!("备用会话启动失败：{e}")).ok();
                            }
                        }
                    }
                    if engine.as_ref().map_or(true, |e| e.needs_rotation(cfg.rotate_every)) {
                        engine = match standby.take() {
                            Some(s) => Some(s),
                            None => match cfg.new_engine() {
                                Ok(e) => Some(e),
                                Err(e) => {
                                    report_err(&ui, &e);
                                    let _ = ui.send(UiMsg::Chinese { id, text: "（翻译不可用）".into() });
                                    continue;
                                }
                            },
                        };
                    }
                    let msg = build_user_message(&snapshot_context(&context), &en, false, None);
                    let t0 = Instant::now();
                    let ui2 = ui.clone();
                    let mut on_delta = |acc: &str| {
                        let _ = ui2.send(UiMsg::ChinesePartial { id, text: acc.to_string() });
                    };
                    match engine.as_mut().unwrap().translate(&msg, &mut on_delta) {
                        Ok(zh) => {
                            tracing::info!("translate[{}] {}ms {zh:?}", cfg.backend.as_str(), t0.elapsed().as_millis());
                            push_context(&context, &en, &zh, cfg.context_sentences);
                            let _ = ui.send(UiMsg::Chinese { id, text: zh });
                        }
                        Err(e) => {
                            tracing::warn!("翻译失败，重建引擎：{e}");
                            engine = None;
                            let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state: StageState::Warn(format!("翻译失败：{e}")) });
                            let _ = ui.send(UiMsg::Chinese { id, text: "（翻译超时）".into() });
                        }
                    }
                }
            }
        })
        .expect("spawn translator")
}

/// 临时翻译线程：只收中间英文，保留最新一条；英文一变就翻（同一时刻只有一个在飞，翻完再看最新的），
/// 提示词带上本话轮此前译出的临时中文让模型续写而不是重新措辞。用自己的后端实例（claude = 第二个常驻会话），
/// 每次调用完成后整段发 ChineseProvisional（不流式上屏，避免显示先缩短再长回来）。失败只打 WARN，不进状态红点。
pub fn spawn_partial_translator(
    initial: TranslatorConfig,
    cfg_rx: Receiver<TranslatorConfig>,
    rx: Receiver<(SegId, String)>,
    ui: Sender<UiMsg>,
    context: Context,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("translator-partial".into())
        .spawn(move || {
            let mut cfg = initial;
            let mut engine: Option<Engine> = None;
            let mut cur_id: SegId = 0;
            // 本话轮上一次送翻的英文与译出的临时中文
            let mut last_text = String::new();
            let mut last_zh = String::new();
            loop {
                let (mut id, mut text) = match rx.recv() {
                    Ok(x) => x,
                    Err(_) => return,
                };
                // 只留最新一条
                while let Ok((i, t)) = rx.try_recv() {
                    id = i;
                    text = t;
                }
                let mut newest = None;
                while let Ok(c) = cfg_rx.try_recv() {
                    newest = Some(c);
                }
                if let Some(c) = newest {
                    cfg = c;
                    engine = None;
                }
                if !cfg.partial_translate || cfg.backend == Backend::Codex {
                    continue;
                }
                if id != cur_id {
                    cur_id = id;
                    last_text.clear();
                    last_zh.clear();
                }
                // 英文没变（识别器重复吐同一快照）就不翻
                if word_count(&text) == 0 || text.trim() == last_text.trim() {
                    continue;
                }
                if engine.as_ref().map_or(true, |e| e.needs_rotation(cfg.rotate_every)) {
                    engine = match cfg.new_engine() {
                        Ok(e) => Some(e),
                        Err(e) => {
                            tracing::warn!("临时翻译引擎启动失败：{e}");
                            continue;
                        }
                    };
                }
                let msg = build_user_message(&snapshot_context(&context), &text, true, Some(&last_zh));
                let t0 = Instant::now();
                match engine.as_mut().unwrap().translate(&msg, &mut |_| {}) {
                    Ok(zh) => {
                        tracing::info!("translate partial[{}] {}ms words={} {zh:?}", cfg.backend.as_str(), t0.elapsed().as_millis(), word_count(&text));
                        let _ = ui.send(UiMsg::ChineseProvisional { id, text: zh.clone() });
                        set_last_provisional(&context, id, &text, &zh);
                        last_zh = zh;
                    }
                    Err(e) => {
                        tracing::warn!("临时翻译失败：{e}");
                        engine = None;
                    }
                }
                last_text = text;
            }
        })
        .expect("spawn partial translator")
}

#[cfg(test)]
mod tests {
    use super::*;





    #[test]
    fn user_message_without_context_is_just_the_sentence() {
        let m = build_user_message(&[], "Hello there.", false, None);
        assert_eq!(m, "请翻译：Hello there.");
    }

    #[test]
    fn user_message_with_context_lists_pairs_then_sentence() {
        let ctx = vec![("We start now.".to_string(), "我们现在开始。".to_string()), ("Second.".to_string(), "第二。".to_string())];
        let m = build_user_message(&ctx, "Third.", false, None);
        assert_eq!(
            m,
            "前文（仅供参考，不要翻译）：\nEN: We start now.\nZH: 我们现在开始。\nEN: Second.\nZH: 第二。\n\n请翻译：Third."
        );
    }

    #[test]
    fn provisional_message_carries_half_sentence_note() {
        let m = build_user_message(&[], "So when you have", true, None);
        assert_eq!(m, "下面是还没说完的半句，只译到目前为止的内容，不要补全、不要猜后文。\n请翻译：So when you have");
        let m = build_user_message(&[], "So when you have", true, Some("   "));
        assert!(!m.contains("此前已译出"), "空白的已译文本不算");
    }

    #[test]
    fn provisional_message_asks_to_continue_previous_translation() {
        let m = build_user_message(&[], "So when you have a nested loop", true, Some("所以当你有"));
        assert_eq!(
            m,
            "下面是还没说完的半句，只译到目前为止的内容，不要补全、不要猜后文。\n此前已译出：所以当你有\n英文有更新。输出更新后的完整译文：已译部分与英文仍一致的原样保留，不一致的改掉；不要重复已译内容，不要另起一遍。\n请翻译：So when you have a nested loop"
        );
        let m = build_user_message(&[], "x", false, Some("不该出现"));
        assert!(!m.contains("此前已译出"), "定稿翻译不带续写提示");
    }

    #[test]
    fn word_count_splits_on_whitespace() {
        assert_eq!(word_count("  a  b\tc\n"), 3);
        assert_eq!(word_count(""), 0);
    }

    #[test]
    fn context_queue_keeps_last_n() {
        let ctx = new_context();
        push_context(&ctx, "a", "甲", 2);
        push_context(&ctx, "b", "乙", 2);
        push_context(&ctx, "c", "丙", 2);
        assert_eq!(snapshot_context(&ctx), vec![("b".to_string(), "乙".to_string()), ("c".to_string(), "丙".to_string())]);
        push_context(&ctx, "d", "丁", 0);
        assert!(snapshot_context(&ctx).is_empty(), "上限 0 = 不保留");
    }

    #[test]
    fn same_words_ignores_case_and_punctuation_only() {
        assert!(same_words("Good morning everyone", "Good morning everyone!"));
        assert!(same_words("good morning, everyone.", "Good morning everyone"));
        assert!(!same_words("Good morning everyone", "Good morning everyone today"));
        assert!(!same_words("", ""), "空文本不算相同");
        assert!(!same_words("data center", "datacenter"), "分词不同不算相同");
    }

    #[test]
    fn provisional_is_reused_only_for_same_utterance_and_same_words_once() {
        let ctx = new_context();
        set_last_provisional(&ctx, 7, "good morning everyone", "大家早上好");
        assert_eq!(take_provisional_for(&ctx, 8, "Good morning everyone!"), None, "话轮不同不复用");
        assert_eq!(take_provisional_for(&ctx, 7, "Good morning everyone today"), None, "英文变了不复用");
        assert_eq!(take_provisional_for(&ctx, 7, "Good morning everyone!").as_deref(), Some("大家早上好"));
        assert_eq!(take_provisional_for(&ctx, 7, "Good morning everyone!"), None, "只能用一次");
    }
}
