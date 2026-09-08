//! claude CLI 常驻会话（stream-json 协议）
use crate::backend::BackendConfig;
use crate::util::resolve_bin;
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

pub struct ClaudeSession {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Receiver<String>,
    turns: u32,
}

impl ClaudeSession {
    pub fn spawn(cfg: &BackendConfig, system_prompt: &str) -> anyhow::Result<ClaudeSession> {
        let (prog, pre): (String, Vec<String>) = if cfg.command_override.is_empty() {
            (resolve_bin(&cfg.claude_bin), Vec::new())
        } else {
            (cfg.command_override[0].clone(), cfg.command_override[1..].to_vec())
        };
        let mut cmd = Command::new(&prog);
        cmd.args(&pre)
            .args(CLAUDE_FIXED_ARGS)
            .args(["--model", &cfg.claude_model, "--system-prompt", system_prompt])
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

    pub fn turns(&self) -> u32 {
        self.turns
    }

    pub fn translate(&mut self, user_message: &str, timeout: Duration, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        writeln!(self.stdin, "{}", user_line(user_message))?;
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
}
