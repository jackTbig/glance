//! codex CLI：每句一个 `codex exec`，无流式
use crate::backend::BackendConfig;
use crate::util::resolve_bin;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn codex_translate(cfg: &BackendConfig, system_prompt: &str, user_message: &str) -> anyhow::Result<String> {
    let bin = resolve_bin(&cfg.codex_bin);
    let tmp = std::env::temp_dir();
    let out = tmp.join(format!("ai-translate-codex-{}-{:?}.txt", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_file(&out);
    let mut cmd = Command::new(&bin);
    cmd.args(["exec", "--skip-git-repo-check", "-s", "read-only", "-C"]).arg(&tmp).arg("-o").arg(&out);
    if !cfg.codex_model.trim().is_empty() {
        cmd.args(["-m", cfg.codex_model.trim()]);
    }
    // prompt 走标准输入而不是命令行参数：Windows 上 codex 是 .cmd，
    // Rust 1.77+ 对批处理文件的参数做严格校验，含中文/换行的长 prompt 会被拒（batch file arguments are invalid）
    cmd.arg("-").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 {bin} 失败：{e}"))?;
    {
        use std::io::Write;
        let mut si = child.stdin.take().ok_or_else(|| anyhow::anyhow!("拿不到 codex 的标准输入"))?;
        si.write_all(format!("{system_prompt}\n\n{user_message}").as_bytes())?;
    }
    let deadline = Instant::now() + cfg.timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                anyhow::bail!("codex exec 退出码 {status}");
            }
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("超时");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = std::fs::read_to_string(&out).map_err(|e| anyhow::anyhow!("codex 没有写出结果：{e}"))?;
    let _ = std::fs::remove_file(&out);
    Ok(s.trim().to_string())
}
