//! 把应用图标与版本信息写进 exe 的资源表（文件管理器、任务栏、属性页都读它），
//! 顺便把构建日期与 git commit 注入成编译期环境变量，供「关于」页显示。

/// 构建日期，UTC。不引 chrono：为了一行日期把一个依赖拉进构建脚本不值当，
/// 也不调 cmd —— 它返回的是 OEM/ANSI 编码（中文机是 GBK），当 UTF-8 读必出 ��。
/// Hinnant civil-from-days（公域算法），结果 YYYY-MM-DD HH:MM。
fn build_date() -> Option<String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let hh = (sod / 3600) as u32;
    let mm = ((sod % 3600) / 60) as u32;
    let z = days + 719_468;
    let era = if z >= 0 { z / 146_097 } else { (z - 146_096) / 146_097 };
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let yy0 = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dd = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let yy = if mo <= 2 { yy0 + 1 } else { yy0 };
    Some(format!("{yy:04}-{mo:02}-{dd:02} {hh:02}:{mm:02}"))
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