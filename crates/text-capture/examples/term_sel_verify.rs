//! 终端划词回归：自己开一个 Windows Terminal 窗口，选中里面的一行字，再走 `Capturer::selection`。
//!
//! 选区用无障碍接口的 `TextRange::Select` 造，**不动鼠标键盘**（估坐标拖选容易落在字外面，
//! 被测的是「读选区」而不是「怎么选」）。但新开的窗口会抢前台，所以开跑前仍等键鼠静止 60 秒，
//! 有人在用机器就退出；跑完关掉自己开的窗口。

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};
    use uiautomation::patterns::UITextPattern;
    use uiautomation::types::{TreeScope, UIProperty};
    use uiautomation::variants::Variant;
    use uiautomation::UIAutomation;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    unsafe {
        let mut li = LASTINPUTINFO { cbSize: 8, dwTime: 0 };
        GetLastInputInfo(&mut li);
        let idle = windows_sys::Win32::System::SystemInformation::GetTickCount().wrapping_sub(li.dwTime);
        if idle < 60_000 {
            anyhow::bail!("键鼠 {idle} ms 前还有操作，有人在用机器，不跑");
        }
    }

    const TITLE: &str = "GlanceTermProbe";
    const LINE: &str = "glance terminal probe";
    // wt.exe 是应用执行别名，不一定在 PATH 里（从 Git Bash 起就找不到），按固定位置找
    let wt = std::path::PathBuf::from(std::env::var("LOCALAPPDATA")?).join(r"Microsoft\WindowsApps\wt.exe");
    std::process::Command::new(&wt)
        .args(["-w", "new", "--title", TITLE, "--suppressApplicationTitle", "cmd", "/k", &format!("echo {LINE}")])
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

    let result = (|| -> anyhow::Result<()> {
        let auto = UIAutomation::new()?;
        let win = auto.element_from_handle((hwnd as isize).into())?;
        let cond = auto.create_property_condition(UIProperty::ClassName, Variant::from("TermControl"), None)?;
        let term = win.find_first(TreeScope::Descendants, &cond)?;
        let tp = term.get_pattern::<UITextPattern>()?;
        let range = tp.get_document_range()?.find_text(LINE, true, false)?;
        range.select()?;
        std::thread::sleep(Duration::from_millis(200));

        let cap = text_capture::Capturer::new();

        // 1. 有原生选区：读得到，且不碰剪贴板
        let before = text_capture::clipboard::sequence_number();
        let got = cap.selection(Some(hwnd as isize))?;
        println!("有选区时取到：{:?}（来源 {:?}）", got.text, got.source);
        anyhow::ensure!(got.text.contains(LINE), "取到的不是探针那行字");
        anyhow::ensure!(text_capture::clipboard::sequence_number() == before, "剪贴板被动过了——终端这条路不该碰剪贴板");

        // 2. 清掉选区（选一个空范围）：没有剪贴板变化时要明确报错，而不是空文本
        let empty = tp.get_document_range()?.find_text(LINE, true, false)?;
        empty.move_endpoint_by_range(
            uiautomation::types::TextPatternRangeEndpoint::End,
            &empty,
            uiautomation::types::TextPatternRangeEndpoint::Start,
        )?;
        empty.select()?;
        std::thread::sleep(Duration::from_millis(200));
        match cap.selection_since(Some(hwnd as isize), Some(text_capture::clipboard::sequence_number())) {
            Err(e) => println!("无选区时报错：{e}"),
            Ok(c) => anyhow::bail!("无选区时不该取到东西：{:?}", c.text),
        }
        Ok(())
    })();
    unsafe {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
    result?;
    println!("通过");
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
