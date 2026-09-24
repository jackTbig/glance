//! Chrome 插件的安装辅助：找到插件目录、在资源管理器里定位、打开扩展页。
//!
//! Chrome 不允许程序静默装未上架的扩展，所以「安装」能做的就是把用户领到该点的地方：
//! 打开插件目录、打开扩展页，再把「加载已解压的扩展程序」这三步写清楚。

use std::path::{Path, PathBuf};

/// 插件目录：安装版在 exe 旁边的 `extension\`，开发时在仓库的 `extension\dist\`
pub fn dist_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let base = exe.parent()?;
    let installed = base.join("extension");
    if installed.join("manifest.json").exists() {
        return Some(installed);
    }
    // 开发时 exe 在 target 的 release 目录下，仓库在别处；顺着 CARGO_MANIFEST_DIR 找
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extension/dist");
    let dev = strip_unc(dev.canonicalize().ok()?);
    dev.join("manifest.json").exists().then_some(dev)
}

/// 去掉 canonicalize 加的 UNC 前缀（反斜杠反斜杠问号反斜杠）。
///
/// 那层前缀是给 Win32 绕 260 字符路径限制用的，但 Chrome 的目录选择框和资源管理器
/// 地址栏都不认它。展示给用户或让用户粘过去，只会平白多一次失败。
fn strip_unc(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().into_owned();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

/// 在资源管理器里打开并选中插件目录
pub fn reveal_dir(dir: &Path) -> anyhow::Result<()> {
    // 用 explorer /select 让目录处于选中态，用户一眼看到该选哪个
    std::process::Command::new("explorer.exe")
        .arg(dir.as_os_str())
        .spawn()
        .map_err(|e| anyhow::anyhow!("打不开资源管理器：{e}"))?;
    Ok(())
}

/// Chrome 的 Local State 里记的上次使用的配置档（如 "Profile 1" / "Default"）。
///
/// 多账号时直接 `chrome.exe <url>` 会先弹账号选择，选完 URL 被吞掉、落在新标签页
/// （Jack 2026-09-24 实测）。带上 `--profile-directory` 就不弹选择、直接开到目标页。
fn chrome_last_profile(state: &str) -> Option<String> {
    // 只挖 "last_used" 那一个字符串值，不为它引一个 JSON 依赖
    let key = "\"last_used\"";
    let i = state.find(key)? + key.len();
    let rest = &state[i..];
    let q1 = rest.find('"')?;
    let q2 = rest[q1 + 1..].find('"')? + q1 + 1;
    let v = rest[q1 + 1..q2].trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// 打开 Chrome 的扩展页。找不到 Chrome 就返回 Err，让调用方提示手动打开。
pub fn open_extensions_page() -> anyhow::Result<()> {
    let candidates = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    ];
    let mut chrome: Option<PathBuf> = candidates.iter().map(PathBuf::from).find(|p| p.exists());
    if chrome.is_none() {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let p = PathBuf::from(local).join(r"Google\Chrome\Application\chrome.exe");
            if p.exists() {
                chrome = Some(p);
            }
        }
    }
    let chrome = chrome.ok_or_else(|| anyhow::anyhow!("没找到 Chrome，请手动打开 chrome://extensions"))?;
    let mut cmd = std::process::Command::new(chrome);
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let user_data = PathBuf::from(local).join(r"Google\Chrome\User Data");
        let state = std::fs::read_to_string(user_data.join("Local State")).unwrap_or_default();
        if let Some(p) = chrome_last_profile(&state) {
            // 配置档目录真的存在才传：传一个不存在的名字 Chrome 会建一个空配置档，设置全丢
            if user_data.join(&p).is_dir() {
                cmd.arg(format!("--profile-directory={p}"));
            }
        }
    }
    cmd.arg("chrome://extensions")
        .spawn()
        .map_err(|e| anyhow::anyhow!("启动 Chrome 失败：{e}"))?;
    Ok(())
}


/// 把目录路径放进剪贴板，方便用户粘到扩展页的目录选择框里
pub fn copy_path(dir: &Path) -> anyhow::Result<()> {
    text_capture::clipboard::set_text(&dir.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_last_used_profile() {
        let s = r#"{ "profile": { "info_cache": {}, "last_used": "Profile 3" } }"#;
        assert_eq!(chrome_last_profile(s).as_deref(), Some("Profile 3"));
        let compact = r#"{"profile":{"last_used":"Default"}}"#;
        assert_eq!(chrome_last_profile(compact).as_deref(), Some("Default"));
        assert_eq!(chrome_last_profile("{}"), None);
        assert_eq!(chrome_last_profile(r#"{"last_used": ""}"#), None);
    }

    #[test]
    fn dist_dir_points_at_a_real_manifest() {
        // 开发环境下应该能找到仓库里的 extension/dist
        match dist_dir() {
            Some(d) => assert!(d.join("manifest.json").exists(), "{d:?} 里应该有 manifest.json"),
            None => eprintln!("跳过：插件还没打包（先在 extension/ 跑 npm run build）"),
        }
    }
}
