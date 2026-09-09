//! 热键字符串解析与全局热键注册
use global_hotkey::hotkey::{Code, HotKey, Modifiers};

/// 悬停触发用的修饰键
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverModifier {
    Alt,
    Ctrl,
    Shift,
    Win,
    /// 关闭悬停翻译
    None,
}

impl HoverModifier {
    pub fn parse(s: &str) -> HoverModifier {
        match s.trim().to_ascii_lowercase().as_str() {
            "alt" => HoverModifier::Alt,
            "ctrl" | "control" => HoverModifier::Ctrl,
            "shift" => HoverModifier::Shift,
            "win" | "super" | "meta" => HoverModifier::Win,
            _ => HoverModifier::None,
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            HoverModifier::Alt => "Alt",
            HoverModifier::Ctrl => "Ctrl",
            HoverModifier::Shift => "Shift",
            HoverModifier::Win => "Win",
            HoverModifier::None => "关闭",
        }
    }

    /// 该修饰键当前是否按下
    #[cfg(windows)]
    pub fn is_down(self) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_LWIN, VK_MENU, VK_CONTROL, VK_RWIN, VK_SHIFT,
        };
        let down = |vk: u16| unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 };
        match self {
            HoverModifier::Alt => down(VK_MENU),
            HoverModifier::Ctrl => down(VK_CONTROL),
            HoverModifier::Shift => down(VK_SHIFT),
            HoverModifier::Win => down(VK_LWIN) || down(VK_RWIN),
            HoverModifier::None => false,
        }
    }

    #[cfg(not(windows))]
    pub fn is_down(self) -> bool {
        false
    }
}

/// 把 "Ctrl+Alt+T" 解析成热键。不认识的写法返回 Err，调用方应提示用户而不是静默忽略。
pub fn parse_hotkey(s: &str) -> anyhow::Result<HotKey> {
    let s = s.trim();
    anyhow::ensure!(!s.is_empty(), "热键为空");
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" => mods |= Modifiers::ALT,
            "shift" => mods |= Modifiers::SHIFT,
            "win" | "super" | "meta" | "cmd" => mods |= Modifiers::META,
            other => {
                anyhow::ensure!(key.is_none(), "热键里有多个主键：{s}");
                key = Some(parse_code(other).ok_or_else(|| anyhow::anyhow!("不认识的按键：{other}"))?);
            }
        }
    }
    let code = key.ok_or_else(|| anyhow::anyhow!("热键缺少主键：{s}"))?;
    anyhow::ensure!(!mods.is_empty(), "热键至少要有一个修饰键：{s}");
    Ok(HotKey::new(Some(mods), code))
}

fn parse_code(s: &str) -> Option<Code> {
    let up = s.to_ascii_uppercase();
    // 字母
    if up.len() == 1 {
        let c = up.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            return Some(match c {
                'A' => Code::KeyA, 'B' => Code::KeyB, 'C' => Code::KeyC, 'D' => Code::KeyD,
                'E' => Code::KeyE, 'F' => Code::KeyF, 'G' => Code::KeyG, 'H' => Code::KeyH,
                'I' => Code::KeyI, 'J' => Code::KeyJ, 'K' => Code::KeyK, 'L' => Code::KeyL,
                'M' => Code::KeyM, 'N' => Code::KeyN, 'O' => Code::KeyO, 'P' => Code::KeyP,
                'Q' => Code::KeyQ, 'R' => Code::KeyR, 'S' => Code::KeyS, 'T' => Code::KeyT,
                'U' => Code::KeyU, 'V' => Code::KeyV, 'W' => Code::KeyW, 'X' => Code::KeyX,
                'Y' => Code::KeyY, _ => Code::KeyZ,
            });
        }
        if c.is_ascii_digit() {
            return Some(match c {
                '0' => Code::Digit0, '1' => Code::Digit1, '2' => Code::Digit2, '3' => Code::Digit3,
                '4' => Code::Digit4, '5' => Code::Digit5, '6' => Code::Digit6, '7' => Code::Digit7,
                '8' => Code::Digit8, _ => Code::Digit9,
            });
        }
    }
    // 功能键
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return Some(match n {
            1 => Code::F1, 2 => Code::F2, 3 => Code::F3, 4 => Code::F4,
            5 => Code::F5, 6 => Code::F6, 7 => Code::F7, 8 => Code::F8,
            9 => Code::F9, 10 => Code::F10, 11 => Code::F11, 12 => Code::F12,
            _ => return None,
        });
    }
    Some(match up.as_str() {
        "SPACE" => Code::Space,
        "ENTER" | "RETURN" => Code::Enter,
        "ESC" | "ESCAPE" => Code::Escape,
        "TAB" => Code::Tab,
        "BACKQUOTE" | "`" => Code::Backquote,
        "COMMA" | "," => Code::Comma,
        "PERIOD" | "." => Code::Period,
        "SLASH" | "/" => Code::Slash,
        "SEMICOLON" | ";" => Code::Semicolon,
        "QUOTE" | "'" => Code::Quote,
        "BRACKETLEFT" | "[" => Code::BracketLeft,
        "BRACKETRIGHT" | "]" => Code::BracketRight,
        "MINUS" | "-" => Code::Minus,
        "EQUAL" | "=" => Code::Equal,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_combinations() {
        assert!(parse_hotkey("Ctrl+Alt+T").is_ok());
        assert!(parse_hotkey("ctrl + shift + f9").is_ok());
        assert!(parse_hotkey("Win+Space").is_ok());
        assert!(parse_hotkey("Ctrl+/").is_ok());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_hotkey("").is_err());
        assert!(parse_hotkey("T").is_err(), "没有修饰键");
        assert!(parse_hotkey("Ctrl").is_err(), "没有主键");
        assert!(parse_hotkey("Ctrl+Alt+T+Y").is_err(), "两个主键");
        assert!(parse_hotkey("Ctrl+Nope").is_err());
    }

    #[test]
    fn same_string_gives_same_id() {
        let a = parse_hotkey("Ctrl+Alt+T").unwrap();
        let b = parse_hotkey("alt+ctrl+t").unwrap();
        assert_eq!(a.id(), b.id(), "顺序与大小写不影响");
    }

    #[test]
    fn hover_modifier_parse() {
        assert_eq!(HoverModifier::parse("Alt"), HoverModifier::Alt);
        assert_eq!(HoverModifier::parse("control"), HoverModifier::Ctrl);
        assert_eq!(HoverModifier::parse("none"), HoverModifier::None);
        assert_eq!(HoverModifier::parse("乱写"), HoverModifier::None);
    }
}
