//! 划词：保存剪贴板 → 模拟 Ctrl+C → 读取新内容 → 还原剪贴板
//!
//! 已知限制：还原只还原 Unicode 文本。原剪贴板里是图片或文件时，划词后会丢失。

#[cfg(windows)]
mod win {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HANDLE, HWND};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL,
    };

    const CF_UNICODETEXT: u32 = 13;

    struct Clip;
    impl Clip {
        /// 打开剪贴板；别的进程可能正占着，重试几次
        fn open() -> anyhow::Result<Clip> {
            for i in 0..10 {
                if unsafe { OpenClipboard(std::ptr::null_mut::<std::ffi::c_void>() as HWND) } != 0 {
                    return Ok(Clip);
                }
                std::thread::sleep(Duration::from_millis(10 * (i + 1)));
            }
            anyhow::bail!("打不开剪贴板（被其他程序占用）")
        }
    }
    impl Drop for Clip {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    pub fn get_text() -> anyhow::Result<String> {
        let _c = Clip::open()?;
        unsafe {
            let h = GetClipboardData(CF_UNICODETEXT);
            if h.is_null() {
                return Ok(String::new());
            }
            let p = GlobalLock(h as _) as *const u16;
            if p.is_null() {
                return Ok(String::new());
            }
            let mut len = 0usize;
            while *p.add(len) != 0 && len < 4 * 1024 * 1024 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            GlobalUnlock(h as _);
            Ok(s)
        }
    }

    pub fn set_text(text: &str) -> anyhow::Result<()> {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let _c = Clip::open()?;
        unsafe {
            if EmptyClipboard() == 0 {
                anyhow::bail!("EmptyClipboard 失败");
            }
            let bytes = wide.len() * 2;
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
            anyhow::ensure!(!h.is_null(), "GlobalAlloc 失败");
            let p = GlobalLock(h) as *mut u16;
            anyhow::ensure!(!p.is_null(), "GlobalLock 失败");
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            GlobalUnlock(h);
            if SetClipboardData(CF_UNICODETEXT, h as HANDLE).is_null() {
                anyhow::bail!("SetClipboardData 失败");
            }
        }
        Ok(())
    }

    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        let mut i: INPUT = unsafe { std::mem::zeroed() };
        i.r#type = INPUT_KEYBOARD;
        i.Anonymous.ki = KEYBDINPUT {
            wVk: vk,
            wScan: 0,
            dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dwExtraInfo: 0,
        };
        i
    }

    /// 送一次 Ctrl+C
    fn send_copy() {
        const VK_C: VIRTUAL_KEY = 0x43;
        let seq = [key(VK_CONTROL, false), key(VK_C, false), key(VK_C, true), key(VK_CONTROL, true)];
        unsafe {
            SendInput(seq.len() as u32, seq.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
    }

    /// 取当前选中的文本。整个过程保存并还原原剪贴板内容。
    pub fn selected_text() -> anyhow::Result<String> {
        let saved = get_text().unwrap_or_default();
        let before = unsafe { GetClipboardSequenceNumber() };
        std::thread::sleep(Duration::from_millis(20));
        send_copy();
        // 等剪贴板真的变化（最多 400 ms）
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut changed = false;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(15));
            if unsafe { GetClipboardSequenceNumber() } != before {
                changed = true;
                break;
            }
        }
        if !changed {
            return Ok(String::new()); // 没有选中内容，或目标程序不支持复制
        }
        let text = get_text().unwrap_or_default();
        // 还原原内容（原来就是空的话不用动）
        if !saved.is_empty() {
            let _ = set_text(&saved);
        }
        Ok(text)
    }
}

#[cfg(windows)]
pub use win::{get_text, selected_text, set_text};

#[cfg(not(windows))]
pub fn get_text() -> anyhow::Result<String> {
    anyhow::bail!("当前平台不支持剪贴板")
}
#[cfg(not(windows))]
pub fn set_text(_t: &str) -> anyhow::Result<()> {
    anyhow::bail!("当前平台不支持剪贴板")
}
#[cfg(not(windows))]
pub fn selected_text() -> anyhow::Result<String> {
    anyhow::bail!("当前平台不支持划词")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn set_then_get_round_trip() {
        let saved = get_text().unwrap_or_default();
        let sample = "划词测试 with English\n第二行";
        set_text(sample).unwrap();
        assert_eq!(get_text().unwrap(), sample);
        set_text("").ok();
        assert_eq!(get_text().unwrap(), "");
        if !saved.is_empty() {
            set_text(&saved).ok();
        }
    }
}
