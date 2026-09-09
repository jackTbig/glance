//! translate-tool 桌面端入口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use translate_tool::config::Config;
use translate_tool::ui::App;

/// 日志文件名前缀与级别变量。两个应用的前缀必须不同，否则同一天会写进同一个文件互相抢。
const LOG_PREFIX: &str = "translate-tool";
const LOG_ENV: &str = "TRANSLATE_TOOL_LOG";

fn main() -> eframe::Result<()> {
    let dir = Config::dir();
    desktop_shell::logging::init(&dir, LOG_PREFIX, LOG_ENV, 7);
    if !desktop_shell::single_instance::claim("translate-tool-single-instance") {
        tracing::warn!("已有一个简译在运行，本次启动退出");
        return Ok(());
    }
    tracing::info!("{} {} 启动，数据目录 {}", translate_tool::ui::APP_NAME_ZH, env!("CARGO_PKG_VERSION"), dir.display());

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("配置读取失败，用默认配置：{e}");
            Config::default()
        }
    };

    // 根视口是那个平时缩到屏幕外、需要时变成浮窗的窗口
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(translate_tool::ui::APP_NAME_ZH)
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_inner_size([1.0, 1.0])
        .with_position([-32000.0, -32000.0]);
    // 标题栏与任务栏的小图标
    match translate_tool::tray::icon_rgba() {
        Ok((rgba, w, h)) => viewport = viewport.with_icon(egui::IconData { rgba, width: w, height: h }),
        Err(e) => tracing::warn!("窗口图标加载失败：{e}"),
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow, // wgpu 在本机 Arc 上不给带 alpha 的合成模式，透明窗会变黑底
        ..Default::default()
    };

    eframe::run_native(
        translate_tool::ui::APP_NAME_ZH,
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
