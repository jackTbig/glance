//! 悬停翻译整条链的自动化探针（不开界面）：
//!
//! 起记事本打开一段英文 → 把鼠标挪到正文上 → 合成按下 Alt →
//! 用**真实**的 `GetAsyncKeyState` / `GetCursorPos` 喂 `WatchState` →
//! 看有没有吐出 `Trigger::Hovered` → 再走一遍例外进程判定与 UIA 取词。
//!
//! 「按住 Alt 悬停没反应」可能断在四个地方，这个探针能把前三个区分开：
//!   1. 修饰键读不到    2. 状态机不触发    3. 取词落空 / 命中例外进程
//!   4. 卡片弹出来了但因为 DPI 换算错跑到屏幕外（这条得靠日志和肉眼，探针管不到）
//!
//! 用法：
//!   cargo run -p translate-tool --example hover_probe          # 只验链路，不需要桌面端在跑
//!   cargo run -p translate-tool --example hover_probe -- live  # 桌面端在跑时驱动一次真悬停并截屏存证
//!
//! 注意：会短暂接管鼠标位置与 Alt 键，跑的时候别抢鼠标。
//!
//! **探针必须先声明 DPI 感知**：`cargo run` 起的进程默认是 DPI-unaware，
//! Win32 会给它虚拟化过的逻辑坐标（本机 1536x960），而 translate-tool.exe 是
//! PER_MONITOR_AWARE、拿到的是物理坐标（3072x1920）。不声明的话探针与被测应用
//! 根本不在同一个坐标系里，量出来的数没有意义。

/// 与 translate-tool.exe 对齐到同一个坐标系：PER_MONITOR_AWARE_V2
#[cfg(windows)]
fn become_dpi_aware() {
    unsafe {
        windows_link::link!("user32.dll" "system" fn SetProcessDpiAwarenessContext(value: isize) -> i32);
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
        SetProcessDpiAwarenessContext(-4);
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use std::process::Command;
    use std::time::{Duration, Instant};
    use text_capture::Capturer;
    use translate_tool::hotkey::HoverModifier;
    use translate_tool::watcher::{Trigger, WatchConfig, WatchState};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, KEYEVENTF_KEYUP, VK_MENU,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

    become_dpi_aware();
    let live = std::env::args().any(|a| a == "live");
    let note = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hover_probe.txt");
    std::fs::write(&note, "Rust is a systems programming language that runs blazingly fast.\r\nIt prevents segfaults and guarantees thread safety.\r\n")?;
    println!("启动记事本…");
    let mut child = Command::new("notepad.exe").arg(&note).spawn()?;
    std::thread::sleep(Duration::from_millis(1800));

    let (rect, hwnd) = find_window("hover_probe").ok_or_else(|| anyhow::anyhow!("找不到记事本窗口"))?;
    force_foreground(hwnd);
    std::thread::sleep(Duration::from_millis(600));
    let (px, py) = ((rect.0 + rect.2) / 2, rect.1 + 200);
    println!("记事本窗口 {rect:?}，取词点物理({px},{py})");
    println!("虚拟屏 {:?}", text_capture::screen::virtual_screen());

    // 1) 鼠标挪过去，2) 按住 Alt
    unsafe { SetCursorPos(px, py) };
    std::thread::sleep(Duration::from_millis(120));
    println!("光标现在在 {:?}", text_capture::screen::cursor_pos());
    unsafe { keybd_event(VK_MENU as u8, 0, 0, 0) };
    std::thread::sleep(Duration::from_millis(60));

    let mod_down = HoverModifier::Alt.is_down();
    println!("① 修饰键读数：Alt {}", if mod_down { "按下（正常）" } else { "没读到（这就是根因）" });

    if live {
        // 桌面端自己在轮询鼠标与 Alt，这里只要按住不动，等它把译文卡片弹出来，然后整屏存证
        println!("live：按住 Alt 等桌面端弹卡片…");
        std::thread::sleep(std::time::Duration::from_millis(3500));
        let (vx, vy, vw, vh) = text_capture::screen::virtual_screen();
        let shot = text_capture::screen::capture_rect(vx, vy, vw, vh)?;
        unsafe { keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0) };
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../hover_live.png");
        shot.save(&out)?;
        println!("整屏已存到 {}", out.display());
        println!("取词点物理({px},{py})：卡片应当紧挨这个点，不该跑到两倍远处");
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&note);
        return Ok(());
    }

    // 3) 用真实输入喂状态机，最多等 1.5 秒
    let cfg = WatchConfig { hover_modifier: HoverModifier::Alt, hover_delay: Duration::from_millis(300), ..Default::default() };
    let mut st = WatchState::new(cfg);
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut fired: Option<Trigger> = None;
    let mut samples: Vec<(i32, i32, bool)> = Vec::new();
    while Instant::now() < deadline {
        let (x, y) = text_capture::screen::cursor_pos();
        let md = HoverModifier::Alt.is_down();
        samples.push((x, y, md));
        if let Some(t) = st.step(x, y, false, md, translate_tool::watcher::CursorKind::Unknown, Instant::now()) {
            fired = Some(t);
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    println!("   采样 {} 次，前 8 条 {:?}", samples.len(), &samples[..samples.len().min(8)]);
    unsafe { keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0) };
    println!("② 状态机：{}", match &fired {
        Some(t) => format!("触发 {t:?}"),
        None => "1.5 秒内没触发（状态机的问题）".into(),
    });

    // 4) 例外进程判定 + 取词
    let c = Capturer::new();
    let proc = c.process_at(px, py);
    let excluded = ["code.exe", "devenv.exe", "windowsterminal.exe"];
    let is_excluded = proc.as_deref().map(|p| excluded.contains(&p)).unwrap_or(false);
    println!("③ 该点进程：{proc:?}{}", if is_excluded { "  ← 在默认例外列表里，悬停会被静默吞掉" } else { "" });

    let cap = c.at_point(px, py)?;
    println!("④ 取词：[{}] {}ms\n   {}", cap.source.label_zh(), cap.took_ms, cap.text.replace('\n', "\n   "));

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&note);

    let ok = mod_down && fired.is_some() && !cap.is_empty();
    println!("\n结论：悬停链路（修饰键 → 状态机 → 取词）{}", if ok { "全通" } else { "有断点，见上面标号" });
    anyhow::ensure!(ok, "悬停链路有断点");
    Ok(())
}

#[cfg(windows)]
#[path = "common/win.rs"]
mod helpers;
#[cfg(windows)]
use helpers::{find_window, force_foreground};

#[cfg(not(windows))]
fn main() {}
