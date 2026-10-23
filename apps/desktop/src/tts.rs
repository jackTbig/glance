//! Windows 朗读：WinRT 语音合成（和 Chrome 的 Web Speech 同一套引擎）+ 系统自带朗读兜底。
//!
//! **为什么换引擎**：Win10/11 的新音色注册在 `HKLM\SOFTWARE\Microsoft\Speech_OneCore`，
//! 经典 SAPI5 只认 `...\Speech`，两棵注册表树互不相通。Chrome 走 WinRT，看得到新音色；
//! 桌面端原来走 SAPI5，只看得到老的 "Huihui Desktop"。实测（2026-10-22）：
//! Chrome 与 WinRT 都只列出 Huihui / Kangkang / Yaoyao 三个中文音色，
//! SAPI5 只有 Zira Desktop（英）与 Huihui Desktop（中）——中文这一侧插件明显更好听。
//!
//! **英文音色**：WinRT 的音色列表里没有英文（本机没装 OneCore 英文语音包），
//! 所以读英文时退回 SAPI5 的 Zira Desktop；装上语音包后自动改走 WinRT 的 David / Zira Natural。
//!
//! **怎么出声**：WinRT 的 SpeechSynthesizer 只产出音频流、不直接发声，交给 MediaPlayer 放。
//! MediaPlayer 不是 agile 对象（不能跨线程用），所以有专门的朗读线程持有它，
//! 界面线程只发消息，绝不阻塞。
//!
//! **打断**：新朗读进来先停掉正在放的那一段。
//!
//! **不处理**：音速/音量/语种手动切换——简译只读原文一种语言，中英自动选就够。

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// `CREATE_NO_WINDOW`：不弹控制台窗口。父进程是 GUI 程序（windows_subsystem="windows"），
/// 但子进程照样会开一个可见的 powershell 窗口——每次点朗读都闪一个黑框，很跳。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 系统自带朗读（SAPI5）起的进程：`None` 表示还没起过。
/// 只在「这个语言没有 WinRT 音色」时才用得到（比如没装英文语音包时读英文）。
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
    win::speak(trimmed);
}

/// 系统里有哪些语音音色（语言标签），给运行环境自检用。
/// 没有 "en" 就是英文语音包没装——此时读英文会退回老音色。
pub fn voice_languages() -> Vec<String> {
    win::voice_languages()
}

/// 文本主要是什么语言。**只看有没有 CJK 字符**：简译只读原文，中英两种足够，
/// 分得更细（en-US / en-GB）反而会在音色列表里挑花眼。
fn lang_prefix(text: &str) -> &'static str {
    if text.chars().any(is_cjk) {
        "zh"
    } else {
        "en"
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F   // 中日韩标点
        | 0x3400..=0x4DBF // 扩展 A
        | 0x4E00..=0x9FFF // 基本区
        | 0xF900..=0xFAFF // 兼容表意
        | 0xFF00..=0xFFEF // 全角
    )
}

/// 系统自带朗读（SAPI 5.4）：起一个常驻 PowerShell 进程，新朗读自动打断旧的。
///
/// 用 PowerShell shim 是为了不引 SAPI COM 绑定：System.Speech 是 .NET Framework 自带，
/// PowerShell 一句 `Add-Type -AssemblyName System.Speech` 就把它加载进来了。
///
/// **只在 WinRT 整个不可用时才走这条路**：这套是老的 SAPI5 引擎，机器味重
/// （Jack 2026-10-23 报「主程序的朗读还是机器味」就是落到了这里）。
/// 哪怕语言对不上，也宁可让 WinRT 的默认音色念——那和浏览器插件是同一个引擎。
fn sapi_speak(text: &str) {
    if text.trim().is_empty() {
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
                let _ = writeln!(stdin, "{text}");
            }
            *slot = Some(Speaker { child });
        }
        Err(e) => {
            tracing::warn!("起 PowerShell 朗读进程失败：{e}");
        }
    }
}

/// WinRT 语音合成。跟 Chrome 的 Web Speech API 走的是同一个引擎。
mod win {
    use super::*;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::OnceLock;
    use windows::Media::Playback::MediaPlayer;
    use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
    use windows::Storage::Streams::IRandomAccessStream;
    use windows::core::{HSTRING, Interface};

    /// 朗读请求队列。界面线程只往这里丢一句话，真正干活的是朗读线程。
    static SENDER: OnceLock<Mutex<Option<Sender<String>>>> = OnceLock::new();

    fn sender() -> Sender<String> {
        let mut slot = SENDER.get_or_init(|| Mutex::new(None)).lock().unwrap();
        if let Some(tx) = slot.as_ref() {
            return tx.clone();
        }
        let (tx, rx) = channel::<String>();
        std::thread::spawn(move || worker(rx));
        *slot = Some(tx.clone());
        tx
    }

    /// 朗读线程：持有 MediaPlayer（它不是 agile 对象，不能跨线程用），按队列一条条念。
    fn worker(rx: Receiver<String>) {
        let player = MediaPlayer::new().ok();
        while let Ok(text) = rx.recv() {
            if let Some(p) = &player {
                // MediaPlayer 没有 Stop()，只有 Pause()：先暂停旧的，
                // 下面换上新音频流再 Play()，旧的那段就断了。
                let _ = p.Pause();
            }
            // 优先挑语言对得上的音色；没有就用系统默认音色（**不要退回 SAPI**——
            // 那套是老的 SAPI5 引擎，机器味重；WinRT 默认音色和浏览器插件是同一个引擎，
            // 听感一致。Jack 2026-10-23 报「主程序的朗读还是机器味」就是退回 SAPI 造成的）
            let voice = voice_for(&text).or_else(default_voice);
            match voice {
                Some(v) => {
                    if let Err(e) = play(&text, &v, player.as_ref()) {
                        tracing::warn!("WinRT 朗读失败（{e}），退回系统自带朗读");
                        sapi_speak(&text);
                    }
                }
                // WinRT 整个不可用（没装语音合成组件等），才走老路
                None => {
                    tracing::warn!("系统没有可用语音音色，退回系统自带朗读");
                    sapi_speak(&text);
                }
            }
        }
    }

    /// 挑一个念这段文本的音色。系统默认音色语言对得上就用它（用户可能自己换过默认）。
    pub(super) fn voice_for(text: &str) -> Option<VoiceInformation> {
        let prefix = lang_prefix(text);
        if let Some(def) = default_voice() {
            if lang_of(&def).to_lowercase().starts_with(prefix) {
                return Some(def);
            }
        }
        let voices = SpeechSynthesizer::AllVoices().ok()?;
        let n = voices.Size().unwrap_or(0);
        for i in 0..n {
            let Ok(v) = voices.GetAt(i) else { continue };
            if lang_of(&v).to_lowercase().starts_with(prefix) {
                return Some(v);
            }
        }
        None
    }

    /// 系统默认音色。没有匹配语言的音色时用它——**和浏览器插件同一套引擎**。
    fn default_voice() -> Option<VoiceInformation> {
        SpeechSynthesizer::DefaultVoice().ok()
    }

    fn lang_of(v: &VoiceInformation) -> String {
        v.Language().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    }

    /// 合成一段话并放出来。WinRT 只给音频流，声音是 MediaPlayer 放的。
    fn play(text: &str, voice: &VoiceInformation, player: Option<&MediaPlayer>) -> anyhow::Result<()> {
        let synth = SpeechSynthesizer::new()?;
        synth.SetVoice(voice)?;
        let stream = synth.SynthesizeTextToStreamAsync(&HSTRING::from(text))?.join()?;
        let ras: IRandomAccessStream = stream.cast()?;
        let player = player.ok_or_else(|| anyhow::anyhow!("音频播放器不可用"))?;
        player.SetStreamSource(&ras)?;
        player.Play()?;
        tracing::info!(
            "朗读：WinRT 引擎，音色 {:?}（{}），{} 字",
            voice.DisplayName().map(|s| s.to_string_lossy().to_string()),
            lang_of(voice),
            text.chars().count()
        );
        Ok(())
    }

    pub(super) fn voice_languages() -> Vec<String> {
        let mut out = Vec::new();
        let Ok(voices) = SpeechSynthesizer::AllVoices() else { return out };
        let n = voices.Size().unwrap_or(0);
        for i in 0..n {
            let Ok(v) = voices.GetAt(i) else { continue };
            out.push(lang_of(&v));
        }
        out
    }

    pub(super) fn speak(text: &str) {
        // 界面线程只发消息：挑音色、合成、播放都在朗读线程上做，绝不阻塞界面。
        // 找不到匹配音色的语言（英文没装语音包）由朗读线程那边退回 SAPI。
        let _ = sender().send(text.to_string());
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

    /// 语言判定：只看 CJK 字符。分错语言会挑错音色（英文念成中文口音）。
    #[test]
    fn lang_prefix_follows_cjk() {
        assert_eq!(lang_prefix("Hello world"), "en");
        assert_eq!(lang_prefix("你好，世界"), "zh");
        assert_eq!(lang_prefix("Rust 是一门语言"), "zh");
        assert_eq!(lang_prefix(""), "en");
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
        sapi_speak("");
        sapi_speak("   \n\t  ");
        let slot = SPEAKER.lock().unwrap();
        assert!(slot.is_none(), "空文本不该起进程");
    }

    #[test]
    fn speak_then_speak_replaces() {
        let _g = lock_tests();
        sapi_speak("hello world");
        {
            let slot = SPEAKER.lock().unwrap();
            assert!(slot.is_some(), "第一次 speak 应起进程");
        }
        sapi_speak("foo bar");
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
        // 只看 sapi_speak() 里那次 spawn，测试模块自己的 spawn 不算
        let body = src
            .split("fn sapi_speak")
            .nth(1)
            .and_then(|s| s.split("/// WinRT 语音合成").next())
            .expect("找不到 sapi_speak()");
        assert!(
            body.contains(".creation_flags(CREATE_NO_WINDOW)"),
            "sapi_speak() 起的 powershell 必须带 CREATE_NO_WINDOW，否则会弹终端窗口"
        );
    }
}
