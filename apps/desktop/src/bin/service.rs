//! 只跑本地 HTTP 服务、不开界面的入口，给端到端测试与排障用。
//!
//! 用法：translate-tool-service [--port N] [--base-url URL] [--key K] [--model M]
//! 不给参数就用正式配置（%APPDATA%\translate-tool\config.toml）。
use ai_translate::Translator;
use std::sync::{Arc, Mutex};
use translate_tool::config::Config;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let mut cfg = Config::load().unwrap_or_default();
    if let Some(p) = arg(&args, "--port").and_then(|p| p.parse().ok()) {
        cfg.server.port = p;
    }
    if let Some(u) = arg(&args, "--base-url") {
        cfg.backend.openai_base_url = u;
    }
    if let Some(k) = arg(&args, "--key") {
        cfg.backend.openai_api_key = k;
    }
    if let Some(m) = arg(&args, "--model") {
        cfg.backend.openai_model = m;
    }

    let tr = Arc::new(Mutex::new(Translator::new(cfg.translator_config())?));
    let h = translate_tool::server::spawn(
        cfg.server.port,
        cfg.server.allowed_extension_ids.clone(),
        tr,
        env!("CARGO_PKG_VERSION"),
    )?;
    println!("本地服务已启动：http://127.0.0.1:{}", h.port);
    println!("后端 {} 模型 {}", cfg.backend.openai_base_url, cfg.backend.openai_model);
    loop {
        std::thread::park();
    }
}
