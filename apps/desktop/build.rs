//! 把应用图标与版本信息写进 exe 的资源表（文件管理器、任务栏、属性页都读它），
//! 顺便把构建日期与 git commit 注入成编译期环境变量，供「关于」页显示。

/// 构建日期。不引 chrono：为了一行日期把一个依赖拉进构建脚本不值当，
/// 用系统命令拿，拿不到就不显示。
fn build_date() -> Option<String> {
    let out = std::process::Command::new("cmd").args(["/C", "date /T"]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn git_commit() -> Option<String> {
    let out = std::process::Command::new("git").args(["rev-parse", "--short", "HEAD"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    // 关于页要显示「什么时候、从哪个提交编出来的」。取不到就不注入，
    // 关于页那边会退化成只显示版本号——**别写「未知」**，那比不写更让人心里没底。
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Some(d) = build_date() {
        println!("cargo:rustc-env=TT_BUILD_DATE={d}");
    }
    if let Some(c) = git_commit() {
        println!("cargo:rustc-env=TT_BUILD_COMMIT={c}");
    }

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("ProductName", "简译");
        res.set("FileDescription", "简译 Glance — 桌面与浏览器翻译工具");
        res.set("LegalCopyright", "Jack");
        if let Err(e) = res.compile() {
            // 没有 Windows SDK 的资源编译器时不阻断构建，只是 exe 没图标
            println!("cargo:warning=图标资源未能编入 exe：{e}");
        }
    }
}
