//! 验证新判据「只有划过文字才弹划词图标」。
//!
//! ① 光标形状判定对不对：自己把光标设成 I 形 / 箭头，看 `cursor_shape()` 认不认得
//!    （这一步不需要解锁屏幕）
//! ② 真拖一遍：在记事本文字上拖 → over_text 应为 true；在桌面空白处拖 → 应为 false
//!    （屏幕锁着就跳过）
//!
//! 用法：cargo run -p translate-tool --example sel_icon_probe
//! 注意：会接管鼠标几秒，跑之前确认人不在用机器（脚本自己会查）。

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use std::time::Duration;
    use translate_tool::watcher::{Trigger, WatchConfig, WatchState};
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

    // 有人在用机器就别抢鼠标
    let idle = idle_seconds();
    println!("人上一次操作距今 {idle:.0} 秒");
    anyhow::ensure!(idle > 5.0, "检测到人刚动过鼠标，别抢");

    // ① 自检：光标形状判定
    self_check();

    if screen_locked() {
        println!("屏幕锁着，跳过真拖测试（解锁后再跑一次）");
        return Ok(());
    }

    // ② 真拖
    use std::process::Command;
    let note = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tt_probe_sel_icon.txt");
    std::fs::write(&note, "Kubernetes is an open source container orchestration system.\r\n")?;
    let _child = Command::new("notepad.exe").arg(&note).spawn()?;
    std::thread::sleep(Duration::from_millis(2000));
    let (rect, hwnd) = find_window("tt_probe_sel_icon").ok_or_else(|| anyhow::anyhow!("找不到记事本窗口"))?;
    force_foreground(hwnd);
    std::thread::sleep(Duration::from_millis(600));
    println!("记事本窗口 {rect:?}");

    let mut old = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut old) };

    // 状态机喂真实输入
    let (tx_s, rx_s) = std::sync::mpsc::channel::<Trigger>();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::spawn(move || {
        let mut st = WatchState::new(WatchConfig::default());
        while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
            let (x, y) = text_capture::screen::cursor_pos();
            let down = unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0 };
            let over = text_capture::screen::cursor_shape() == text_capture::screen::CursorShape::IBeam;
            if let Some(t) = st.step(x, y, down, false, over, std::time::Instant::now()) {
                let _ = tx_s.send(t);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    // 正文第一行（Windows 11 记事本：标题栏+标签栏+菜单栏约 180 px，200% 缩放则再多一倍）
    let (tx, ty) = (rect.0 + 60, rect.1 + 200);
    let (dx, dy) = (rect.2 + 120, rect.3 + 120);
    for (name, (x, y)) in [("正文", (tx, ty)), ("桌面", (dx, dy))] {
        unsafe { SetCursorPos(x, y) };
        std::thread::sleep(Duration::from_millis(250));
        println!("  {name} ({x},{y})：光标形状 {:?}", text_capture::screen::cursor_shape());
    }

    println!("\n① 在正文上拖选：");
    drag(tx, ty, tx + 320, ty);
    println!("   触发：{:?}", rx_s.recv_timeout(Duration::from_millis(800)).ok());

    println!("\n② 在桌面空白处拖：");
    drag(dx, dy, dx + 200, dy);
    println!("   触发：{:?}", rx_s.recv_timeout(Duration::from_millis(800)).ok());

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    unsafe { SetCursorPos(old.x, old.y) };
    // 关掉探针开的记事本（Windows 11 的记事本是应用包，杀启动进程关不掉窗口）
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd as _, 0x0010, 0, 0); // WM_CLOSE
    }
    let _ = std::fs::remove_file(&note);
    Ok(())
}

/// 自检：自己把光标设成 I 形与箭头，看判定认不认得
#[cfg(windows)]
fn self_check() {
    use std::time::Duration;
    use windows_sys::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_ARROW, IDC_IBEAM};
    println!("\n光标形状自检：");
    unsafe {
        SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_IBEAM));
        std::thread::sleep(Duration::from_millis(150));
        println!("  设成 I 形 → {:?}", text_capture::screen::cursor_shape());
        SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_ARROW));
        std::thread::sleep(Duration::from_millis(150));
        println!("  设成箭头 → {:?}", text_capture::screen::cursor_shape());
    }
}

/// 屏幕锁着吗（锁了就没有可见窗口可试）
///
/// 锁屏时前台窗口是系统的「Windows.UI.Core.CoreWindow」，用它来认。
#[cfg(windows)]
fn screen_locked() -> bool {
    let Some(hwnd) = text_capture::clipboard::foreign_foreground_window() else {
        println!("没有前台窗口");
        return false;
    };
    let class = text_capture::clipboard::class_name(hwnd as isize);
    let locked = class.contains("Windows.UI.Core.CoreWindow");
    println!("前台窗口类 {class:?}（锁着={locked}）");
    locked
}

/// 按下 → 分步移动 → 松开
#[cfg(windows)]
fn drag(x1: i32, y1: i32, x2: i32, y2: i32) {
    use std::time::Duration;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{mouse_event, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP};
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;
    unsafe {
        SetCursorPos(x1, y1);
        std::thread::sleep(Duration::from_millis(150));
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        for i in 1..=12 {
            SetCursorPos(x1 + (x2 - x1) * i / 12, y1 + (y2 - y1) * i / 12);
            std::thread::sleep(Duration::from_millis(45));
        }
        std::thread::sleep(Duration::from_millis(120));
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
}

/// 距上一次键鼠操作过了多少秒
#[cfg(windows)]
fn idle_seconds() -> f64 {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    unsafe {
        let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut li) == 0 {
            return 0.0;
        }
        (GetTickCount().saturating_sub(li.dwTime)) as f64 / 1000.0
    }
}

#[cfg(windows)]
fn find_window(title_part: &str) -> Option<((i32, i32, i32, i32), isize)> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowRect, GetWindowTextW, IsWindowVisible};
    static mut TARGET: *const str = "" as *const str;
    static mut FOUND: Option<((i32, i32, i32, i32), isize)> = None;
    unsafe extern "system" fn cb(hwnd: HWND, _l: LPARAM) -> i32 {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut buf = [0u16; 512];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n <= 0 {
                return 1;
            }
            let t = String::from_utf16_lossy(&buf[..n as usize]);
            if t.to_lowercase().contains(&(*TARGET).to_lowercase()) {
                let mut r: RECT = std::mem::zeroed();
                if GetWindowRect(hwnd, &mut r) != 0 {
                    FOUND = Some(((r.left, r.top, r.right, r.bottom), hwnd as isize));
                    return 0;
                }
            }
            1
        }
    }
    unsafe {
        TARGET = title_part as *const str;
        FOUND = None;
        EnumWindows(Some(cb), 0);
        FOUND
    }
}

/// 前置窗口（抄 selftest）
#[cfg(windows)]
fn force_foreground(hwnd: isize) {
    use windows_sys::Win32::System::Threading::AttachThreadInput;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };
    unsafe {
        let me = windows_sys::Win32::System::Threading::GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let target_thread = GetWindowThreadProcessId(hwnd as _, std::ptr::null_mut());
        AttachThreadInput(me, fg_thread, 1);
        AttachThreadInput(me, target_thread, 1);
        ShowWindow(hwnd as _, SW_RESTORE);
        BringWindowToTop(hwnd as _);
        SetForegroundWindow(hwnd as _);
        SetActiveWindow(hwnd as _);
        SetFocus(hwnd as _);
        AttachThreadInput(me, target_thread, 0);
        AttachThreadInput(me, fg_thread, 0);
    }
}

#[cfg(not(windows))]
fn main() {
    println!("只在 Windows 上跑");
}
