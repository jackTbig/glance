//! 简译 ↔ 字幕之间的控制协议。**协议定义放在共享 crate 里，两边各实现一半。**
//!
//! 不能放在字幕那侧：简译要引用它就得依赖 live-caption crate，
//! 而那会把 whisper-rs 拉进简译的依赖图——于是 `cargo build -p translate-tool`
//! 又要 CMake、Ninja 和 Vulkan SDK，整个「轻量端不受字幕拖累」的前提就没了。
//! 也不该两边各抄一份：三个 bool 的 JSON 抄错了不会编译报错，只会在运行时勾选态错乱。
//!
//! 两条链路方向相反：
//! - **父 → 子**：字幕起的 HTTP 服务，简译用它查勾选态（`/state`）与下命令（`/cmd`）。
//! - **子 → 父**：字幕心跳去敲简译的 `/v1/health`，连不上就自救（关穿透 + 恢复自己的托盘）。
//!
//! 都只绑 127.0.0.1：这些命令能开关鼠标穿透、能让进程退出，绝不能对外可达。

/// 字幕控制端点的两条路径
pub const PATH_STATE: &str = "/state";
pub const PATH_CMD: &str = "/cmd";

/// 父进程能下的命令。字符串就是 HTTP 里传的 action 值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// 无条件切回可点击。**父进程只转发一个热键的话，必须是这个**——
    /// 「切换穿透」在状态不同步时可能反而把穿透打开，而这一条永远是往安全方向走。
    Clickable,
    TogglePassthrough,
    TogglePause,
    ToggleMini,
    OpenSettings,
    OpenTranscript,
    Quit,
}

impl Command {
    pub fn parse(s: &str) -> Option<Command> {
        Some(match s.trim() {
            "clickable" => Command::Clickable,
            "passthrough" => Command::TogglePassthrough,
            "pause" => Command::TogglePause,
            "mini" => Command::ToggleMini,
            "settings" => Command::OpenSettings,
            "transcript" => Command::OpenTranscript,
            "quit" => Command::Quit,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Command::Clickable => "clickable",
            Command::TogglePassthrough => "passthrough",
            Command::TogglePause => "pause",
            Command::ToggleMini => "mini",
            Command::OpenSettings => "settings",
            Command::OpenTranscript => "transcript",
            Command::Quit => "quit",
        }
    }
}

/// 父进程弹托盘菜单前要读的勾选态。
///
/// 菜单项是有状态的（暂停/穿透/迷你条三个勾），所以内部端点必须双向，
/// 不能只做单向命令通道。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    pub paused: bool,
    pub passthrough: bool,
    pub mini: bool,
}

impl State {
    pub fn to_json(self) -> String {
        format!(
            r#"{{"paused":{},"passthrough":{},"mini":{}}}"#,
            self.paused, self.passthrough, self.mini
        )
    }

    /// 手写解析：只有三个 bool，为它引一个 JSON 依赖不值当
    pub fn from_json(s: &str) -> Option<State> {
        let field = |k: &str| -> Option<bool> {
            let at = s.find(&format!("\"{k}\""))?;
            let rest = &s[at + k.len() + 2..];
            let v = rest.trim_start_matches([':', ' ']);
            if v.starts_with("true") {
                Some(true)
            } else if v.starts_with("false") {
                Some(false)
            } else {
                None
            }
        };
        Some(State { paused: field("paused")?, passthrough: field("passthrough")?, mini: field("mini")? })
    }
}


/// 把命令包成 `/cmd` 的请求体
pub fn cmd_body(c: Command) -> String {
    format!(r#"{{"action":"{}"}}"#, c.as_str())
}

/// 从 `{"action":"clickable"}` 里取出 action。只有一个字段，不值当引 JSON 依赖。
pub fn extract_action(body: &str) -> Option<&str> {
    let at = body.find("\"action\"")?;
    let rest = &body[at + 8..];
    let rest = rest.trim_start_matches([':', ' ']);
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_round_trip() {
        for c in [
            Command::Clickable,
            Command::TogglePassthrough,
            Command::TogglePause,
            Command::ToggleMini,
            Command::OpenSettings,
            Command::OpenTranscript,
            Command::Quit,
        ] {
            assert_eq!(Command::parse(c.as_str()), Some(c));
            assert_eq!(extract_action(&cmd_body(c)), Some(c.as_str()));
        }
        assert_eq!(Command::parse("绝无此项"), None);
    }

    #[test]
    fn state_json_round_trips() {
        for s in [
            State::default(),
            State { paused: true, passthrough: false, mini: true },
            State { paused: false, passthrough: true, mini: false },
        ] {
            assert_eq!(State::from_json(&s.to_json()), Some(s), "{}", s.to_json());
        }
        assert_eq!(State::from_json("{}"), None);
        assert_eq!(State::from_json("不是 json"), None);
    }

    #[test]
    fn extract_action_tolerates_spacing_and_rejects_junk() {
        assert_eq!(extract_action(r#"{ "action" : "mini" }"#), Some("mini"));
        for junk in ["{}", "", r#"{"action":123}"#, r#"{"act":"mini"}"#] {
            assert_eq!(extract_action(junk), None, "{junk:?}");
        }
    }
}
