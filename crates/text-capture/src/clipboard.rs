//! 划词：保存剪贴板 → 模拟 Ctrl+C → 读取新内容 → 还原剪贴板
//!
//! **模拟的 Ctrl+C 只会发给前台窗口。** 用户点「译」图标时，简译自己的窗口会被激活成前台，
//! 那一下 Ctrl+C 就发给了简译，有选区的程序根本没收到——剪贴板不变，被判成「没有选中文字」
//! （2026-09-11 Jack 报）。所以从图标触发的取词一律走 [`selected_text_from`]：
//! 先把弹图标那一刻记下的前台窗口切回来、确认切过去了，再发 Ctrl+C。
//! 回归探针：`cargo run -p text-capture --example focus_probe`。
//!
//! 已知限制：还原只还原 Unicode 文本。原剪贴板里是图片或文件时，划词后会丢失。

#[cfg(windows)]
mod win {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HANDLE, HWND};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL,
    };
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
    };

    const CF_UNICODETEXT: u32 = 13;

    /// 打开剪贴板的重试退避（毫秒），总计约 1.9 秒。
    ///
    /// 早先是 10 次线性退避共 550 ms，实测扛不住 Windows 的**剪贴板历史服务**（`cbdhsvc_*`）：
    /// 它用 `AddClipboardFormatListener` 持有会话锁，而这种持有者**不会**出现在
    /// `GetOpenClipboardWindow()` 里——于是现象是「`OpenClipboard` 一直失败，但查不到谁占着」，
    /// 很容易误判成自己的代码有问题。开着「剪贴板历史」（Win+V）的机器上尤其容易撞。
    ///
    /// 前几档密是为了让常见的瞬时争抢尽快通过（划词是交互路径，不能让用户干等）；
    /// 后几档拉到秒级才有意义——继续加密集重试只是空转，退避拉长才可能等到锁释放。
    const OPEN_BACKOFF_MS: &[u64] = &[5, 10, 20, 40, 80, 150, 300, 500, 800];

    struct Clip;
    impl Clip {
        /// 打开剪贴板；别的进程（含系统的剪贴板历史服务）可能正占着，按退避重试
        fn open() -> anyhow::Result<Clip> {
            let try_once = || unsafe { OpenClipboard(std::ptr::null_mut::<std::ffi::c_void>() as HWND) } != 0;
            if try_once() {
                return Ok(Clip);
            }
            for ms in OPEN_BACKOFF_MS {
                std::thread::sleep(Duration::from_millis(*ms));
                if try_once() {
                    return Ok(Clip);
                }
            }
            // **`ERROR_ACCESS_DENIED`(5) 与「被别人占着」是两回事**，混着报会把排障带偏：
            // 前者是这个进程压根没被允许开剪贴板（安全软件的剪贴板管控、UIPI 完整性级别、
            // 非前台进程受限），重试多久都没用；后者才是争抢，退避重试有意义。
            let err = unsafe { GetLastError() };
            let waited = OPEN_BACKOFF_MS.iter().sum::<u64>();
            if err == 5 {
                anyhow::bail!(
                    "打不开剪贴板：系统拒绝访问（错误码 5）。                     这不是被别的程序占着——多半是安全软件的剪贴板管控，或本进程完整性级别不够。                     重试无用，已等了 {waited} 毫秒。"
                );
            }
            anyhow::bail!(
                "打不开剪贴板（错误码 {err}，等了 {waited} 毫秒仍不放行）。                 常见成因是 Windows 的剪贴板历史服务；查不到占用进程是正常的，                 用 NULL 句柄开的剪贴板不会出现在 GetOpenClipboardWindow 里。                 系统级卡死时 PowerShell 的 Set-Clipboard 也会失败，重启 explorer.exe 可恢复。"
            )
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

    /// 剪贴板序号：每次有程序写剪贴板就加一，用来判断「这段时间里有没有人复制过」
    pub fn sequence_number() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    /// 当前前台窗口（不管属于哪个进程）
    pub fn foreground_window() -> isize {
        unsafe { GetForegroundWindow() as isize }
    }

    /// 窗口类名（取不到返回空串）
    pub fn class_name(hwnd: isize) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetClassNameW(hwnd as HWND, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// 取当前选中的文本。整个过程保存并还原原剪贴板内容。
    /// 当前前台窗口，**不属于本进程时**才返回。
    ///
    /// 弹划词图标那一刻调用：这时前台就是用户刚拖选文字的程序。
    /// 属于本进程（比如用户在简译自己的卡片上拖了一下）就返回 None，别把自己记成取词目标。
    pub fn foreign_foreground_window() -> Option<isize> {
        unsafe {
            let h = GetForegroundWindow();
            if h.is_null() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            (pid != std::process::id()).then_some(h as isize)
        }
    }

    /// 把指定窗口切回前台，并确认真的切过去了。
    ///
    /// 用户刚点了简译的图标，简译此刻是前台，有权把前台交出去；
    /// 仍然先把输入队列挂到前台线程与目标线程上——没有前台权限时 SetForegroundWindow 会被系统忽略
    /// （托盘弹菜单用的是同一招）。
    pub fn bring_to_front(target: isize) -> bool {
        unsafe {
            if GetForegroundWindow() as isize == target {
                return true;
            }
            let me = GetCurrentThreadId();
            let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
            let target_thread = GetWindowThreadProcessId(target as HWND, std::ptr::null_mut());
            AttachThreadInput(me, fg_thread, 1);
            AttachThreadInput(me, target_thread, 1);
            BringWindowToTop(target as HWND);
            SetForegroundWindow(target as HWND);
            AttachThreadInput(me, target_thread, 0);
            AttachThreadInput(me, fg_thread, 0);
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if unsafe { GetForegroundWindow() } as isize == target {
                return true;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        false
    }

    /// 从指定窗口取选中的文字：先切回它，再模拟 Ctrl+C。`target` 为 None 就用当前前台（热键触发时）。
    ///
    /// 切不回去就报错，不假装「没有选中文字」——那样用户会以为是自己没选好，反复重选。
    pub fn selected_text_from(target: Option<isize>) -> anyhow::Result<String> {
        if let Some(t) = target {
            if unsafe { IsWindow(t as HWND) } == 0 {
                anyhow::bail!("选中文字的那个窗口已经关了，请重新选中再试");
            }
            let before = unsafe { GetForegroundWindow() } as isize;
            let t0 = Instant::now();
            let ok = bring_to_front(t);
            tracing::info!(
                "划词切回目标窗口 {t:#x}：{}，用时 {} ms（切之前前台 {before:#x}{}）",
                if ok { "成功" } else { "失败" },
                t0.elapsed().as_millis(),
                if before == t { "，本来就是它" } else { "" }
            );
            if !ok {
                anyhow::bail!("没能切回选中文字的那个窗口，请先点一下那个窗口再重新选中");
            }
        }
        selected_text()
    }

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
            // 没有选中内容、目标程序不支持复制，或者 Ctrl+C 根本没发到有选区的窗口。
            // 记下前台窗口的进程：「前台是简译自己」一眼就能看出是焦点被抢走了
            let fg = unsafe { GetForegroundWindow() };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(fg, &mut pid) };
            tracing::info!(
                "模拟 Ctrl+C 后剪贴板没有变化：前台窗口 {:?} 属于进程 {pid}（本进程 {}）",
                fg,
                std::process::id()
            );
            return Ok(String::new());
        }
        // 读失败别吞掉：剪贴板被拒绝访问时也会表现成「取到空文本」，不留痕迹就查不到
        let text = match get_text() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("剪贴板已更新，但读取失败：{e}");
                String::new()
            }
        };
        tracing::info!("模拟 Ctrl+C 后剪贴板已更新，取到 {} 字", text.chars().count());
        // 还原原内容（原来就是空的话不用动）
        if !saved.is_empty() {
            let _ = set_text(&saved);
        }
        Ok(text)
    }
}

#[cfg(windows)]
pub use win::{class_name, foreground_window, sequence_number, foreign_foreground_window, get_text, selected_text, selected_text_from, set_text, bring_to_front};

#[cfg(not(windows))]
pub fn class_name(_hwnd: isize) -> String {
    String::new()
}

#[cfg(not(windows))]
pub fn foreground_window() -> isize {
    0
}

#[cfg(not(windows))]
pub fn sequence_number() -> u32 {
    0
}

/// 这个窗口类是不是终端。
///
/// **对终端绝不能模拟 Ctrl+C**：终端里没有原生选区时，Ctrl+C 就是「中断」信号，
/// 会打断里面正在跑的程序（2026-09-22 在 Windows Terminal 里跑 Claude Code 时实测复现
/// 「没有选中文字」，同时那一下 Ctrl+C 是发给 Claude Code 的）。终端的选区改走无障碍接口读。
pub fn is_terminal_class(class: &str) -> bool {
    const TERMINALS: [&str; 6] = [
        "CASCADIA_HOSTING_WINDOW_CLASS", // Windows Terminal
        "ConsoleWindowClass",            // 老式控制台 conhost
        "mintty",                        // Git Bash / Cygwin / MSYS2
        "PuTTY",
        "VirtualConsoleClass", // ConEmu / Cmder
        "org.wezfurlong.wezterm",
    ];
    TERMINALS.contains(&class)
}

/// 这个窗口类是不是 Chromium 系浏览器。
///
/// Chrome / Edge / Brave / Vivaldi / Opera / Arc 都是这一个类名（Firefox 是另一个）。
///
/// **浏览器里不能靠模拟 Ctrl+C 取选区**：2026-10-22 Jack 实测，同一个 Chrome 窗口连着两次
/// 「模拟 Ctrl+C 后剪贴板已更新，取到 0 字」——剪贴板序号变了（说明复制确实发生了）
/// 但内容是空串，划词因此没取到；同一个窗口里悬停取词（也走无障碍接口）却能读到选区。
/// 浏览器改走无障碍接口读选区，读不到再退回 Ctrl+C。
pub fn is_browser_class(class: &str) -> bool {
    class == "Chrome_WidgetWin_1"
}

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
#[cfg(not(windows))]
pub fn selected_text_from(_target: Option<isize>) -> anyhow::Result<String> {
    anyhow::bail!("当前平台不支持划词")
}
#[cfg(not(windows))]
pub fn foreign_foreground_window() -> Option<isize> {
    None
}
#[cfg(not(windows))]
pub fn bring_to_front(_target: isize) -> bool {
    false
}

#[cfg(test)]
mod terminal_tests {
    use super::*;

    /// 终端清单漏一个，那个终端里点「译」就会被发一记 Ctrl+C（= 中断里面跑着的程序）
    #[test]
    fn known_terminals_are_recognised() {
        for c in ["CASCADIA_HOSTING_WINDOW_CLASS", "ConsoleWindowClass", "mintty", "PuTTY", "VirtualConsoleClass"] {
            assert!(is_terminal_class(c), "{c}");
        }
        for c in ["Notepad", "Chrome_WidgetWin_1", "", "cascadia_hosting_window_class"] {
            assert!(!is_terminal_class(c), "{c}");
        }
    }

    /// 浏览器要单独认：它的选区走无障碍接口读，不能发 Ctrl+C
    #[test]
    fn chromium_browsers_are_recognised() {
        for c in ["Chrome_WidgetWin_1"] {
            assert!(is_browser_class(c), "{c}");
        }
        for c in ["Notepad", "CASCADIA_HOSTING_WINDOW_CLASS", "", "chrome_widgetwin_1"] {
            assert!(!is_browser_class(c), "{c}");
        }
    }
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
