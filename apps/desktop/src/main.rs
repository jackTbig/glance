//! translate-tool 桌面端入口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use translate_tool::config::Config;
use translate_tool::ui::App;

fn init_logging(dir: &std::path::Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs).ok()?;
    let file = tracing_appender::rolling::daily(&logs, "translate-tool.log");
    let (nb, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(nb)
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .init();
    Some(guard)
}

fn main() -> eframe::Result<()> {
    let dir = Config::dir();
    let _guard = init_logging(&dir);
    tracing::info!("translate-tool {} 启动，数据目录 {}", env!("CARGO_PKG_VERSION"), dir.display());

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("配置读取失败，用默认配置：{e}");
            Config::default()
        }
    };

    // 根视口是那个平时缩到屏幕外、需要时变成浮窗的窗口
    let viewport = egui::ViewportBuilder::default()
        .with_title("translate-tool")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_inner_size([1.0, 1.0])
        .with_position([-32000.0, -32000.0]);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow, // wgpu 在本机 Arc 上不给带 alpha 的合成模式，透明窗会变黑底
        ..Default::default()
    };

    eframe::run_native(
        "translate-tool",
        options,
        Box::new(move |cc| match App::new(cc, cfg) {
            Ok(a) => Ok(Box::new(a) as Box<dyn eframe::App>),
            Err(e) => {
                tracing::error!("启动失败：{e}");
                Err(e.into())
            }
        }),
    )
}
