use crate::config::TranslateCfg;
use crate::messages::{SegId, Stage, StageState, UiMsg};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const SYSTEM_PROMPT: &str = "你是英译中会议同传。用户每条消息是一句英文会议发言，你只输出对应的简体中文译文，不加解释、不加引号、不加前缀。专有名词与产品名保留原文。";

/// 实测（2026-09-03）：关掉 MCP 与设置源后单句往返从 4–7 s 降到约 1.5 s；工具用 --disallowedTools "*" 关闭；
/// --include-partial-messages 让译文按 token 流式回来
pub const CLAUDE_FIXED_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--disallowedTools",
    "*",
    "--strict-mcp-config",
    "--mcp-config",
    "{\"mcpServers\":{}}",
    "--setting-sources",
    "",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Claude,
    Codex,
    OpenAi,
}

impl Backend {
    pub fn parse(s: &str) -> Backend {
        match s.trim().to_ascii_lowercase().as_str() {
            "codex" => Backend::Codex,
            "openai" | "deepseek" | "qwen" | "api" => Backend::OpenAi,
            _ => Backend::Claude,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Claude => "claude",
            Backend::Codex => "codex",
            Backend::OpenAi => "openai",
        }
    }
}

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
        }
    }
}

/// Windows 上 `Command::new("codex")` 不会找 .cmd/.bat；这里按 PATH 补全成完整路径
pub fn resolve_bin(name: &str) -> String {
    let p = Path::new(name);
    if p.components().count() > 1 || p.exists() {
        return name.to_string();
    }
    let exts = ["exe", "cmd", "bat", "com"];
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for ext in exts {
                let cand: PathBuf = dir.join(format!("{name}.{ext}"));
                if cand.is_file() {
                    return cand.to_string_lossy().into_owned();
                }
            }
            let bare = dir.join(name);
            if bare.is_file() {
                return bare.to_string_lossy().into_owned();
            }
        }
    }
    // PATH 里没有：再看 claude 原生安装器与 npm 全局的常见位置
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        extra.push(PathBuf::from(&home).join(".local").join("bin").join(format!("{name}.exe")));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        extra.push(PathBuf::from(&appdata).join("npm").join(format!("{name}.cmd")));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        extra.push(PathBuf::from(&local).join("Programs").join(name).join(format!("{name}.exe")));
    }
    for cand in extra {
        if cand.is_file() {
            return cand.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

// ---------- claude CLI 协议 ----------

pub fn user_line(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
        "parent_tool_use_id": null
    })
    .to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultEvent {
    pub text: String,
    pub is_error: bool,
}

pub fn parse_result_line(line: &str) -> Option<ResultEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "result" {
        return None;
    }
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
    let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) || subtype.starts_with("error");
    let text = v.get("result").and_then(|s| s.as_str()).unwrap_or("").trim().to_string();
    Some(ResultEvent { text, is_error })
}

/// `--include-partial-messages` 下的文本增量
pub fn parse_delta_line(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "stream_event" {
        return None;
    }
    let ev = v.get("event")?;
    if ev.get("type")?.as_str()? != "content_block_delta" {
        return None;
    }
    let d = ev.get("delta")?;
    if d.get("type")?.as_str()? != "text_delta" {
        return None;
    }
    Some(d.get("text")?.as_str()?.to_string())
}

struct ClaudeSession {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Receiver<String>,
    turns: u32,
}

impl ClaudeSession {
    fn spawn(cfg: &TranslatorConfig) -> anyhow::Result<ClaudeSession> {
        let (prog, pre): (String, Vec<String>) = if cfg.command_override.is_empty() {
            (resolve_bin(&cfg.claude_bin), Vec::new())
        } else {
            (cfg.command_override[0].clone(), cfg.command_override[1..].to_vec())
        };
        let mut cmd = Command::new(&prog);
        cmd.args(&pre)
            .args(CLAUDE_FIXED_ARGS)
            .args(["--model", &cfg.claude_model, "--system-prompt", SYSTEM_PROMPT])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 {prog} 失败：{e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = bounded::<String>(1024);
        std::thread::Builder::new().name("claude-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })?;
        Ok(ClaudeSession { child, stdin, lines: rx, turns: 0 })
    }

    fn translate(&mut self, text: &str, timeout: Duration, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        writeln!(self.stdin, "{}", user_line(text))?;
        self.stdin.flush()?;
        let deadline = Instant::now() + timeout;
        let mut acc = String::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                anyhow::bail!("超时");
            }
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Some(d) = parse_delta_line(&line) {
                        acc.push_str(&d);
                        on_delta(&acc);
                    } else if let Some(r) = parse_result_line(&line) {
                        self.turns += 1;
                        if r.is_error {
                            anyhow::bail!("claude 返回错误：{}", r.text);
                        }
                        return Ok(if r.text.is_empty() { acc.trim().to_string() } else { r.text });
                    }
                }
                Err(RecvTimeoutError::Timeout) => anyhow::bail!("超时"),
                Err(RecvTimeoutError::Disconnected) => anyhow::bail!("子进程已退出"),
            }
        }
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- codex CLI（每句一个 codex exec） ----------

fn codex_translate(cfg: &TranslatorConfig, text: &str) -> anyhow::Result<String> {
    let bin = resolve_bin(&cfg.codex_bin);
    let tmp = std::env::temp_dir();
    let out = tmp.join(format!("live-caption-codex-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let mut cmd = Command::new(&bin);
    cmd.args(["exec", "--skip-git-repo-check", "-s", "read-only", "-C"])
        .arg(&tmp)
        .arg("-o")
        .arg(&out);
    if !cfg.codex_model.trim().is_empty() {
        cmd.args(["-m", cfg.codex_model.trim()]);
    }
    cmd.arg(format!("{SYSTEM_PROMPT}\n\n{text}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 {bin} 失败：{e}"))?;
    let deadline = Instant::now() + cfg.timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                anyhow::bail!("codex exec 退出码 {status}");
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("超时");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = std::fs::read_to_string(&out).map_err(|e| anyhow::anyhow!("codex 没有写出结果：{e}"))?;
    let _ = std::fs::remove_file(&out);
    Ok(s.trim().to_string())
}

// ---------- OpenAI 兼容接口（SSE 流式） ----------

struct OpenAi {
    client: reqwest::blocking::Client,
    url: String,
    key: String,
    model: String,
}

impl OpenAi {
    fn new(cfg: &TranslatorConfig) -> anyhow::Result<Self> {
        let base = cfg.openai_base_url.trim().trim_end_matches('/');
        anyhow::ensure!(!base.is_empty(), "openai_base_url 为空");
        let client = reqwest::blocking::Client::builder().timeout(cfg.timeout).build()?;
        Ok(Self { client, url: format!("{base}/chat/completions"), key: cfg.openai_api_key.trim().to_string(), model: cfg.openai_model.trim().to_string() })
    }

    fn translate(&self, text: &str, timeout: Duration, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "stream": true,
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": text }
            ]
        });
        let mut req = self.client.post(&self.url).timeout(timeout).json(&body);
        if !self.key.is_empty() {
            req = req.bearer_auth(&self.key);
        }
        let resp = req.send()?;
        let status = resp.status();
        if !status.is_success() {
            let msg = resp.text().unwrap_or_default();
            anyhow::bail!("HTTP {status}: {}", msg.chars().take(200).collect::<String>());
        }
        let mut acc = String::new();
        let mut saw_sse = false;
        let mut raw = String::new();
        for line in BufReader::new(resp).lines() {
            let line = line?;
            if let Some(data) = line.strip_prefix("data:") {
                saw_sse = true;
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }
                let v: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(c) = v["choices"][0]["delta"]["content"].as_str() {
                    if !c.is_empty() {
                        acc.push_str(c);
                        on_delta(&acc);
                    }
                }
            } else if !saw_sse {
                raw.push_str(&line);
            }
        }
        if !saw_sse {
            // 服务端没按 SSE 回：按普通 JSON 解析
            let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("响应不是 SSE 也不是 JSON：{e}"))?;
            if let Some(c) = v["choices"][0]["message"]["content"].as_str() {
                acc = c.to_string();
            } else if let Some(err) = v["error"]["message"].as_str() {
                anyhow::bail!("接口错误：{err}");
            }
        }
        Ok(acc.trim().to_string())
    }
}

// ---------- 引擎 ----------

enum Engine {
    Claude(ClaudeSession),
    Codex,
    OpenAi(OpenAi),
}

impl Engine {
    fn new(cfg: &TranslatorConfig) -> anyhow::Result<Engine> {
        Ok(match cfg.backend {
            Backend::Claude => Engine::Claude(ClaudeSession::spawn(cfg)?),
            Backend::Codex => Engine::Codex,
            Backend::OpenAi => Engine::OpenAi(OpenAi::new(cfg)?),
        })
    }

    fn translate(&mut self, cfg: &TranslatorConfig, text: &str, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        match self {
            Engine::Claude(s) => s.translate(text, cfg.timeout, on_delta),
            Engine::Codex => codex_translate(cfg, text),
            Engine::OpenAi(o) => o.translate(text, cfg.timeout, on_delta),
        }
    }

    fn turns(&self) -> u32 {
        match self {
            Engine::Claude(s) => s.turns,
            _ => 0,
        }
    }

    fn needs_rotation(&self, every: u32) -> bool {
        matches!(self, Engine::Claude(_)) && self.turns() >= every
    }
}

/// 翻译线程。cfg_rx 收到新配置即热切换后端；claude 会话启动即预热，轮换前 5 句预起备用会话。
pub fn spawn_translator(
    initial: TranslatorConfig,
    cfg_rx: Receiver<TranslatorConfig>,
    rx: Receiver<(SegId, String)>,
    ui: Sender<UiMsg>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("translator".into())
        .spawn(move || {
            let mut cfg = initial;
            let report_err = |ui: &Sender<UiMsg>, e: &anyhow::Error| {
                tracing::error!("{e}");
                let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state: StageState::Err(e.to_string()) });
            };
            let mut engine: Option<Engine> = Engine::new(&cfg).map_err(|e| report_err(&ui, &e)).ok();
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
                    if standby.is_none() {
                        if let Some(Engine::Claude(s)) = &engine {
                            if s.turns + 5 >= cfg.rotate_every {
                                standby = Engine::new(&cfg).map_err(|e| tracing::warn!("备用会话启动失败：{e}")).ok();
                            }
                        }
                    }
                    if engine.as_ref().map_or(true, |e| e.needs_rotation(cfg.rotate_every)) {
                        engine = match standby.take() {
                            Some(s) => Some(s),
                            None => match Engine::new(&cfg) {
                                Ok(e) => Some(e),
                                Err(e) => {
                                    report_err(&ui, &e);
                                    let _ = ui.send(UiMsg::Chinese { id, text: "（翻译不可用）".into() });
                                    continue;
                                }
                            },
                        };
                    }
                    let t0 = Instant::now();
                    let ui2 = ui.clone();
                    let mut on_delta = |acc: &str| {
                        let _ = ui2.send(UiMsg::ChinesePartial { id, text: acc.to_string() });
                    };
                    match engine.as_mut().unwrap().translate(&cfg, &en, &mut on_delta) {
                        Ok(zh) => {
                            tracing::info!("translate[{}] {}ms {zh:?}", cfg.backend.as_str(), t0.elapsed().as_millis());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_line_is_single_line_json() {
        let l = user_line("hello \"world\"\nnext");
        assert!(!l.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], "hello \"world\"\nnext");
    }

    #[test]
    fn parse_result_success() {
        let r = parse_result_line(r#"{"type":"result","subtype":"success","result":"你好"}"#).unwrap();
        assert_eq!(r.text, "你好");
        assert!(!r.is_error);
    }

    #[test]
    fn parse_result_error_subtype() {
        let r = parse_result_line(r#"{"type":"result","subtype":"error_during_execution","result":"","is_error":true}"#).unwrap();
        assert!(r.is_error);
    }

    #[test]
    fn non_result_lines_are_none() {
        assert!(parse_result_line(r#"{"type":"system","subtype":"init"}"#).is_none());
        assert!(parse_result_line("not json").is_none());
    }

    #[test]
    fn parse_delta_extracts_text_delta_only() {
        let l = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"你好"}}}"#;
        assert_eq!(parse_delta_line(l).as_deref(), Some("你好"));
        let other = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        assert!(parse_delta_line(other).is_none());
        assert!(parse_delta_line(r#"{"type":"result","result":"x"}"#).is_none());
    }

    #[test]
    fn backend_parse_is_lenient() {
        assert_eq!(Backend::parse("Codex"), Backend::Codex);
        assert_eq!(Backend::parse("deepseek"), Backend::OpenAi);
        assert_eq!(Backend::parse("whatever"), Backend::Claude);
    }

    #[test]
    fn resolve_bin_keeps_paths_and_finds_python_on_path() {
        assert_eq!(resolve_bin(r"C:\x\y.exe"), r"C:\x\y.exe");
        let p = resolve_bin("python");
        assert!(p.to_ascii_lowercase().ends_with("python.exe") || p == "python", "{p}");
    }
}
