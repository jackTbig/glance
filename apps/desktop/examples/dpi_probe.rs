//! 探测 DPI 相关的坐标口径：鼠标、虚拟屏、进程 DPI 感知级别。
//! 三个「位置漂移」问题都指向同一个怀疑：Win32 给的是物理像素，egui 的窗口定位要逻辑像素。
#[cfg(windows)]
fn main() {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN};

    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        println!("GetCursorPos            = ({}, {})", p.x, p.y);
        println!("SM_CXSCREEN             = {}", GetSystemMetrics(SM_CXSCREEN));
        println!("SM_CXVIRTUALSCREEN      = {}", GetSystemMetrics(SM_CXVIRTUALSCREEN));
        println!("SM_CYVIRTUALSCREEN      = {}", GetSystemMetrics(SM_CYVIRTUALSCREEN));
    }
    // 系统 DPI（96 = 100%，120 = 125%，144 = 150%）
    windows_sys::core::s!("");
    unsafe {
        windows_link::link!("user32.dll" "system" fn GetDpiForSystem() -> u32);
        windows_link::link!("user32.dll" "system" fn GetThreadDpiAwarenessContext() -> isize);
        windows_link::link!("user32.dll" "system" fn GetAwarenessFromDpiAwarenessContext(value: isize) -> i32);
        let dpi = GetDpiForSystem();
        println!("GetDpiForSystem         = {dpi}  （缩放 {}%）", dpi * 100 / 96);
        let ctx = GetThreadDpiAwarenessContext();
        let aware = GetAwarenessFromDpiAwarenessContext(ctx);
        let name = match aware {
            0 => "UNAWARE（系统会替你缩放，坐标是逻辑像素）",
            1 => "SYSTEM_AWARE（坐标是物理像素）",
            2 => "PER_MONITOR_AWARE（坐标是物理像素）",
            3 => "PER_MONITOR_AWARE_V2（坐标是物理像素）",
            _ => "未知",
        };
        println!("进程 DPI 感知           = {aware} {name}");
    }
    println!();
    println!("判据：缩放不是 100% 且进程是 aware 的话，Win32 坐标（物理）与 egui 窗口定位（逻辑）差一个缩放比，位置就会漂。");
}

#[cfg(not(windows))]
fn main() {}
