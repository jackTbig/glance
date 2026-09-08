//! 探测热键是否真的被占用：直接调 Win32 RegisterHotKey 看错误码。
//! global-hotkey 0.8 把所有 OS 错误都报成 AlreadyRegistered（它那句 if-let 是变量绑定不是常量匹配），
//! 所以它的错误信息不可信，要自己探。
#[cfg(windows)]
fn main() {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
    };
    let combos: &[(&str, u32, u32)] = &[
        ("Ctrl+Alt+T", MOD_CONTROL | MOD_ALT, 0x54),
        ("Ctrl+Alt+S", MOD_CONTROL | MOD_ALT, 0x53),
        ("Ctrl+Alt+X", MOD_CONTROL | MOD_ALT, 0x58),
        ("Ctrl+Alt+D", MOD_CONTROL | MOD_ALT, 0x44),
        ("Ctrl+Alt+Q", MOD_CONTROL | MOD_ALT, 0x51),
        ("Ctrl+Alt+W", MOD_CONTROL | MOD_ALT, 0x57),
        ("Ctrl+Alt+G", MOD_CONTROL | MOD_ALT, 0x47),
        ("Ctrl+Alt+Y", MOD_CONTROL | MOD_ALT, 0x59),
        ("Ctrl+Shift+T", MOD_CONTROL | MOD_SHIFT, 0x54),
        ("Ctrl+Shift+D", MOD_CONTROL | MOD_SHIFT, 0x44),
        ("Ctrl+Shift+Y", MOD_CONTROL | MOD_SHIFT, 0x59),
        ("Ctrl+Shift+Q", MOD_CONTROL | MOD_SHIFT, 0x51),
        ("Alt+Shift+T", MOD_ALT | MOD_SHIFT, 0x54),
        ("Alt+Shift+S", MOD_ALT | MOD_SHIFT, 0x53),
        ("Alt+Shift+Q", MOD_ALT | MOD_SHIFT, 0x51),
        ("Alt+Shift+Z", MOD_ALT | MOD_SHIFT, 0x5A),
        ("Ctrl+Alt+F9", MOD_CONTROL | MOD_ALT, 0x78),
        ("Ctrl+Alt+F10", MOD_CONTROL | MOD_ALT, 0x79),
        ("Ctrl+Shift+F12", MOD_CONTROL | MOD_SHIFT, 0x7B),
    ];
    for (i, (name, mods, vk)) in combos.iter().enumerate() {
        let id = 9000 + i as i32;
        unsafe {
            let ok = RegisterHotKey(std::ptr::null_mut(), id, *mods, *vk);
            if ok != 0 {
                println!("{name:14} 可用");
                UnregisterHotKey(std::ptr::null_mut(), id);
            } else {
                let e = GetLastError();
                let why = match e {
                    1409 => "已被其他程序占用（ERROR_HOTKEY_ALREADY_REGISTERED）",
                    87 => "参数无效（ERROR_INVALID_PARAMETER）",
                    _ => "其他错误",
                };
                println!("{name:14} 失败 错误码 {e}：{why}");
            }
        }
    }
}

#[cfg(not(windows))]
fn main() {
    println!("只在 Windows 上有意义");
}
