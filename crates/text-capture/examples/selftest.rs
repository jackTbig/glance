//! 自动化取词自检：启动记事本打开一段英文 → UIA 取词 + OCR 取词 → 关掉记事本
//! 用法：cargo run -p text-capture --example selftest
use std::process::Command;
use std::time::Duration;
use text_capture::{Capturer, Source};

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

/// 前置窗口。进程没有前台权限时 SetForegroundWindow 会被系统忽略，
/// 先把本线程附到当前前台线程的输入队列上就能绕开（live-caption 托盘菜单用的同一招）。
#[cfg(windows)]
fn force_foreground(hwnd: isize) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        ShowWindow, SW_RESTORE,
    };
    unsafe {
        let me = GetCurrentThreadId();
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
        // 真正压到最上层：只 SetForegroundWindow 有时仍被别的置顶窗口盖住
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST, HWND_NOTOPMOST, SWP_NOMOVE, SWP_NOSIZE};
        SetWindowPos(hwnd as _, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        SetWindowPos(hwnd as _, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        SetWindowPos(hwnd as _, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        AttachThreadInput(me, target_thread, 0);
        AttachThreadInput(me, fg_thread, 0);
    }
}

fn main() -> anyhow::Result<()> {
    let note = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/selftest.txt");
    std::fs::write(
        &note,
        "Rust is a systems programming language that runs blazingly fast.\r\nIt prevents segfaults and guarantees thread safety.\r\nKubernetes is an open source container orchestration system.\r\n",
    )?;
    println!("启动记事本…");
    let mut child = Command::new("notepad.exe").arg(&note).spawn()?;
    std::thread::sleep(Duration::from_millis(1500));

    let (rect, hwnd) = find_window("selftest").ok_or_else(|| anyhow::anyhow!("找不到记事本窗口"))?;
    println!("记事本窗口 {rect:?}");
    force_foreground(hwnd);
    std::thread::sleep(Duration::from_millis(800));
    // 编辑区大致在标题栏与菜单下方
    let x = (rect.0 + rect.2) / 2;
    // Windows 11 记事本：标题栏 + 标签栏 + 菜单栏约占 180 px，正文从那以下开始
    let y = rect.1 + 200;

    let c = Capturer::new();
    println!("进程 {:?}", c.process_at(x, y));

    let r = c.at_point(x, y)?;
    println!("[{}] {}ms\n  {}", r.source.label_zh(), r.took_ms, r.text.replace('\n', "\n  "));
    let uia_ok = r.source == Source::Uia && r.text.to_lowercase().contains("rust");

    // 强制走一次 OCR 对比：区域限定在记事本编辑区内部，别跨到别的窗口上
    let (rx, ry) = (rect.0 + 20, y - 5);
    let (rw, rh) = ((rect.2 - rect.0 - 40).max(100) as u32, 100u32);
    println!("识别区域 ({rx},{ry}) {rw}x{rh}");
    let o = c.region(rx, ry, rw, rh)?;
    println!("[强制文字识别] {}ms\n  {}", o.took_ms, o.text.replace('\n', "\n  "));
    let ocr_ok = o.text.to_lowercase().contains("rust");

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&note);

    println!("\n无障碍接口取词: {}", if uia_ok { "通过" } else { "未取到（会退到文字识别）" });
    println!("文字识别取词:   {}", if ocr_ok { "通过" } else { "未识别出预期文字" });
    anyhow::ensure!(uia_ok || ocr_ok, "两种取词都失败");
    Ok(())
}
