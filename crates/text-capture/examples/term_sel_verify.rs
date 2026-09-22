//! 终端划词回归：自己开一个 Windows Terminal 窗口，在里面拖选一段字，再走 `Capturer::selection`。
//!
//! 验的是「终端不发 Ctrl+C、改读无障碍接口选区」那条路。**会接管鼠标约 3 秒**，
//! 开跑前自己等键鼠静止 60 秒（有人在用机器就退出），只动自己开的那个窗口，跑完关掉它。

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetLastInputInfo, LASTINPUTINFO,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    unsafe {
        // 与 translate-tool.exe 一样按物理像素算坐标
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(-4isize as _);
        let mut li = LASTINPUTINFO { cbSize: 8, dwTime: 0 };
        GetLastInputInfo(&mut li);
        let idle = windows_sys::Win32::System::SystemInformation::GetTickCount().wrapping_sub(li.dwTime);
        if idle < 60_000 {
            anyhow::bail!("键鼠 {idle} ms 前还有操作，有人在用机器，不跑");
        }
    }

    const TITLE: &str = "GlanceTermProbe";
    std::process::Command::new("wt.exe")
        .args(["-w", "new", "--title", TITLE, "--suppressApplicationTitle", "cmd", "/k", "echo glance terminal probe"])
        .spawn()?;
    let wide: Vec<u16> = TITLE.encode_utf16().chain(Some(0)).collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let hwnd: HWND = loop {
        let h = unsafe { FindWindowW(std::ptr::null(), wide.as_ptr()) };
        if !h.is_null() {
            break h;
        }
        anyhow::ensure!(Instant::now() < deadline, "10 秒内没等到探针窗口");
        std::thread::sleep(Duration::from_millis(200));
    };
    std::thread::sleep(Duration::from_millis(1500));
    let result = (|| -> anyhow::Result<String> {
        unsafe {
            SetForegroundWindow(hwnd);
            std::thread::sleep(Duration::from_millis(300));
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut r);
            // 第一行文字大约在标题栏下方；从左往右拖过一整行
            let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as i32;
            let y = r.top + 60 * dpi / 96;
            let (x0, x1) = (r.left + 12 * dpi / 96, r.left + 400 * dpi / 96);
            windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(x0, y);
            mouse(0x0002); // 左键按下
            for i in 1..=20 {
                SetCursorPos(x0 + (x1 - x0) * i / 20, y);
                std::thread::sleep(Duration::from_millis(15));
            }
            mouse(0x0004); // 左键松开
            std::thread::sleep(Duration::from_millis(300));
        }
        let before = text_capture::clipboard::get_text().unwrap_or_default();
        let cap = text_capture::Capturer::new();
        let got = cap.selection(Some(hwnd as isize))?;
        let after = text_capture::clipboard::get_text().unwrap_or_default();
        anyhow::ensure!(before == after, "剪贴板被动过了——终端这条路不该碰剪贴板");
        Ok(got.text)
    })();
    unsafe {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
    match result {
        Ok(t) => {
            println!("取到：{t:?}");
            anyhow::ensure!(t.contains("glance terminal probe"), "取到的不是探针那行字");
            println!("通过");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[cfg(windows)]
unsafe fn mouse(flags: u32) {
    windows_sys::Win32::UI::Input::KeyboardAndMouse::mouse_event(flags, 0, 0, 0, 0);
}

#[cfg(not(windows))]
fn main() {}
