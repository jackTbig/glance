//! 通用小工具
use std::path::{Path, PathBuf};

/// Windows 上 `Command::new("codex")` 不会找 .cmd/.bat；这里按 PATH 补全成完整路径。
/// PATH 里没有再看 claude 原生安装器与 npm 全局的常见位置。找不到就原样返回。
pub fn resolve_bin(name: &str) -> String {
    let p = Path::new(name);
    if p.components().count() > 1 || p.exists() {
        return name.to_string();
    }
    let exts = ["exe", "cmd", "bat", "com"];
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for ext in exts {
                let cand: PathBuf = dir.join(format!("{name}.{ext}"));
                if cand.is_file() {
                    return cand.to_string_lossy().into_owned();
                }
            }
            let bare = dir.join(name);
            if bare.is_file() {
                return bare.to_string_lossy().into_owned();
            }
        }
    }
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        extra.push(PathBuf::from(&home).join(".local").join("bin").join(format!("{name}.exe")));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        extra.push(PathBuf::from(&appdata).join("npm").join(format!("{name}.cmd")));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        extra.push(PathBuf::from(&local).join("Programs").join(name).join(format!("{name}.exe")));
    }
    for cand in extra {
        if cand.is_file() {
            return cand.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_bin_keeps_paths_and_finds_python_on_path() {
        assert_eq!(resolve_bin(r"C:\x\y.exe"), r"C:\x\y.exe");
        let p = resolve_bin("python");
        assert!(p.to_ascii_lowercase().ends_with("python.exe") || p == "python", "{p}");
    }
}
