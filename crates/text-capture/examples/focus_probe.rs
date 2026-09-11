//! 划词取词与前台窗口的回归探针。**不碰用户的任何窗口**。
//!
//! 用法：cargo run -p text-capture --example focus_probe
//!
//! 背景（2026-09-11 Jack 报）：选中文字后点「译」图标，卡片提示「没有选中文字」。
//! 取词靠模拟 Ctrl+C，而 Ctrl+C 只会发给**前台窗口**；点图标会把简译自己激活成前台，
//! 这一下 Ctrl+C 就发给了简译，用户的程序根本没收到复制，剪贴板没变，于是判成「没选中」。
//! 取词自检（selftest）每次都先把记事本切到前台再取，恰好绕开了这个前提，所以一直没抓到。
//!
//! 探针建两个顶层窗口：
//! - EDIT：装着选中的文字，扮演用户正在用的程序
//! - STATIC：扮演抢走前台的「译」图标
//! 情形 E 另起一个子进程（探针自己带 `--host`）来装 EDIT：真实场景里有选区的程序与简译不是同一个进程，
//! 而跨进程切前台受系统的前台锁限制，同进程内能切过去不代表跨进程也能。
//!
//! 发任何按键之前都核实前台确实是探针自己的窗口，不是就中止——
//! 2026-09-11 截安装向导时往前台发回车，落进了 Jack 正在用的资源管理器，不能再犯。
//! 开跑前还要等键鼠静止 3 秒：有人正在用电脑时切前台会把他的输入抢进探针窗口。

#[cfg(windows)]
mod probe {
    use std::io::{BufRead, Write};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, CreateWindowExW, DestroyWindow, DispatchMessageW, GetForegroundWindow, GetMessageW,
        GetWindowThreadProcessId, PostThreadMessageW, SendMessageW, SetForegroundWindow, TranslateMessage, MSG,
        WM_QUIT, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    const EM_SETSEL: u32 = 0x00B1;
    pub const TEXT: &str = "focus probe selection";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub struct Windows {
        pub edit: isize,
        pub stealer: isize,
        thread_id: u32,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl Windows {
        /// 窗口必须建在一个跑着消息循环的线程上：取词时主线程在等剪贴板，
        /// 如果窗口也在主线程，编辑框收不到 Ctrl+C 的消息，会得出「取词失败」的假结论。
        pub fn create() -> Windows {
            Self::create_with(true)
        }

        pub fn create_with(with_stealer: bool) -> Windows {
            let (tx, rx) = mpsc::channel();
            let join = std::thread::spawn(move || unsafe {
                let edit_cls = wide("EDIT");
                let static_cls = wide("STATIC");
                let text = wide(TEXT);
                let title = wide("focus probe stealer");
                let edit = CreateWindowExW(
                    0,
                    edit_cls.as_ptr(),
                    text.as_ptr(),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    120,
                    120,
                    520,
                    140,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                let stealer = if !with_stealer { std::ptr::null_mut() } else { CreateWindowExW(
                    0,
                    static_cls.as_ptr(),
                    title.as_ptr(),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    680,
                    120,
                    360,
                    140,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                ) };
                // 全选编辑框里的文字：用消息，不用按键
                SendMessageW(edit, EM_SETSEL, 0, -1);
                tx.send((edit as isize, stealer as isize, GetCurrentThreadId())).unwrap();
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                DestroyWindow(edit);
                DestroyWindow(stealer);
            });
            let (edit, stealer, thread_id) = rx.recv().unwrap();
            Windows { edit, stealer, thread_id, join: Some(join) }
        }
    }

    impl Drop for Windows {
        fn drop(&mut self) {
            unsafe {
                PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
            }
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }

    /// 等键鼠静止 3 秒再开跑（最多等 2 分钟）。静止的判据是「最后一次输入的时刻 3 秒内没变过」，
    /// 不用 GetTickCount 相减，省得多开一个 windows-sys 特性。
    pub fn wait_until_idle() -> bool {
        let last = || unsafe {
            let mut l = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
            GetLastInputInfo(&mut l);
            l.dwTime
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            let t = last();
            std::thread::sleep(Duration::from_secs(3));
            if last() == t {
                return true;
            }
        }
        false
    }

    /// 子进程模式：只建 EDIT，把句柄写到标准输出，然后等父进程关掉标准输入再退出
    pub fn host() {
        let w = Windows::create_with(false);
        println!("{}", w.edit);
        std::io::stdout().flush().ok();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        drop(w);
    }

    pub fn foreground() -> isize {
        unsafe { GetForegroundWindow() as isize }
    }

    /// 把自己的窗口切到前台，并确认真的切过去了
    pub fn bring(hwnd: isize) -> bool {
        unsafe {
            let me = GetCurrentThreadId();
            let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
            let target_thread = GetWindowThreadProcessId(hwnd as _, std::ptr::null_mut());
            AttachThreadInput(me, fg_thread, 1);
            AttachThreadInput(me, target_thread, 1);
            BringWindowToTop(hwnd as _);
            SetForegroundWindow(hwnd as _);
            AttachThreadInput(me, target_thread, 0);
            AttachThreadInput(me, fg_thread, 0);
        }
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            if foreground() == hwnd {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use probe::*;
    use std::io::BufRead;
    if std::env::args().any(|a| a == "--host") {
        host();
        return Ok(());
    }
    anyhow::ensure!(wait_until_idle(), "两分钟里键鼠一直有人在动，不抢前台，中止");
    let w = Windows::create();
    std::thread::sleep(std::time::Duration::from_millis(400));
    let mut failed = Vec::new();

    // A：用户的程序就在前台（基线）——应当取到选中的文字
    anyhow::ensure!(bring(w.edit), "没能把探针的编辑框切到前台，中止（不往别的窗口发按键）");
    let a = text_capture::clipboard::selected_text()?;
    println!("[A] 编辑框在前台：取到 {a:?}");
    if a != TEXT {
        failed.push("A 基线没取到文字（这台机器的剪贴板或按键注入本身有问题，后面的结论不可信）");
    }

    // B：前台被别的窗口抢走（等同点了「译」图标之后）——旧实现会取到空
    anyhow::ensure!(bring(w.stealer), "没能把探针的空窗口切到前台，中止（不往别的窗口发按键）");
    let b = text_capture::clipboard::selected_text()?;
    println!("[B] 前台被抢走后直接取词：取到 {b:?}");
    if !b.is_empty() {
        failed.push("B 预期复现出空文本，却取到了内容——复现条件不成立，根因判断要重看");
    }

    // C：前台仍被抢着，但按「有选区的窗口」取词（修复后的路径）——应当取到文字
    anyhow::ensure!(bring(w.stealer), "没能把探针的空窗口切到前台，中止（不往别的窗口发按键）");
    let c = text_capture::clipboard::selected_text_from(Some(w.edit))?;
    println!("[C] 前台被抢走后按原窗口取词：取到 {c:?}，此时前台是编辑框={}", foreground() == w.edit);
    if c != TEXT {
        failed.push("C 修复后的路径没取到文字");
    }

    // E：有选区的窗口在另一个进程里（真实场景）——跨进程切回前台后应当取到文字
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg("--host")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)?;
    let foreign: isize = line.trim().parse()?;
    std::thread::sleep(std::time::Duration::from_millis(400));
    anyhow::ensure!(bring(w.stealer), "没能把探针的空窗口切到前台，中止（不往别的窗口发按键）");
    let e = text_capture::clipboard::selected_text_from(Some(foreign));
    drop(child.stdin.take()); // 关标准输入 = 叫子进程退出
    let _ = child.wait();
    match e {
        Ok(t) if t == TEXT => println!("[E] 选区在另一个进程：取到 {t:?}"),
        Ok(t) => {
            println!("[E] 选区在另一个进程：取到 {t:?}");
            failed.push("E 跨进程切回后没取到文字");
        }
        Err(err) => {
            println!("[E] 选区在另一个进程：报错 {err}");
            failed.push("E 跨进程切不回前台");
        }
    }

    // D：记下的窗口已经不在了——应当明确报错，而不是返回空文本冒充「没选中」
    let gone = w.stealer;
    drop(w);
    std::thread::sleep(std::time::Duration::from_millis(300));
    match text_capture::clipboard::selected_text_from(Some(gone)) {
        Err(e) => println!("[D] 原窗口已关：报错 {e}"),
        Ok(t) => {
            println!("[D] 原窗口已关：却返回了 {t:?}");
            failed.push("D 原窗口不存在时应当报错");
        }
    }

    if failed.is_empty() {
        println!("\n通过：B 复现了缺陷；C 修复后能从原窗口取到文字；E 跨进程同样取到；D 原窗口不在时明确报错。");
        Ok(())
    } else {
        anyhow::bail!("{}", failed.join("；"))
    }
}

#[cfg(not(windows))]
fn main() {}
