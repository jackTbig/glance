//! Windows SAPI 5.4 朗读：起一个常驻 PowerShell 进程，新朗读自动打断旧的。
//!
//! 用 PowerShell shim 是为了不引 SAPI COM 绑定：System.Speech 是 .NET Framework 自带，
//! PowerShell 一句 `Add-Type -AssemblyName System.Speech` 就把它加载进来了。代价是
//! 启动一个 PS 进程约 200ms；这是用户点「朗读」按钮触发的，频次低、可接受。
//!
//! **打断机制**：单例 PS 进程，新 `speak()` 进来先发一个换行符触发旧进程在「Speak 阻塞
//! 中」那次返回（PS 阻塞读 stdin），再起新的。简单可靠；不引第三方 TTS 库。
//!
//! **不处理**：音速/音量/语种切换——简译只读原文一种语言，一种就够了。

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// `CREATE_NO_WINDOW`：不弹控制台窗口。父进程是 GUI 程序（windows_subsystem="windows"），
/// 但子进程照样会开一个可见的 powershell 窗口——每次点朗读都闪一个黑框，很跳。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 全局朗读器：`None` 表示还没起过进程；`Some(child)` 是当前在跑的 PS 进程
static SPEAKER: Mutex<Option<Speaker>> = Mutex::new(None);

struct Speaker {
    child: Child,
}

/// 朗读一段文本。会打断上一次正在进行的朗读。
///
/// 空文本 / 仅空白 / 控制字符都直接忽略（不报错，不起进程）。
pub fn speak(text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    let mut slot = SPEAKER.lock().unwrap();
    // 杀旧 PS 进程，下一次 speak 重新起。Wait 不阻塞——已经死掉的 child.wait() 是非阻塞的，
    // 还活着的 child.wait() 立即返回（child.kill 已发过 SIGKILL 等价信号）
    if let Some(mut s) = slot.take() {
        let _ = s.child.kill();
        let _ = s.child.wait();
    }

    let script = format!(
        "$ErrorActionPreference='Stop'; \
         Add-Type -AssemblyName System.Speech; \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         while (($line = [Console]::In.ReadLine()) -ne $null) {{ \
             try {{ $s.Speak($line) }} catch {{ }} \
         }}"
    );

    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(mut child) => {
            // 写一次要朗读的文本 + 换行触发 PS 的 ReadLine 返回；stdin 用完即弃，
            // 留着会让 PS 一直等下一行不会自然退出
            if let Some(mut stdin) = child.stdin.take() {
                let _ = writeln!(stdin, "{trimmed}");
            }
            *slot = Some(Speaker { child });
        }
        Err(e) => {
            tracing::warn!("起 PowerShell 朗读进程失败：{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试串行锁：`SPEAKER` 是全局单槽，几个测试并行跑会互相踩——
    /// 一个测试的 `speak()` 正好落在另一个测试断言「不该起进程」的窗口里。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 取测试锁。上一条测试 panic 过的话锁中毒，这里照样拿到锁（测试之间不该连坐）。
    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 不依赖真实 TTS：发空字符串、不应起进程；发普通字符串、应起进程并清理。
    /// 真机朗读听感靠手动验证。
    #[test]
    fn empty_text_does_not_spawn() {
        let _g = lock_tests();
        // 先清掉前序测试可能残留的进程
        if let Some(mut s) = SPEAKER.lock().unwrap().take() {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
        speak("");
        speak("   \n\t  ");
        let slot = SPEAKER.lock().unwrap();
        assert!(slot.is_none(), "空文本不该起进程");
    }

    #[test]
    fn speak_then_speak_replaces() {
        let _g = lock_tests();
        speak("hello world");
        {
            let slot = SPEAKER.lock().unwrap();
            assert!(slot.is_some(), "第一次 speak 应起进程");
        }
        speak("foo bar");
        {
            let slot = SPEAKER.lock().unwrap();
            assert!(slot.is_some(), "第二次 speak 应替换进程，旧进程已 kill");
        }
        // 收尾：清掉进程
        if let Some(mut s) = SPEAKER.lock().unwrap().take() {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
    }

    /// 回归：生产用的 spawn 必须带 `CREATE_NO_WINDOW`，否则会弹终端窗口。
    ///
    /// 真机验证过：GUI 父进程（release 版 `windows_subsystem="windows"`）起 powershell
    /// 不带这个标志时，屏幕上会多出一个 Windows Terminal 窗口——类名
    /// `CASCADIA_HOSTING_WINDOW_CLASS`，标题是 powershell.exe 的完整路径。
    /// 用户点一次朗读闪一个窗口，很跳。
    ///
    /// 为什么查源码不查窗口：窗口是否真出现取决于**父进程有没有控制台**。
    /// 测试进程通常自带控制台，子进程继承它就不新开窗口，测了也是白测
    /// （去掉标志照样绿）。插件测试也是这么从 `scene.rs` 抠用例跑的。
    #[test]
    fn production_spawn_carries_create_no_window() {
        // 常量值写错会让标志静默失效（宁可不生效也不能弹窗）
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000, "CREATE_NO_WINDOW 的值不对");

        let src = include_str!("tts.rs");
        // 只看 speak() 里那次 spawn，测试模块自己的 spawn 不算
        let body = src
            .split("pub fn speak")
            .nth(1)
            .and_then(|s| s.split("#[cfg(test)]").next())
            .expect("找不到 speak()");
        assert!(
            body.contains(".creation_flags(CREATE_NO_WINDOW)"),
            "speak() 起的 powershell 必须带 CREATE_NO_WINDOW，否则会弹终端窗口"
        );
    }
}
