//! 运行环境自检：这台机器缺什么、缺了影响哪个功能、怎么补。
//!
//! 安装包能内嵌的都内嵌了（VC++ 运行库、识别模型、浏览器插件），但有三类东西装不进安装包：
//! - **随显卡驱动来的**（`vulkan-1.dll`）——只能提示用户更新驱动。
//! - **Windows 可选功能**（OCR 语言包）——要联网或 FoD 源，且需要管理员权限。
//! - **第三方 CLI**（claude / codex）——是可选后端，用户未必想装。
//!
//! 所以这里做三件事：**查得出**（`check_all`）、**说得清**（每项带影响与办法）、
//! **修得了**（`Fix` 描述一个可执行的修复动作，由界面在用户点下按钮时才执行）。
//! 修复一律不静默进行：装系统组件、跑网络安装脚本都要用户当场点头。

use std::path::PathBuf;

/// 一项依赖的检查结果
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// 显示名
    pub name: &'static str,
    pub status: Status,
    /// 查到的实际情况（版本、路径、语言列表……），一行以内
    pub detail: String,
    /// 缺了会怎样。**必须写清影响的是哪个功能**，否则用户无从判断要不要管
    pub impact: &'static str,
    /// 怎么补。没有可自动执行的动作时，这里是给人看的步骤
    pub how: String,
    /// 能一键修的话，是什么动作
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 有，且可用
    Ok,
    /// 没有，但只影响可选功能
    Optional,
    /// 没有，对应功能直接不能用
    Missing,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Ok => "正常",
            Status::Optional => "未装（可选）",
            Status::Missing => "缺失",
        }
    }
}

/// 一个可执行的修复动作。**构造它不执行任何东西**，执行要显式调 `run`。
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    /// 用管理员权限跑一段 PowerShell（装 Windows 可选功能用）
    Elevated { label: &'static str, powershell: String },
    /// 以当前用户跑一段 PowerShell（装用户级 CLI 用）
    User { label: &'static str, powershell: String },
    /// 打开一个网页让用户自己下载
    OpenUrl { label: &'static str, url: &'static str },
    /// 跑一个本地程序（安装包内嵌的运行库等）
    RunLocal { label: &'static str, exe: PathBuf, args: Vec<String> },
}

impl Fix {
    pub fn label(&self) -> &'static str {
        match self {
            Fix::Elevated { label, .. } | Fix::User { label, .. } | Fix::OpenUrl { label, .. } | Fix::RunLocal { label, .. } => label,
        }
    }

    /// 这个修复会不会弹 UAC。界面要提前告诉用户，别让提权框显得突兀。
    pub fn needs_admin(&self) -> bool {
        matches!(self, Fix::Elevated { .. })
    }
}

/// 检查这台机器的运行环境。`app_dir` 是程序所在目录（找内嵌的 vc_redist 用）。
///
/// `voice_languages` 是系统里可用的语音音色语言标签（朗读用），由调用方从朗读模块拿。
pub fn check_all(
    app_dir: Option<&std::path::Path>,
    ocr_languages: &[String],
    voice_languages: &[String],
) -> Vec<Item> {
    vec![
        vc_redist(app_dir),
        vulkan(),
        ocr_chinese(ocr_languages),
        ocr_english(ocr_languages),
        english_voice(voice_languages),
        offline_dict(app_dir),
        chrome(),
        cli("claude"),
        cli("codex"),
    ]
}

/// 有没有哪一项是硬缺失
pub fn has_blocking(items: &[Item]) -> bool {
    items.iter().any(|i| i.status == Status::Missing)
}

fn vc_redist(app_dir: Option<&std::path::Path>) -> Item {
    let ok = vc_redist_installed();
    // 安装包会把 vc_redist 放进 {tmp} 装完即删，所以这里多半找不到；找得到就能一键重装
    let local = app_dir.map(|d| d.join("vc_redist.x64.exe")).filter(|p| p.exists());
    Item {
        name: "VC++ 2015-2022 运行库",
        status: if ok { Status::Ok } else { Status::Missing },
        detail: if ok { "已安装".into() } else { "未检测到".into() },
        impact: "实时字幕起不来（whisper 的原生库依赖它）",
        how: "安装包会自动装；若已跳过，从微软官网下载 vc_redist.x64.exe 安装".into(),
        fix: if ok {
            None
        } else if let Some(exe) = local {
            Some(Fix::RunLocal { label: "安装运行库", exe, args: vec!["/install".into(), "/passive".into(), "/norestart".into()] })
        } else {
            Some(Fix::OpenUrl { label: "打开下载页", url: "https://aka.ms/vs/17/release/vc_redist.x64.exe" })
        },
    }
}

#[cfg(windows)]
fn vc_redist_installed() -> bool {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
    let sub: Vec<u16> = r"SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "Installed".encode_utf16().chain([0]).collect();
    let mut val: u32 = 0;
    let mut len: u32 = 4;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut val as *mut u32 as *mut core::ffi::c_void,
            &mut len,
        )
    };
    rc == ERROR_SUCCESS && val == 1
}

#[cfg(not(windows))]
fn vc_redist_installed() -> bool {
    true
}

/// Vulkan 运行库。**装不进安装包**：它是显卡驱动的一部分，微软与显卡厂商都不提供独立运行时分发包。
fn vulkan() -> Item {
    let path = system_dir().join("vulkan-1.dll");
    let ok = path.exists();
    Item {
        name: "Vulkan 运行库",
        status: if ok { Status::Ok } else { Status::Missing },
        detail: if ok { format!("{}", path.display()) } else { "system32 下没有 vulkan-1.dll".into() },
        // 这句是 2026-09-09 解析 PE 导入表确认的，别改软
        impact: "实时字幕**完全无法启动**（不是没有 GPU 加速：vulkan-1.dll 是加载期依赖，缺了 Windows 直接拒绝启动）",
        how: "更新显卡驱动。它随驱动一起装，没有独立的运行库安装包。".into(),
        fix: (!ok).then_some(Fix::OpenUrl { label: "打开驱动下载页", url: "https://www.intel.cn/content/www/cn/zh/download-center/home.html" }),
    }
}

fn ocr_chinese(langs: &[String]) -> Item {
    let ok = langs.iter().any(|l| l.starts_with("zh"));
    Item {
        name: "OCR 中文语言包",
        status: if ok { Status::Ok } else { Status::Missing },
        detail: if langs.is_empty() { "本机没有任何 OCR 语言包".into() } else { format!("已装：{}", langs.join("、")) },
        impact: "截屏翻译与悬停取词的识别兜底都用不了",
        how: "以管理员身份运行：Add-WindowsCapability -Online -Name \"Language.OCR~~~zh-Hans-CN~0.0.1.0\"".into(),
        fix: (!ok).then_some(Fix::Elevated {
            label: "安装中文 OCR 包",
            powershell: "Add-WindowsCapability -Online -Name 'Language.OCR~~~zh-Hans-CN~0.0.1.0'".into(),
        }),
    }
}

fn ocr_english(langs: &[String]) -> Item {
    let ok = langs.iter().any(|l| l.starts_with("en"));
    Item {
        name: "OCR 英文语言包",
        status: if ok { Status::Ok } else { Status::Optional },
        detail: if ok { "已装".into() } else { "未装".into() },
        // 实测过的具体错法，比「识别不准」有用
        impact: "截屏识别英文会插空格、It 认成 lt、连字符认成一（模型翻译时能纠正大半，够用）",
        how: "以管理员身份运行：Add-WindowsCapability -Online -Name \"Language.OCR~~~en-US~0.0.1.0\"".into(),
        fix: (!ok).then_some(Fix::Elevated {
            label: "安装英文 OCR 包",
            powershell: "Add-WindowsCapability -Online -Name 'Language.OCR~~~en-US~0.0.1.0'".into(),
        }),
    }
}

/// 英文语音包（朗读用）。**不是硬缺失**：缺了只是英文朗读用中文音色念，还能用。
///
/// 朗读走 WinRT 引擎（和 Chrome 的 Web Speech 同一套），它只列 OneCore 音色；
/// 英文那几个（Microsoft David / Zira / Mark）要装系统语音包才有。
/// 没装时读英文会拿中文音色念（和浏览器插件一样），没有纯正英文音色。
fn english_voice(voice_langs: &[String]) -> Item {
    let ok = voice_langs.iter().any(|l| l.to_lowercase().starts_with("en"));
    Item {
        name: "英文语音包（朗读）",
        status: if ok { Status::Ok } else { Status::Optional },
        detail: if ok {
            "已装".into()
        } else if voice_langs.is_empty() {
            "系统里没有可用音色".into()
        } else {
            format!("未装（现有：{}）", voice_langs.join("、"))
        },
        impact: "英文朗读用中文音色念（和浏览器插件一样），没有纯正英文音色；装上后桌面端与插件都会用 Microsoft David / Zira / Mark",
        how: "以管理员身份运行下面两条（**Basic 必须一起装**，只装 TextToSpeech 会因缺依赖挂死半小时报 0x80073712）：\n"
            .to_string()
            + "Add-WindowsCapability -Online -Name \"Language.Basic~~~en-US~0.0.1.0\"\n"
            + "Add-WindowsCapability -Online -Name \"Language.TextToSpeech~~~en-US~0.0.1.0\"",
        fix: (!ok).then_some(Fix::Elevated {
            label: "安装英文语音包",
            powershell: "Add-WindowsCapability -Online -Name 'Language.Basic~~~en-US~0.0.1.0' | Out-Null; Add-WindowsCapability -Online -Name 'Language.TextToSpeech~~~en-US~0.0.1.0' | Out-Null".into(),
        }),
    }
}

/// 离线词典。**不是硬缺失**：没有它只是模型慢的时候没有兜底，翻译本身照常。
fn offline_dict(app_dir: Option<&std::path::Path>) -> Item {
    let found = dict_path(app_dir);
    let size = found.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0);
    Item {
        name: "离线词典",
        status: if found.is_some() { Status::Ok } else { Status::Optional },
        detail: match &found {
            Some(p) => format!("{}（{:.0} MB）", p.display(), size as f64 / 1_048_576.0),
            None => "未安装".into(),
        },
        impact: "取词/划词到单个词时没有本地释义兜底：模型慢或断网时要一直等着",
        how: "重新运行安装包、勾上「离线词典」组件".into(),
        fix: None,
    }
}

/// 词库文件的位置。与 `translate_tool::offline::dict_path` 是同一套判据，
/// 那边给运行时用、这边给自检与安装向导用；**改一处要改两处**。
pub fn dict_path(app_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TT_DICT").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = app_dir {
        candidates.push(d.join("dict").join("ecdict.bin"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            candidates.push(d.join("dict").join("ecdict.bin"));
        }
    }
    candidates.push(PathBuf::from(r"D:\DevCache\tt-dict\ecdict.bin"));
    candidates.into_iter().find(|p| p.is_file())
}

fn chrome() -> Item {
    let found = chrome_path();
    Item {
        name: "Chrome 浏览器",
        status: if found.is_some() { Status::Ok } else { Status::Optional },
        detail: found.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "未找到".into()),
        impact: "浏览器插件（整页翻译、网页划词）用不了；桌面端功能不受影响",
        how: "装 Chrome 后，在设置页的「浏览器插件」一栏按三步加载插件".into(),
        fix: found.is_none().then_some(Fix::OpenUrl { label: "打开 Chrome 下载页", url: "https://www.google.cn/chrome/" }),
    }
}

pub fn chrome_path() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join(r"Google\Chrome\Application\chrome.exe"));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// claude / codex CLI。两个都是**可选**后端，默认走 OpenAI 兼容接口。
fn cli(name: &'static str) -> Item {
    let found = which(name);
    let (impact, how, fix) = match name {
        "claude" => (
            "翻译后端里选 claude 会失败；默认的 OpenAI 兼容后端不受影响",
            "装 Claude Code CLI（会联网下载并执行官方安装脚本）",
            Fix::User { label: "安装 Claude CLI", powershell: "irm https://claude.ai/install.ps1 | iex".into() },
        ),
        _ => (
            "翻译后端里选 codex 会失败；默认的 OpenAI 兼容后端不受影响",
            "用 npm 安装：npm i -g @openai/codex",
            Fix::User { label: "用 npm 安装 codex", powershell: "npm i -g @openai/codex".into() },
        ),
    };
    Item {
        name: if name == "claude" { "Claude Code CLI（可选后端）" } else { "Codex CLI（可选后端）" },
        status: if found.is_some() { Status::Ok } else { Status::Optional },
        detail: found.unwrap_or_else(|| "未在 PATH 里找到".into()),
        impact,
        how: how.into(),
        fix: fix_if_missing(found_none(name), fix),
    }
}

fn found_none(name: &str) -> bool {
    which(name).is_none()
}

fn fix_if_missing(missing: bool, f: Fix) -> Option<Fix> {
    missing.then_some(f)
}

/// 在 PATH 里找可执行文件。Windows 上 `Command::new` 不认 .cmd，所以要自己补后缀。
fn which(name: &str) -> Option<String> {
    let exts = if cfg!(windows) { &["exe", "cmd", "bat", "com"][..] } else { &[""][..] };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let cand = if ext.is_empty() { dir.join(name) } else { dir.join(format!("{name}.{ext}")) };
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn system_dir() -> PathBuf {
    std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32")
}

/// 执行一个修复动作。**只应在用户点了按钮之后调**。
///
/// 提权的那类走 `runas`，会弹 UAC；用户点「否」就是 Err，如实报出来，不要重试。
pub fn run_fix(fix: &Fix) -> anyhow::Result<()> {
    match fix {
        Fix::Elevated { powershell, .. } => run_powershell(powershell, true),
        Fix::User { powershell, .. } => run_powershell(powershell, false),
        Fix::OpenUrl { url, .. } => open_url(url),
        Fix::RunLocal { exe, args, .. } => {
            std::process::Command::new(exe).args(args).spawn().map_err(|e| anyhow::anyhow!("启动 {} 失败：{e}", exe.display()))?;
            Ok(())
        }
    }
}

/// 跑一段 PowerShell。装系统组件动辄几分钟，所以**不等它结束**，让窗口留着给用户看进度。
///
/// 故意用带窗口的 PowerShell 而不是隐藏窗口：这些命令要联网下载几十上百 MB，
/// 没有可见进度的话用户会以为点了没反应，然后反复点。
#[cfg(windows)]
fn run_powershell(script: &str, elevated: bool) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
    // -NoExit 让窗口停住：装完的成功/失败信息要留给用户看
    let args = format!("-NoProfile -ExecutionPolicy Bypass -NoExit -Command \"{}\"", script.replace('"', "\\\""));
    let verb = wide(if elevated { "runas" } else { "open" });
    let file = wide("powershell.exe");
    let params = wide(&args);
    let rc = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), std::ptr::null(), SW_SHOWNORMAL as i32) };
    // ShellExecuteW 返回值 ≤32 是错误；提权被拒是 5（ERROR_ACCESS_DENIED）
    let code = rc as isize;
    if code <= 32 {
        if code == 5 {
            anyhow::bail!("需要管理员权限，提权被拒绝");
        }
        anyhow::bail!("启动 PowerShell 失败（错误码 {code}）");
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_powershell(_script: &str, _elevated: bool) -> anyhow::Result<()> {
    anyhow::bail!("当前平台不支持")
}

#[cfg(windows)]
fn open_url(url: &str) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w: Vec<u16> = std::ffi::OsStr::new(url).encode_wide().chain(std::iter::once(0)).collect();
    let open: Vec<u16> = std::ffi::OsStr::new("open").encode_wide().chain(std::iter::once(0)).collect();
    let rc = unsafe { ShellExecuteW(std::ptr::null_mut(), open.as_ptr(), w.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL as i32) };
    if (rc as isize) <= 32 {
        anyhow::bail!("打不开浏览器（错误码 {}）", rc as isize);
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_url(_url: &str) -> anyhow::Result<()> {
    anyhow::bail!("当前平台不支持")
}

/// 纯文本报告，给控制台自检与贴进反馈用
pub fn report(items: &[Item]) -> String {
    let mut s = String::from("简译 Glance 运行环境自检\n");
    s.push_str(&"─".repeat(40));
    s.push('\n');
    for i in items {
        s.push_str(&format!("[{}] {}\n    {}\n", i.status.label(), i.name, i.detail));
        if i.status != Status::Ok {
            s.push_str(&format!("    影响：{}\n    办法：{}\n", i.impact, i.how));
        }
    }
    let missing = items.iter().filter(|i| i.status == Status::Missing).count();
    let optional = items.iter().filter(|i| i.status == Status::Optional).count();
    s.push_str(&"─".repeat(40));
    s.push_str(&format!("\n{} 项缺失、{} 项可选未装、{} 项正常\n", missing, optional, items.len() - missing - optional));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_items_read_the_language_list() {
        let zh = ocr_chinese(&["zh-Hans-CN".into()]);
        assert_eq!(zh.status, Status::Ok);
        assert!(zh.fix.is_none(), "已装的不该给修复按钮");

        let none = ocr_chinese(&[]);
        assert_eq!(none.status, Status::Missing);
        assert!(none.detail.contains("没有任何"));
        assert!(none.fix.as_ref().unwrap().needs_admin(), "装语言包要提权，界面得先告诉用户");

        // 英文包缺了只是识别质量差，不该报成硬缺失吓人
        assert_eq!(ocr_english(&["zh-Hans-CN".into()]).status, Status::Optional);
        assert_eq!(ocr_english(&["en-US".into(), "zh-Hans-CN".into()]).status, Status::Ok);
    }

    #[test]
    fn blocking_only_counts_hard_misses() {
        let ok = Item { name: "a", status: Status::Ok, detail: String::new(), impact: "", how: String::new(), fix: None };
        let opt = Item { status: Status::Optional, ..ok.clone() };
        let bad = Item { status: Status::Missing, ..ok.clone() };
        assert!(!has_blocking(&[ok.clone(), opt.clone()]));
        assert!(has_blocking(&[ok, opt, bad]));
    }

    #[test]
    fn report_lists_reasons_only_for_problems() {
        let items = vec![
            Item { name: "好的", status: Status::Ok, detail: "已装".into(), impact: "不该出现", how: "不该出现".into(), fix: None },
            Item { name: "缺的", status: Status::Missing, detail: "没有".into(), impact: "会坏事", how: "这样修".into(), fix: None },
        ];
        let r = report(&items);
        assert!(r.contains("好的") && r.contains("缺的"));
        assert!(!r.contains("不该出现"), "正常项不用解释影响，铺开只会淹没真正的问题");
        assert!(r.contains("会坏事") && r.contains("这样修"));
        assert!(r.contains("1 项缺失、0 项可选未装、1 项正常"));
    }

    #[test]
    fn fixes_declare_whether_they_prompt_for_admin() {
        // 界面要靠这个提前提示，标错了用户会被突然弹出的 UAC 吓到
        assert!(Fix::Elevated { label: "x", powershell: "y".into() }.needs_admin());
        assert!(!Fix::User { label: "x", powershell: "y".into() }.needs_admin());
        assert!(!Fix::OpenUrl { label: "x", url: "y" }.needs_admin());
        assert!(!Fix::RunLocal { label: "x", exe: PathBuf::from("y"), args: vec![] }.needs_admin());
    }

    #[test]
    fn every_item_explains_itself() {
        let items = check_all(None, &[], &[]);
        for i in &items {
            assert!(!i.impact.is_empty(), "{} 没写影响，用户无从判断要不要管", i.name);
            assert!(!i.how.is_empty(), "{} 没写办法", i.name);
            if i.status != Status::Ok {
                assert!(i.fix.is_some() || !i.how.is_empty(), "{} 既不能一键修也没给步骤", i.name);
            }
        }
    }

    /// 英文语音包：没装只是读英文用老音色，不能报成硬缺失吓人
    #[test]
    fn english_voice_reads_the_voice_list() {
        assert_eq!(english_voice(&["en-US".into()]).status, Status::Ok);
        assert!(english_voice(&["en-US".into()]).fix.is_none(), "装好的不该给修复按钮");

        let none = english_voice(&["zh-CN".into()]);
        assert_eq!(none.status, Status::Optional);
        assert!(none.detail.contains("zh-CN"), "要说清现在有哪些音色");
        assert!(none.fix.as_ref().unwrap().needs_admin(), "装语音包要提权，界面得先告诉用户");
    }
}
