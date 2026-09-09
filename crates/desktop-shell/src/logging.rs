//! 日志：按天滚动、保留若干天。两个应用共用一套，但**文件名前缀必须各不相同**。
//!
//! 前缀写死是踩过的坑：两个进程同一天会打开同一个日志文件，各持一个句柄交错写，
//! 滚动时还互抢——排障时看到的是一份内容错乱、偶尔被截断的日志。

use std::path::Path;

/// 起日志。`prefix` 是文件名前缀（如 `translate-tool`、`caption`），
/// `env_var` 是控制级别的环境变量名。
///
/// 用同步写而不是 `tracing_appender::non_blocking`：这两个应用日志量都很小，
/// 非阻塞写的 BufWriter 要攒满才落盘，结果是跑了半天日志文件还是 0 字节，排障时什么也看不到。
///
/// **级别只认自己的环境变量，故意不读 `RUST_LOG`**：本机有一个用户级 `RUST_LOG=error`，
/// 读它的话应用日志会被静默压成 0 字节。`RUST_LOG` 是给开发工具链用的通用变量，
/// 面向用户的桌面应用不该被它左右。
pub fn init(dir: &Path, prefix: &str, env_var: &str, keep_days: u64) {
    let logs = dir.join("logs");
    if std::fs::create_dir_all(&logs).is_err() {
        return;
    }
    prune(&logs, prefix, keep_days);
    let file = tracing_appender::rolling::daily(&logs, format!("{prefix}.log"));
    let filter = build_filter(env_var);
    tracing_subscriber::fmt().with_writer(file).with_ansi(false).with_env_filter(filter).init();
}

/// 过滤串一律用 `try_new`：`EnvFilter::new` 遇到解析不了的指令会**静默丢掉**，
/// 全丢光时一条日志都不写，表现是「日志 0 字节」而不是报错。
fn build_filter(env_var: &str) -> tracing_subscriber::EnvFilter {
    let spec = std::env::var(env_var).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "info".into());
    tracing_subscriber::EnvFilter::try_new(&spec).unwrap_or_else(|e| {
        eprintln!("{env_var}={spec:?} 解析失败（{e}），退回 info");
        tracing_subscriber::EnvFilter::new("info")
    })
}

/// 删掉超过 keep_days 天没动过的日志。只删自己前缀的，别人的不碰。
fn prune(dir: &Path, prefix: &str, keep_days: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days * 86_400);
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_mine = name.starts_with(prefix) && name.contains(".log");
        let old = e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false);
        if is_mine && old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_falls_back_to_info_on_garbage() {
        // 解析不了不能变成「什么都不记」
        std::env::set_var("DS_TEST_LOG", "!!! 这不是过滤串 !!!");
        assert_eq!(build_filter("DS_TEST_LOG").to_string(), "info");
        std::env::set_var("DS_TEST_LOG", "   ");
        assert_eq!(build_filter("DS_TEST_LOG").to_string(), "info");
        std::env::set_var("DS_TEST_LOG", "debug");
        assert_eq!(build_filter("DS_TEST_LOG").to_string(), "debug");
        std::env::remove_var("DS_TEST_LOG");
        assert_eq!(build_filter("DS_TEST_LOG").to_string(), "info");
    }

    #[test]
    fn prune_only_touches_its_own_prefix() {
        let dir = std::env::temp_dir().join(format!("ds-prune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
        for name in ["mine.log.2026-01-01", "other.log.2026-01-01", "mine-notes.txt"] {
            let p = dir.join(name);
            std::fs::write(&p, b"x").unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(old).unwrap();
        }
        prune(&dir, "mine", 7);
        assert!(!dir.join("mine.log.2026-01-01").exists(), "自己的旧日志该删");
        assert!(dir.join("other.log.2026-01-01").exists(), "别的应用的日志不能碰");
        assert!(dir.join("mine-notes.txt").exists(), "不含 .log 的文件不能碰");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
