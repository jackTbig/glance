use crate::messages::{SegId, Stage, StageState, UiMsg};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub const SYSTEM_PROMPT: &str = "你是英译中会议同传。用户每条消息是一句英文会议发言，你只输出对应的简体中文译文，不加解释、不加引号、不加前缀。专有名词与产品名保留原文。";

/// 实测（2026-09-03）：关掉 MCP 与设置源后单句往返从 4–7 s 降到约 1.5 s；工具用 --disallowedTools "*" 关闭
pub const FIXED_ARGS: &[&str] = &[
    "-p",
    "--input-format",
    "stream-json",
    "--output-format",
    "stream-json",
    "--verbose",
    "--disallowedTools",
    "*",
    "--strict-mcp-config",
    "--mcp-config",
    "{\"mcpServers\":{}}",
    "--setting-sources",
    "",
];

#[derive(Debug, Clone)]
pub struct TranslatorConfig {
    /// 命令与前置参数，如 ["C:\\...\\claude.exe"] 或测试用 ["python", "tests/fake_claude.py"]
    pub command: Vec<String>,
    pub model: String,
    pub timeout: Duration,
    pub rotate_every: u32,
    pub max_backlog: usize,
}

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

struct Session {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Receiver<String>,
    turns: u32,
}

impl Session {
    fn spawn(cfg: &TranslatorConfig) -> anyhow::Result<Session> {
        let (prog, pre) = cfg.command.split_first().ok_or_else(|| anyhow::anyhow!("command 为空"))?;
        let mut cmd = Command::new(prog);
        cmd.args(pre)
            .args(FIXED_ARGS)
            .args(["--model", &cfg.model, "--system-prompt", SYSTEM_PROMPT])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 {prog} 失败：{e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = bounded::<String>(256);
        std::thread::Builder::new().name("claude-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })?;
        Ok(Session { child, stdin, lines: rx, turns: 0 })
    }

    /// 送一句，等 result；超时或子进程退出返回 Err
    fn translate(&mut self, text: &str, timeout: Duration) -> anyhow::Result<String> {
        writeln!(self.stdin, "{}", user_line(text))?;
        self.stdin.flush()?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                anyhow::bail!("超时");
            }
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Some(r) = parse_result_line(&line) {
                        self.turns += 1;
                        if r.is_error {
                            anyhow::bail!("claude 返回错误：{}", r.text);
                        }
                        return Ok(r.text);
                    }
                }
                Err(RecvTimeoutError::Timeout) => anyhow::bail!("超时"),
                Err(RecvTimeoutError::Disconnected) => anyhow::bail!("子进程已退出"),
            }
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 翻译线程。会话启动即预热；到轮换点前 5 句先起一个备用会话，轮换时无缝切换。
pub fn spawn_translator(cfg: TranslatorConfig, rx: Receiver<(SegId, String)>, ui: Sender<UiMsg>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("translator".into())
        .spawn(move || {
            let report_spawn_err = |e: &anyhow::Error| {
                tracing::error!("{e}");
                let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state: StageState::Err(e.to_string()) });
            };
            let mut session: Option<Session> = Session::spawn(&cfg).map_err(|e| report_spawn_err(&e)).ok();
            let mut standby: Option<Session> = None;
            loop {
                // 取下一句；合并积压，超过 max_backlog 的旧句直接标记跳过
                let first = match rx.recv() {
                    Ok(x) => x,
                    Err(_) => return,
                };
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
                    // 快到轮换点：提前起备用会话
                    if standby.is_none() {
                        if let Some(s) = &session {
                            if s.turns + 5 >= cfg.rotate_every {
                                standby = Session::spawn(&cfg).map_err(|e| tracing::warn!("备用会话启动失败：{e}")).ok();
                            }
                        }
                    }
                    if session.as_ref().map_or(true, |s| s.turns >= cfg.rotate_every) {
                        session = match standby.take() {
                            Some(s) => Some(s),
                            None => match Session::spawn(&cfg) {
                                Ok(s) => Some(s),
                                Err(e) => {
                                    report_spawn_err(&e);
                                    let _ = ui.send(UiMsg::Chinese { id, text: "（翻译不可用）".into() });
                                    continue;
                                }
                            },
                        };
                    }
                    let t0 = std::time::Instant::now();
                    match session.as_mut().unwrap().translate(&en, cfg.timeout) {
                        Ok(zh) => {
                            tracing::info!("translate {}ms {zh:?}", t0.elapsed().as_millis());
                            let _ = ui.send(UiMsg::Chinese { id, text: zh });
                        }
                        Err(e) => {
                            tracing::warn!("翻译失败，重启会话：{e}");
                            session = None;
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
}
