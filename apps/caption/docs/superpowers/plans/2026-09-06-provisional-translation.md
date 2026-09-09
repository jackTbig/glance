# 临时翻译与带前文翻译 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 说话过程中就把中间英文翻成灰色临时中文（落后英文约 1.5–2 s），英文定稿后整句重翻定稿；每次翻译都带最近 N 句英中对照当上下文。

**Architecture:** 识别线程给每个话轮在第一个中间结果时分配 id，中间结果同时送界面与新的"临时翻译"线程；临时线程只留最新一条，按"≥1.2 s 且 ≥3 个新词"发翻译，用自己的后端实例，结果发 `ChineseProvisional`。定稿线程译完把 (en, zh) 推进共享前文队列，两线程组提示词时都带前文。界面对临时中文灰色斜体，定稿流开始后忽略晚到的临时结果，中文替换按公共前缀保留已显示部分。

**Tech Stack:** Rust（crossbeam-channel、egui 0.36、reqwest blocking、serde）；测试用 `tests/fake_claude.py` / `tests/fake_openai.py` 冒充后端。规格：`docs/superpowers/specs/2026-09-06-provisional-translation-design.md`。

编译前必须 `source scripts/env.sh`；跑测试：`PYTHONUTF8=1 cargo test --release`。编译期间安装版可以在跑（exe 不同文件），但 `cargo run` 不行。

---

## 文件结构

- `src/messages.rs` —— `EnglishPartial` 加 `id`；新增 `ChineseProvisional { id, text }`。
- `src/config.rs` —— `TranslateCfg` 新增 `partial_every_ms / partial_min_new_words / context_sentences`。
- `src/translate.rs` —— 系统提示词；`build_user_message`；`should_send` / `word_count`；共享前文 `Context`；`TranslatorConfig` 三个新字段；`spawn_translator` 带前文并推队列；新增 `spawn_partial_translator`。
- `src/asr.rs` —— `IdAlloc`（话轮 id 分配）；中间结果送临时翻译通道。
- `src/ui.rs` —— `Line` 两个新标记；`common_prefix_chars`；消息处理；灰色斜体；设置页两项；配置通道变为多个。
- `src/main.rs` —— 接线。
- `tests/fake_claude.py` / `tests/fake_openai.py` —— 回显时剥掉提示词，附带 `ctx=<前文句数> prov=<0/1>`。
- `tests/translator.rs` —— 适配新签名；新增临时翻译与前文集成测试。
- `config.example.toml`、`CLAUDE.md` —— 文档。

---

### Task 1: 消息与配置项

**Files:**
- Modify: `src/messages.rs`
- Modify: `src/config.rs`（`TranslateCfg` 与其 `Default`，测试模块）
- Modify: `config.example.toml`

- [ ] **Step 1: 写失败测试（config.rs 测试模块末尾追加）**

```rust
    #[test]
    fn translate_partial_defaults() {
        let cfg = Config::default();
        assert_eq!(cfg.translate.partial_every_ms, 1200);
        assert_eq!(cfg.translate.partial_min_new_words, 3);
        assert_eq!(cfg.translate.context_sentences, 2);
        let old: Config = toml::from_str("[translate]\nbackend = \"openai\"\n").unwrap();
        assert_eq!(old.translate.partial_every_ms, 1200, "旧配置文件没有这几项也要有默认值");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib config::tests::translate_partial_defaults`
Expected: 编译错误 `no field partial_every_ms`。

- [ ] **Step 3: 实现**

`src/messages.rs`：

```rust
#[derive(Debug, Clone)]
pub enum UiMsg {
    /// 正在说的这句的中间识别结果（会被后续 Partial 或 English 替换）；id 与之后的定稿相同
    EnglishPartial { id: SegId, text: String },
    English { id: SegId, at: DateTime<Local>, text: String },
    /// 说话过程中按半句翻出来的临时中文（灰色），定稿译文开始后不再采纳
    ChineseProvisional { id: SegId, text: String },
    /// 流式译文（累积到目前为止的全文）
    ChinesePartial { id: SegId, text: String },
    Chinese { id: SegId, text: String },
    Status { stage: Stage, state: StageState },
}
```

`src/config.rs` `TranslateCfg` 末尾加三个字段，`Default` 里给值：

```rust
    pub max_backlog: usize,
    /// 说话过程中每隔多少毫秒把半句送去翻一次临时中文（0 = 关闭）
    pub partial_every_ms: u64,
    /// 半句比上次送翻时至少多这么多个词才再送
    pub partial_min_new_words: usize,
    /// 翻译时带最近几句英中对照当上下文（0 = 不带）
    pub context_sentences: usize,
```

```rust
            max_backlog: 10,
            partial_every_ms: 1200,
            partial_min_new_words: 3,
            context_sentences: 2,
```

`config.example.toml` `[translate]` 段 `max_backlog = 10` 后加：

```toml
partial_every_ms = 1200   # 说话过程中每 1.2 s 把半句翻成灰色临时中文（0 关闭）；定稿后整句重翻
partial_min_new_words = 3 # 半句至少多 3 个词才再送翻
context_sentences = 2     # 翻译时带最近 2 句英中对照当上下文
```

- [ ] **Step 4: 跑测试**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib config::tests`
Expected: 全部 PASS。此时 `ui.rs` / `asr.rs` 因 `EnglishPartial` 缺 `id` 会编译失败：先在 `src/asr.rs` 两处 `EnglishPartial { text }` 临时改为 `EnglishPartial { id: 0, text }`，`src/ui.rs` 的 `UiMsg::EnglishPartial { text }` 改为 `UiMsg::EnglishPartial { id: _, text }`，并在 `drain` 的 match 里加一条 `UiMsg::ChineseProvisional { .. } => {}`，Task 5 / Task 6 再改成正式逻辑。

- [ ] **Step 5: 提交**

```bash
git add src/messages.rs src/config.rs src/asr.rs src/ui.rs config.example.toml
git commit -m "feat: 临时翻译消息类型与三个翻译配置项"
```

---

### Task 2: 提示词、发送条件、前文队列（translate.rs 纯函数）

**Files:**
- Modify: `src/translate.rs`（顶部常量、`TranslatorConfig`、新增纯函数、测试模块）

- [ ] **Step 1: 写失败测试（translate.rs 测试模块末尾追加）**

```rust
    #[test]
    fn user_message_without_context_is_just_the_sentence() {
        let m = build_user_message(&[], "Hello there.", false);
        assert_eq!(m, "请翻译：Hello there.");
    }

    #[test]
    fn user_message_with_context_lists_pairs_then_sentence() {
        let ctx = vec![("We start now.".to_string(), "我们现在开始。".to_string()), ("Second.".to_string(), "第二。".to_string())];
        let m = build_user_message(&ctx, "Third.", false);
        assert_eq!(
            m,
            "前文（仅供参考，不要翻译）：\nEN: We start now.\nZH: 我们现在开始。\nEN: Second.\nZH: 第二。\n\n请翻译：Third."
        );
    }

    #[test]
    fn provisional_message_carries_half_sentence_note() {
        let m = build_user_message(&[], "So when you have", true);
        assert_eq!(m, "下面是还没说完的半句，只译到目前为止的内容，不要补全、不要猜后文。\n请翻译：So when you have");
    }

    #[test]
    fn should_send_rules() {
        let every = Duration::from_millis(1200);
        let t0 = Instant::now();
        assert!(!should_send(t0, None, 0, 2, every, 3), "首次不足 3 词不发");
        assert!(should_send(t0, None, 0, 3, every, 3), "首次满 3 词就发");
        let sent = Some(t0);
        assert!(!should_send(t0 + Duration::from_millis(500), sent, 3, 9, every, 3), "间隔不足不发");
        assert!(!should_send(t0 + Duration::from_millis(1300), sent, 3, 5, every, 3), "新词不足不发");
        assert!(should_send(t0 + Duration::from_millis(1300), sent, 3, 6, every, 3), "间隔够且多 3 词才发");
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib translate::tests`
Expected: 编译错误，`build_user_message` 等未定义。

- [ ] **Step 3: 实现**

顶部 `use` 加：

```rust
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
```

系统提示词改为：

```rust
pub const SYSTEM_PROMPT: &str = "你是英译中会议同传。用户消息里可能先给出若干句\"前文\"英中对照，只作参考、不要翻译；你只输出\"请翻译\"后面那句英文的简体中文译文，不加解释、不加引号、不加前缀。专有名词与产品名保留原文。如果注明是还没说完的半句，只译到目前为止的内容，不要补全。";
```

`TranslatorConfig` 加字段并在 `From<&TranslateCfg>` 里赋值：

```rust
    pub command_override: Vec<String>,
    /// 临时翻译最短间隔；0 = 关闭临时翻译
    pub partial_every: Duration,
    pub partial_min_new_words: usize,
    pub context_sentences: usize,
```

```rust
            command_override: Vec::new(),
            partial_every: Duration::from_millis(c.partial_every_ms),
            partial_min_new_words: c.partial_min_new_words.max(1),
            context_sentences: c.context_sentences,
```

在 `resolve_bin` 前加纯函数与前文队列：

```rust
// ---------- 提示词与前文 ----------

pub const PROVISIONAL_NOTE: &str = "下面是还没说完的半句，只译到目前为止的内容，不要补全、不要猜后文。";

/// 三个后端共用的用户消息：可选前文对照 + 可选半句说明 + 请翻译的句子
pub fn build_user_message(context: &[(String, String)], text: &str, provisional: bool) -> String {
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
    }
    m.push_str("请翻译：");
    m.push_str(text);
    m
}

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

/// 临时翻译发送条件：首次要够 min_new 个词；之后距上次发出 ≥ every 且比上次多 ≥ min_new 个词
pub fn should_send(now: Instant, last_sent_at: Option<Instant>, last_sent_words: usize, words: usize, every: Duration, min_new: usize) -> bool {
    match last_sent_at {
        None => words >= min_new,
        Some(t) => now.duration_since(t) >= every && words >= last_sent_words + min_new,
    }
}

/// 最近几句英中对照，定稿线程写、两个翻译线程读
pub type Context = Arc<Mutex<VecDeque<(String, String)>>>;

pub fn new_context() -> Context {
    Arc::new(Mutex::new(VecDeque::new()))
}

pub fn push_context(ctx: &Context, en: &str, zh: &str, max: usize) {
    let mut q = ctx.lock().unwrap();
    q.push_back((en.to_string(), zh.to_string()));
    while q.len() > max {
        q.pop_front();
    }
}

pub fn snapshot_context(ctx: &Context) -> Vec<(String, String)> {
    ctx.lock().unwrap().iter().cloned().collect()
}
```

- [ ] **Step 4: 跑测试**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib translate::tests`
Expected: 全部 PASS。（`tests/translator.rs` 里的 `base_cfg()` 结构体字面量此时缺字段会编译失败，但 `--lib` 不编它；Task 4 修。）

- [ ] **Step 5: 提交**

```bash
git add src/translate.rs
git commit -m "feat: 翻译提示词带前文与半句说明；临时翻译发送条件；共享前文队列"
```

---

### Task 3: 假后端剥提示词并回显前文/半句标记

**Files:**
- Modify: `tests/fake_claude.py`
- Modify: `tests/fake_openai.py`

- [ ] **Step 1: 改 fake_claude.py**

把 `text = msg["message"]["content"]`、`if "SLEEP" in text: time.sleep(20)` 与 `full = ...` 替换为：

```python
    content = msg["message"]["content"]
    # 用户消息形如 "前文…\n\n[半句说明\n]请翻译：<句子>"：只回显句子，并把前文句数与半句标记带在前面
    text = content.split("请翻译：", 1)[-1].strip()
    n_ctx = content.count("\nEN: ") + (1 if content.startswith("EN: ") else 0)
    prov = 1 if "还没说完的半句" in content else 0
    if "SLEEP" in text:
        time.sleep(20)
    full = f"pid={os.getpid()} ctx={n_ctx} prov={prov} zh={text}"
```

- [ ] **Step 2: 改 fake_openai.py**

把 `text = body["messages"][-1]["content"]` 与 `full = ...` 两行替换为：

```python
        content = body["messages"][-1]["content"]
        text = content.split("请翻译：", 1)[-1].strip()
        n_ctx = content.count("\nEN: ") + (1 if content.startswith("EN: ") else 0)
        prov = 1 if "还没说完的半句" in content else 0
        full = f"pid={os.getpid()} ctx={n_ctx} prov={prov} zh={text}"
```

- [ ] **Step 3: 手工验证 fake_openai**

起 `python tests/fake_openai.py 18999`，POST 一条 content 为 `前文（仅供参考，不要翻译）：\nEN: a\nZH: 甲\n\n请翻译：hello` 的消息。
Expected: SSE 增量拼起来是 `pid=<n> ctx=1 prov=0 zh=hello`。

- [ ] **Step 4: 提交**

```bash
git add tests/fake_claude.py tests/fake_openai.py
git commit -m "test: 假后端剥掉提示词，回显前文句数与半句标记"
```

---

### Task 4: 定稿线程带前文、新增临时翻译线程、集成测试

**Files:**
- Modify: `src/translate.rs`（`Engine::translate` 调用处、`spawn_translator` 签名与循环、新增 `spawn_partial_translator`）
- Modify: `tests/translator.rs`

- [ ] **Step 1: 改 tests/translator.rs 的 base_cfg 并加失败测试**

`base_cfg()` 字面量末尾加：

```rust
        partial_every: Duration::from_millis(300),
        partial_min_new_words: 3,
        context_sentences: 2,
```

所有 `spawn_translator(cfg, cfg_rx, rx, ui_tx)` 调用改为 `spawn_translator(cfg, cfg_rx, rx, ui_tx, new_context())`，`use` 行改为：

```rust
use live_caption::translate::{new_context, push_context, spawn_partial_translator, spawn_translator, Backend, TranslatorConfig};
```

文件末尾追加：

```rust
fn next_provisional(rx: &crossbeam_channel::Receiver<UiMsg>) -> (u64, String) {
    loop {
        match rx.recv_timeout(Duration::from_secs(15)).expect("等临时译文超时") {
            UiMsg::ChineseProvisional { id, text } => return (id, text),
            _ => continue,
        }
    }
}

/// 收临时译文直到出现以 want 结尾的一条（流式增量前面会有若干截断的）
fn provisional_ending_with(rx: &crossbeam_channel::Receiver<UiMsg>, want: &str) -> (u64, String) {
    let t0 = Instant::now();
    loop {
        let (id, t) = next_provisional(rx);
        if t.ends_with(want) {
            return (id, t);
        }
        assert!(t0.elapsed() < Duration::from_secs(15), "没等到 {want:?}，最后一条 {t:?}");
    }
}

#[test]
fn final_translation_carries_previous_sentences_as_context() {
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let ctx = new_context();
    let _h = spawn_translator(base_cfg(), cfg_rx, rx, ui_tx, ctx.clone());
    tx.send((1, "one".into())).unwrap();
    let (_, t1) = next_chinese(&ui_rx);
    assert!(t1.contains(" ctx=0 "), "第一句没有前文：{t1}");
    tx.send((2, "two".into())).unwrap();
    let (_, t2) = next_chinese(&ui_rx);
    assert!(t2.contains(" ctx=1 ") && t2.contains("prov=0"), "第二句应带 1 句前文：{t2}");
    tx.send((3, "three".into())).unwrap();
    let (_, t3) = next_chinese(&ui_rx);
    assert!(t3.contains(" ctx=2 "), "第三句带 2 句前文：{t3}");
    tx.send((4, "four".into())).unwrap();
    let (_, t4) = next_chinese(&ui_rx);
    assert!(t4.contains(" ctx=2 "), "上限 2 句：{t4}");
    assert!(t4.ends_with("zh=four"));
}

#[test]
fn provisional_thread_translates_half_sentence_with_note_and_context() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let ctx = new_context();
    push_context(&ctx, "Earlier sentence.", "前一句。", 2);
    let _h = spawn_partial_translator(base_cfg(), cfg_rx, rx, ui_tx, ctx);
    tx.send((7, "so when".into())).unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert!(ui_rx.try_recv().is_err(), "不足 3 词不该翻");
    tx.send((7, "so when you have".into())).unwrap();
    let (id, t) = provisional_ending_with(&ui_rx, "zh=so when you have");
    assert_eq!(id, 7);
    assert!(t.contains(" ctx=1 ") && t.contains(" prov=1 "), "应带前文与半句标记：{t}");
}

#[test]
fn provisional_thread_rate_limits_and_takes_newest() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let mut cfg = base_cfg();
    cfg.partial_every = Duration::from_millis(2000);
    let _h = spawn_partial_translator(cfg, cfg_rx, rx, ui_tx, new_context());
    tx.send((1, "a b c".into())).unwrap();
    let _ = provisional_ending_with(&ui_rx, "zh=a b c");
    // 间隔内连发两条：都不该翻；间隔过后只翻最新一条
    tx.send((1, "a b c d e f".into())).unwrap();
    tx.send((1, "a b c d e f g h i".into())).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert!(ui_rx.try_recv().is_err(), "2 s 内不该再翻");
    std::thread::sleep(Duration::from_millis(1500));
    tx.send((1, "a b c d e f g h i j".into())).unwrap();
    let (_, t) = provisional_ending_with(&ui_rx, "zh=a b c d e f g h i j");
    assert!(!t.contains("zh=a b c d e f g h i "), "旧快照作废：{t}");
}

#[test]
fn provisional_thread_resets_counter_on_new_utterance() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_partial_translator(base_cfg(), cfg_rx, rx, ui_tx, new_context());
    tx.send((1, "one two three four five six".into())).unwrap();
    let _ = provisional_ending_with(&ui_rx, "zh=one two three four five six");
    std::thread::sleep(Duration::from_millis(400));
    // 新话轮只有 3 个词：按"首次"规则应立即翻，而不是要求比上一话轮多 3 词
    tx.send((2, "new one here".into())).unwrap();
    let (id, _) = provisional_ending_with(&ui_rx, "zh=new one here");
    assert_eq!(id, 2);
}

#[test]
fn provisional_thread_is_idle_when_disabled_or_codex() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (cfg_tx, cfg_rx) = bounded(4);
    let mut cfg = base_cfg();
    cfg.partial_every = Duration::ZERO;
    let _h = spawn_partial_translator(cfg, cfg_rx, rx, ui_tx, new_context());
    tx.send((1, "a b c d".into())).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(ui_rx.try_recv().is_err(), "关闭时不翻");
    let mut on = base_cfg();
    on.backend = Backend::Codex;
    cfg_tx.send(on).unwrap();
    tx.send((1, "a b c d e f g".into())).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(ui_rx.try_recv().is_err(), "codex 后端不翻");
}
```

- [ ] **Step 2: 跑确认失败**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --test translator`
Expected: 编译错误，`spawn_partial_translator` 未定义 / `spawn_translator` 参数数量不对。

- [ ] **Step 3: 实现 spawn_translator 改动**

签名加 `context: Context`：

```rust
pub fn spawn_translator(
    initial: TranslatorConfig,
    cfg_rx: Receiver<TranslatorConfig>,
    rx: Receiver<(SegId, String)>,
    ui: Sender<UiMsg>,
    context: Context,
) -> std::thread::JoinHandle<()> {
```

循环里 `let t0 = Instant::now();` 之前加：

```rust
                    let msg = build_user_message(&snapshot_context(&context), &en, false);
```

`translate(&cfg, &en, &mut on_delta)` 改为 `translate(&cfg, &msg, &mut on_delta)`；`Ok(zh)` 分支在 `ui.send(UiMsg::Chinese …)` 之前加：

```rust
                            push_context(&context, &en, &zh, cfg.context_sentences);
```

- [ ] **Step 4: 实现 spawn_partial_translator（放在 spawn_translator 之后）**

```rust
/// 临时翻译线程：只收中间英文，保留最新一条，按 `partial_every` / `partial_min_new_words` 规则送翻，
/// 用自己的后端实例（claude = 第二个常驻会话），结果发 ChineseProvisional。失败只打 WARN，不进状态红点。
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
            let mut last_sent_at: Option<Instant> = None;
            let mut last_words = 0usize;
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
                if cfg.partial_every.is_zero() || cfg.backend == Backend::Codex {
                    continue;
                }
                if id != cur_id {
                    cur_id = id;
                    last_sent_at = None;
                    last_words = 0;
                }
                let words = word_count(&text);
                if !should_send(Instant::now(), last_sent_at, last_words, words, cfg.partial_every, cfg.partial_min_new_words) {
                    continue;
                }
                if engine.as_ref().map_or(true, |e| e.needs_rotation(cfg.rotate_every)) {
                    engine = match Engine::new(&cfg) {
                        Ok(e) => Some(e),
                        Err(e) => {
                            tracing::warn!("临时翻译引擎启动失败：{e}");
                            continue;
                        }
                    };
                }
                let msg = build_user_message(&snapshot_context(&context), &text, true);
                let t0 = Instant::now();
                let ui2 = ui.clone();
                let mut on_delta = |acc: &str| {
                    let _ = ui2.send(UiMsg::ChineseProvisional { id, text: acc.to_string() });
                };
                match engine.as_mut().unwrap().translate(&cfg, &msg, &mut on_delta) {
                    Ok(zh) => {
                        tracing::info!("translate partial[{}] {}ms words={words} {zh:?}", cfg.backend.as_str(), t0.elapsed().as_millis());
                        let _ = ui.send(UiMsg::ChineseProvisional { id, text: zh });
                    }
                    Err(e) => {
                        tracing::warn!("临时翻译失败：{e}");
                        engine = None;
                    }
                }
                last_sent_at = Some(Instant::now());
                last_words = words;
            }
        })
        .expect("spawn partial translator")
}
```

注意 `last_sent_at` 记的是**返回时刻**，规则里的"距上次发出"实际是距上次返回；这样翻译耗时长于间隔时不会连续发。

- [ ] **Step 5: 跑集成测试**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --test translator`
Expected: 全部 PASS（含原有 5 个，它们靠 `ends_with("zh=…")` 与 `pid_of` 不受新前缀影响）。若 `provisional_thread_rate_limits_and_takes_newest` 偶发，把 sleep 800 → 600 再看，不要放宽断言。

- [ ] **Step 6: 提交**

```bash
git add src/translate.rs tests/translator.rs
git commit -m "feat: 定稿翻译带前文并维护前文队列；新增临时翻译线程（限频、只留最新、独立引擎）"
```

---

### Task 5: 识别线程话轮 id 与中间结果送翻

**Files:**
- Modify: `src/asr.rs`（`spawn_asr` 签名与循环；新增 `IdAlloc`；测试）

- [ ] **Step 1: 写失败测试（asr.rs 测试模块追加）**

```rust
    #[test]
    fn id_alloc_shares_id_between_partials_and_final() {
        let mut a = super::IdAlloc::default();
        assert_eq!(a.partial_id(), 1);
        assert_eq!(a.partial_id(), 1, "同一话轮的中间结果共用 id");
        assert_eq!(a.final_id(), 1, "定稿沿用");
        assert_eq!(a.partial_id(), 2, "下一话轮新 id");
        a.discard();
        assert_eq!(a.final_id(), 3, "作废的 id 不复用；没有中间结果直接定稿也能分配");
    }
```

- [ ] **Step 2: 跑确认失败**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib asr::tests::id_alloc`
Expected: 编译错误 `IdAlloc` 未定义。

- [ ] **Step 3: 实现**

在 `spawn_asr` 前加：

```rust
/// 话轮 id：第一个中间结果时分配，同话轮的中间结果与定稿共用；作废不复用
#[derive(Default)]
pub(crate) struct IdAlloc {
    next: SegId,
    cur: Option<SegId>,
}

impl IdAlloc {
    fn alloc(&mut self) -> SegId {
        self.next += 1;
        self.next
    }
    pub(crate) fn partial_id(&mut self) -> SegId {
        match self.cur {
            Some(i) => i,
            None => {
                let i = self.alloc();
                self.cur = Some(i);
                i
            }
        }
    }
    pub(crate) fn final_id(&mut self) -> SegId {
        self.cur.take().unwrap_or_else(|| self.alloc())
    }
    /// 定稿为空（噪声）：当前话轮作废
    pub(crate) fn discard(&mut self) {
        self.cur = None;
    }
}
```

`spawn_asr` 签名加 `tx_partial: Sender<(SegId, String)>`（放在 `tx_tr` 之后）；循环里 `let mut next_id: SegId = 1;` 改为 `let mut ids = IdAlloc::default();`。中间结果分支：

```rust
                        Ok(text) if has_words(&text) => {
                            tracing::info!("asr partial {}ms audio={:.1}s text={text:?}", t0.elapsed().as_millis(), pcm.len() as f32 / 16000.0);
                            let id = ids.partial_id();
                            let _ = ui.send(UiMsg::EnglishPartial { id, text: text.clone() });
                            // 满了就丢：临时翻译线程只要最新一条
                            let _ = tx_partial.try_send((id, text));
                        }
```

定稿分支：

```rust
                        Ok(text) if has_words(&text) => {
                            tracing::info!("asr {}ms audio={:.1}s text={text:?}", t0.elapsed().as_millis(), seg.len() as f32 / 16000.0);
                            let id = ids.final_id();
                            let _ = ui.send(UiMsg::English { id, at, text: text.clone() });
                            // 满了由翻译线程按积压规则处理；这里不阻塞
                            let _ = tx_tr.try_send((id, text));
                        }
                        Ok(_) => {
                            // 这句识别为空（多半是噪声）：把可能挂着的中间结果清掉，话轮作废
                            let _ = ui.send(UiMsg::EnglishPartial { id: 0, text: String::new() });
                            ids.discard();
                        }
```

`next_id` 的其余引用全部删掉。

- [ ] **Step 4: 跑单测**

`main.rs` 此时因 `spawn_asr` 少一个参数编译失败：先在 `main.rs` 的 `spawn_asr` 调用里临时传 `bounded::<(SegId, String)>(4).0`，Task 7 正式接线。

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib asr::tests`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add src/asr.rs src/main.rs
git commit -m "feat: 识别线程按话轮分配 id，中间结果同时送临时翻译"
```

---

### Task 6: 界面——临时中文、公共前缀打字机、定稿接管、设置页

**Files:**
- Modify: `src/ui.rs`（`Line`、`drain`、渲染、`apply_settings`、设置翻译页、`CaptionApp::new` 的 `tx_cfg`、测试）

- [ ] **Step 1: 写失败测试（ui.rs 测试模块追加）**

```rust
    fn line(id: SegId) -> Line {
        Line { id, at: Local::now(), en: "x".into(), zh: None, partial: true, en_shown: 0.0, zh_shown: 0.0, zh_provisional: false, zh_final_started: false }
    }

    #[test]
    fn common_prefix_counts_chars() {
        assert_eq!(common_prefix_chars("你好世界", "你好吗"), 2);
        assert_eq!(common_prefix_chars("abc", "abc"), 3);
        assert_eq!(common_prefix_chars("abc", "xyz"), 0);
        assert_eq!(common_prefix_chars("", "abc"), 0);
    }

    #[test]
    fn set_zh_keeps_shown_prefix_and_retypes_tail() {
        let mut l = line(1);
        l.set_zh("我们现在开始会议".into(), true);
        l.zh_shown = 6.0;
        l.set_zh("我们现在开始今天的会议".into(), true);
        assert_eq!(l.zh_shown, 6.0, "公共前缀 6 字保留");
        l.set_zh("大家好".into(), false);
        assert_eq!(l.zh_shown, 0.0, "完全不同就从头打");
        assert!(!l.zh_provisional);
    }

    #[test]
    fn provisional_is_ignored_once_final_stream_started() {
        let mut l = line(3);
        assert!(l.apply_provisional("半句".into()));
        assert!(l.zh_provisional);
        l.set_zh("定稿".into(), false);
        l.zh_final_started = true;
        assert!(!l.apply_provisional("晚到的半句".into()));
        assert_eq!(l.zh.as_deref(), Some("定稿"));
    }
```

- [ ] **Step 2: 跑确认失败**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib ui::tests`
Expected: 编译错误（字段与方法不存在）。

- [ ] **Step 3: 实现 Line**

```rust
struct Line {
    id: SegId,
    #[allow(dead_code)]
    at: DateTime<Local>,
    en: String,
    zh: Option<String>,
    /// 还在说的这句（中间识别结果），会被替换
    partial: bool,
    /// 打字机：已显示的字符数（浮点便于按帧推进）
    en_shown: f32,
    zh_shown: f32,
    /// 中文是说话过程中翻的半句（灰色斜体）
    zh_provisional: bool,
    /// 定稿译文已开始到达：之后晚到的临时中文一律忽略
    zh_final_started: bool,
}

/// 两段文本开头相同的字符数
fn common_prefix_chars(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

impl Line {
    /// 换中文：公共前缀部分保留已显示进度，只有变动的尾部重新打字
    fn set_zh(&mut self, text: String, provisional: bool) {
        let keep = self.zh.as_deref().map_or(0, |old| common_prefix_chars(old, &text)) as f32;
        self.zh_shown = self.zh_shown.min(keep);
        self.zh = Some(text);
        self.zh_provisional = provisional;
    }

    /// 临时中文：定稿流开始后不再采纳。返回是否采纳
    fn apply_provisional(&mut self, text: String) -> bool {
        if self.zh_final_started {
            return false;
        }
        self.set_zh(text, true);
        true
    }
```

（`advance` 保留在同一个 `impl Line` 里。）现有两处 `Line { … zh_shown: 0.0 }` 字面量（`drain` 里）和测试里的两处都补 `zh_provisional: false, zh_final_started: false`。

- [ ] **Step 4: 实现 drain**

```rust
                UiMsg::EnglishPartial { id, text } => {
                    self.last_activity = Instant::now();
                    if text.is_empty() {
                        if self.lines.back().map_or(false, |l| l.partial) {
                            self.lines.pop_back();
                        }
                        continue;
                    }
                    match self.lines.back_mut() {
                        Some(l) if l.partial => {
                            // 新快照通常是旧快照的延长；已显示的部分不回退
                            l.en_shown = l.en_shown.min(text.chars().count() as f32);
                            l.en = text;
                            l.id = id;
                        }
                        _ => {
                            self.lines.push_back(Line { id, at: Local::now(), en: text, zh: None, partial: true, en_shown: 0.0, zh_shown: 0.0, zh_provisional: false, zh_final_started: false });
                        }
                    }
                }
```

`UiMsg::English` 分支不变（除字面量补字段）。新增与修改：

```rust
                UiMsg::ChineseProvisional { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.apply_provisional(text);
                    }
                }
                UiMsg::ChinesePartial { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.zh_final_started = true;
                        l.set_zh(text, false);
                    }
                }
                UiMsg::Chinese { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.zh_final_started = true;
                        l.set_zh(text.clone(), false);
                    }
                    if let Some(line) = self.transcript.push_zh(id, &text) {
                        if let Some(f) = self.file.as_mut() {
                            f.append(&line);
                        }
                    }
                }
```

- [ ] **Step 5: 渲染临时中文为灰色斜体**

渲染循环里 `Some(zh) =>` 分支改为：

```rust
                            Some(zh) => {
                                let zh_text = head(zh, l.zh_shown);
                                let rt = if l.zh_provisional {
                                    RichText::new(zh_text).size(fs).italics().color(Color32::from_rgba_unmultiplied(215, 220, 230, 170))
                                } else {
                                    RichText::new(zh_text).size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, dim))
                                };
                                ui.add(egui::Label::new(rt).wrap());
                            }
```

- [ ] **Step 6: 配置通道改为多个，设置页加两项**

`CaptionApp` 字段 `tx_cfg: Sender<TranslatorConfig>` 改为 `tx_cfg: Vec<Sender<TranslatorConfig>>`，`new` 的参数同样改为 `tx_cfg: Vec<Sender<TranslatorConfig>>`。`apply_settings` 里：

```rust
                for tx in &self.tx_cfg {
                    let _ = tx.send(TranslatorConfig::from(&self.cfg.translate));
                }
```

翻译页 `ui.label("单句超时");` 那一组之后加：

```rust
            ui.label("临时翻译间隔");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.translate.partial_every_ms).range(0..=5000).speed(50).suffix(" ms"));
                ui.label(RichText::new("说话过程中每隔这么久把半句翻成灰色临时中文，0 关闭；定稿后整句重翻").small().weak());
            });
            ui.end_row();
            ui.label("上下文句数");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.translate.context_sentences).range(0..=6));
                ui.label(RichText::new("翻译时带最近几句英中对照，0 不带").small().weak());
            });
            ui.end_row();
```

- [ ] **Step 7: 跑 ui 单测**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release --lib ui::tests`
Expected: PASS（`main.rs` 因 `CaptionApp::new` 参数类型此时编译失败，`--lib` 不受影响；Task 7 修）。

- [ ] **Step 8: 提交**

```bash
git add src/ui.rs
git commit -m "feat: 界面显示灰色临时中文，定稿流开始后忽略晚到的临时结果；中文替换按公共前缀保留打字进度；设置页加临时翻译间隔与上下文句数"
```

---

### Task 7: main.rs 接线、全量测试、文档

**Files:**
- Modify: `src/main.rs`
- Modify: `CLAUDE.md`（"逐字上屏机制"与"翻译后端"两节）

- [ ] **Step 1: 接线**

`use live_caption::translate::{new_context, spawn_partial_translator, spawn_translator, TranslatorConfig};` 与 `use live_caption::messages::SegId;`。通道段加：

```rust
    let (tx_partial, rx_partial) = bounded::<(SegId, String)>(4);
    let (tx_cfg2, rx_cfg2) = bounded::<TranslatorConfig>(4);
    let context = new_context();
```

`spawn_asr(...)` 的参数在 `tx_tr` 后加 `tx_partial`；把 Task 5 的临时 `bounded(...).0` 去掉。翻译线程两行：

```rust
    spawn_translator(TranslatorConfig::from(&cfg.translate), rx_cfg, rx_tr, tx_ui.clone(), context.clone());
    spawn_partial_translator(TranslatorConfig::from(&cfg.translate), rx_cfg2, rx_partial, tx_ui.clone(), context);
```

`CaptionApp::new(cc, rx_ui, cfg, paused_ui, tx_cfg)` 改为 `CaptionApp::new(cc, rx_ui, cfg, paused_ui, vec![tx_cfg, tx_cfg2])`。

- [ ] **Step 2: 全量测试**

Run: `source scripts/env.sh && PYTHONUTF8=1 cargo test --release`
Expected: lib 全部 PASS、translator 集成 10 个 PASS。再跑 `cargo build --release` 确认二进制编译通过。

- [ ] **Step 3: CLAUDE.md**

"逐字上屏机制"一节末尾加：

```markdown
- **临时翻译**（2026-09-06）：中间英文同时送 `translate::spawn_partial_translator`（独立线程 + 自己的后端实例；claude 是第二个常驻会话），
  按 `translate.partial_every_ms`（默认 1200，0 关）且比上次多 `partial_min_new_words`（默认 3）个词才送翻，只留最新一条，结果 `ChineseProvisional` 灰色斜体上屏；
  定稿译文（`ChinesePartial`/`Chinese`）一到，该行 `zh_final_started` 置真，晚到的临时结果一律忽略。中文替换按公共前缀保留打字进度（`Line::set_zh`）。
  话轮 id 在第一个中间结果时分配（`asr::IdAlloc`），中间结果与定稿共用；空定稿作废 id 不复用。Codex 后端不做临时翻译。
- **带前文翻译**：定稿线程每译完一句把 (en, zh) 推进共享的 `translate::Context`（最近 `context_sentences` 句，默认 2），两个翻译线程组提示词都带（`build_user_message`）。
  假后端 `tests/fake_*.py` 会剥掉提示词只回显句子，并附 `ctx=<n> prov=<0/1>` 供断言。
```

"翻译后端"一节 `claude` 条目末尾加一句：`开临时翻译时会多一个常驻会话（约 300 MB，每句 1.5 s，临时中文比 OpenAI 后端慢）。`

- [ ] **Step 4: 提交**

```bash
git add src/main.rs CLAUDE.md
git commit -m "feat: 接线临时翻译线程与前文队列；交接卡补充"
```

---

### Task 8: 整机验收（Jack 机器，DeepSeek）

**Files:** 无代码改动；产物 `D:\DevCache\lc-target\release\live-caption.exe`。

- [ ] **Step 1: 准备**：确认安装版没在跑（`tasklist | grep live-caption`；在跑就先经托盘"退出"或提示 Jack）。仓库根 `config.toml` 的 `translate.backend = "openai"`、DeepSeek key 在（已是）。
- [ ] **Step 2: 跑验收脚本**

Run: `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\e2e_check.ps1`
Expected：日志出现 `translate partial[openai] <ms> words=<n> "…"` 若干行，且早于同话轮的 `translate[openai]`；`asr partial` 到对应 `translate partial` 的时间差 ≈ 翻译往返（DeepSeek 0.6–0.9 s）；定稿 `translate[openai]` 照旧；记录文件只有定稿中文。

- [ ] **Step 3: 量化落后时间**

从日志取一段，算首条临时中文相对首条中间英文的延迟，记入 CLAUDE.md"实测延迟"一节（一行）。目标 1.5–2 s。

- [ ] **Step 4: 提交**

```bash
git add CLAUDE.md
git commit -m "docs: 临时翻译实测延迟"
```
