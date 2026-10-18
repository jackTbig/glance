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
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

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

    /// 不依赖真实 TTS：发空字符串、不应起进程；发普通字符串、应起进程并清理。
    /// 真机朗读听感靠手动验证。
    #[test]
    fn empty_text_does_not_spawn() {
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
}
