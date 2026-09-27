//! 翻译延迟归因探针（Jack 2026-09-27 问：是服务商慢还是系统提示词大）。
//!
//! 用配置里真实的服务端跑四组对照：
//! - overhead：完整系统提示词 + 一个字的用户消息 ≈ 连接+预填充+最短生成的固定开销
//! - full    ：完整系统提示词 + 5 行待翻文本，非流式（截屏覆盖现在走的就是这条）
//! - min     ：一句话系统提示词 + 同样的 5 行 → 与 full 的差就是系统提示词的代价
//! - stream  ：完整系统提示词 + 5 行，流式 → 首字节时间 vs 总时间，看生成占多少
//!
//! 每组跑两遍取第二遍（第一遍含 TLS 握手）。用法：
//! `cargo run --release -p ai-translate --example latency_probe`

use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let cfg_path = std::env::var("APPDATA")? + r"\translate-tool\config.toml";
    let raw = std::fs::read_to_string(&cfg_path)?;
    let get = |key: &str| -> String {
        raw.lines()
            .find_map(|l| {
                let l = l.trim();
                let rest = l.strip_prefix(key)?;
                let rest = rest.trim().strip_prefix('=')?;
                Some(rest.trim().trim_matches('"').to_string())
            })
            .unwrap_or_default()
    };
    let base = get("openai_base_url");
    let key = get("openai_api_key");
    let model = get("openai_model");
    anyhow::ensure!(!base.is_empty() && !key.is_empty(), "配置里缺 base 或 key");

    use ai_translate::{prompts, Direction, Scene};
    let dir = Direction::ToChinese;
    let sys_full = prompts::system_prompt(Scene::Screenshot, None, dir);
    let sys_min = "你是专业翻译。把用户给的内容翻译成简体中文。只输出译文。".to_string();
    let lines = [
        "K8s API VIP 由 10.0.0.10 换到 10.0.0.11，",
        "keepalived 优先级调到 200，证书轮换已完成，",
        "apiserver 就绪探针通过，三台节点都没有重启，",
        "CMDB 同步延迟了五分钟，等变更窗口结束再核对，",
        "回滚演练的记录见附件三。",
    ];
    let mut numbered = String::from("请翻译以下各段：");
    for (i, l) in lines.iter().enumerate() {
        numbered.push_str(&format!("\n⟨{}⟩ {l}", i + 1));
    }

    println!("服务端 {base} 模型 {model}");
    println!("系统提示词：完整 {} 字 / 极简 {} 字；用户消息 {} 字", sys_full.chars().count(), sys_min.chars().count(), numbered.chars().count());

    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(60)).build()?;
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));

    let run = |sys: &str, user: &str, stream: bool| -> anyhow::Result<(Duration, Duration)> {
        let body = serde_json::json!({
            "model": model,
            "stream": stream,
            "messages": [
                {"role": "system", "content": sys},
                {"role": "user", "content": user},
            ],
        });
        let t0 = Instant::now();
        let resp = client.post(&url).bearer_auth(&key).json(&body).send()?;
        let mut first = Duration::ZERO;
        if stream {
            use std::io::Read;
            let mut mutt = resp;
            let mut chunk = [0u8; 4096];
            while mutt.read(&mut chunk).unwrap_or(0) > 0 {
                if first.is_zero() {
                    first = t0.elapsed();
                }
            }
        } else {
            let _ = resp.text()?;
        }
        Ok((first, t0.elapsed()))
    };

    for (name, sys, user, stream) in [
        ("overhead", sys_full.as_str(), "好", false),
        ("full    ", sys_full.as_str(), numbered.as_str(), false),
        ("min     ", sys_min.as_str(), numbered.as_str(), false),
        ("stream  ", sys_full.as_str(), numbered.as_str(), true),
    ] {
        let mut last = (Duration::ZERO, Duration::ZERO);
        for round in 0..2 {
            match run(sys, user, stream) {
                Ok(t) => last = t,
                Err(e) => {
                    println!("{name} 第{}轮失败：{e}", round + 1);
                    return Ok(());
                }
            }
        }
        if stream {
            println!("{name} 首字节 {:>6} ms  总 {:>6} ms（第二遍）", last.0.as_millis(), last.1.as_millis());
        } else {
            println!("{name} 总 {:>6} ms（第二遍）", last.1.as_millis());
        }
    }
    Ok(())
}
