//! hover_probe / region_probe 共用的窗口助手（找窗口、抢前台）。
//! 放在 examples 下当模块引用，不进 lib：这些是探针专用的粗暴做法，不该给正式代码用。
#![allow(dead_code)]

#[cfg(windows)]
pub fn find_window(title_part: &str) -> Option<((i32, i32, i32, i32), isize)> {
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

/// 前置窗口：没有前台权限时 SetForegroundWindow 会被系统忽略，先挂到当前前台线程的输入队列上
#[cfg(windows)]
pub fn force_foreground(hwnd: isize) {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow, SetWindowPos, ShowWindow,
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE,
    };
    unsafe {
        let me = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let target = GetWindowThreadProcessId(hwnd as _, std::ptr::null_mut());
        AttachThreadInput(me, fg_thread, 1);
        AttachThreadInput(me, target, 1);
        ShowWindow(hwnd as _, SW_RESTORE);
        BringWindowToTop(hwnd as _);
        SetForegroundWindow(hwnd as _);
        SetActiveWindow(hwnd as _);
        SetFocus(hwnd as _);
        SetWindowPos(hwnd as _, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        SetWindowPos(hwnd as _, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        AttachThreadInput(me, target, 0);
        AttachThreadInput(me, fg_thread, 0);
    }
}

