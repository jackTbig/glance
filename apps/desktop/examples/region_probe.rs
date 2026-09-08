//! 截屏框选（Ctrl+Alt+G）的自动化探针：合成热键 + 合成鼠标拖拽，框住记事本里的英文，
//! 然后整屏存证。桌面端必须已经在跑。
//!
//! 桌面端的框选与译文面板是纯 GUI 交互，自动化测试盖不到，只能这样「假装用户」跑一遍，
//! 再拿日志（矩形是不是物理坐标、识别出多少字、面板摆到哪）和截图对账。
//!
//! 用法：
//!   cargo run -p translate-tool --example region_probe         # 截屏框选（Ctrl+Alt+G）
//!   cargo run -p translate-tool --example region_probe -- icon # 只拖选文字，验划词小图标的位置
//! 注意：会接管鼠标与键盘几秒，跑的时候别抢。

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use std::process::Command;
    use std::time::Duration;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, mouse_event, KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, VK_CONTROL, VK_MENU,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

    unsafe {
        windows_link::link!("user32.dll" "system" fn SetProcessDpiAwarenessContext(value: isize) -> i32);
        SetProcessDpiAwarenessContext(-4); // PER_MONITOR_AWARE_V2，与 translate-tool.exe 同一坐标系
    }
    anyhow::ensure!(
        std::net::TcpStream::connect("127.0.0.1:42731").is_ok(),
        "桌面端没在跑（127.0.0.1:42731 连不上）"
    );

    let note = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("region_probe.txt");
    std::fs::write(&note, "Kubernetes is an open source container orchestration system.\r\n")?;
    let mut child = Command::new("notepad.exe").arg(&note).spawn()?;
    std::thread::sleep(Duration::from_millis(1800));
    let (rect, hwnd) = find_window("region_probe").ok_or_else(|| anyhow::anyhow!("找不到记事本窗口"))?;
    force_foreground(hwnd);
    std::thread::sleep(Duration::from_millis(600));
    println!("记事本窗口 {rect:?}  虚拟屏 {:?}", text_capture::screen::virtual_screen());

    // 正文那一行大致的物理矩形
    let (x1, y1) = (rect.0 + 40, rect.1 + 175);
    let (x2, y2) = (rect.0 + 1200, rect.1 + 235);
    println!("准备框选物理({x1},{y1}) → ({x2},{y2})");

    let icon_only = std::env::args().any(|a| a == "icon");
    // Ctrl+Alt+G
    if !icon_only {
    unsafe {
        keybd_event(VK_CONTROL as u8, 0, 0, 0);
        keybd_event(VK_MENU as u8, 0, 0, 0);
        keybd_event(b'G', 0, 0, 0);
        std::thread::sleep(Duration::from_millis(60));
        keybd_event(b'G', 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_CONTROL as u8, 0, KEYEVENTF_KEYUP, 0);
    }
    std::thread::sleep(Duration::from_millis(500));
    }

    // 拖一个框：分步移动，让 30 fps 的界面每帧都能看到位移
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
    if icon_only {
        // 小图标默认 2.5 秒消失，抓在中间
        println!("已松开鼠标，等划词小图标…");
        std::thread::sleep(Duration::from_millis(700));
        let shot = text_capture::screen::capture_rect(x2 - 200, y2 - 120, 500, 320)?;
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../icon_live.png");
        shot.save(&out)?;
        println!("松手点物理({x2},{y2})，小图标应贴着它；局部截图存到 {}", out.display());
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&note);
        return Ok(());
    }

    println!("已松开鼠标，等识别与翻译…");
    // 把鼠标挪进面板里，不然译文一出来面板就因为「鼠标在外面 1.2 秒」自动关了，截图什么也拍不到
    std::thread::sleep(Duration::from_millis(600));
    unsafe { SetCursorPos(x1 + 400, y2 + 400) };
    std::thread::sleep(Duration::from_millis(3500));

    // 对账：探针自己按同一个物理矩形抓一次并识别，用来分辨
    //「桌面端抓错了地方」和「这块图本来就识别不出字」
    {
        let c = text_capture::Capturer::new();
        let strip = text_capture::screen::capture_rect(x1, y1, (x2 - x1) as u32, (y2 - y1) as u32)?;
        let sp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../region_strip.png");
        strip.save(&sp)?;
        match c.region(x1, y1, (x2 - x1) as u32, (y2 - y1) as u32) {
            Ok(r) => println!("对账识别（{}x{}）：{:?}", x2 - x1, y2 - y1, r.text),
            Err(e) => println!("对账识别失败：{e}"),
        }
        println!("同一块图已存到 {}", sp.display());
    }

    let (vx, vy, vw, vh) = text_capture::screen::virtual_screen();
    let shot = text_capture::screen::capture_rect(vx, vy, vw, vh)?;
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../region_live.png");
    shot.save(&out)?;
    println!("整屏已存到 {}", out.display());

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&note);
    Ok(())
}

#[cfg(windows)]
#[path = "common/win.rs"]
mod helpers;
#[cfg(windows)]
use helpers::{find_window, force_foreground};

#[cfg(not(windows))]
fn main() {}
