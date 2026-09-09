//! translate-tool 桌面端入口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use translate_tool::config::Config;
use translate_tool::ui::App;

/// 日志按天切分，保留 7 天。
///
/// 用同步写而不是 tracing_appender::non_blocking：这个应用日志量很小（启动几行、每次翻译一行），
/// 非阻塞写的 BufWriter 要攒满才落盘，结果是跑了半天日志文件还是 0 字节，排障时什么也看不到。
fn init_logging(dir: &std::path::Path) {
    let logs = dir.join("logs");
    if std::fs::create_dir_all(&logs).is_err() {
        return;
    }
    prune_old_logs(&logs, 7);
    let file = tracing_appender::rolling::daily(&logs, "translate-tool.log");
    // 日志级别只认自己的 TRANSLATE_TOOL_LOG，**故意不读 RUST_LOG**：
    // 本机有一个用户级 RUST_LOG=error，读它的话应用日志会被静默压成 0 字节，排障时什么都看不到。
    // RUST_LOG 是给开发工具链用的通用变量，面向用户的桌面应用不该被它左右。
    //
    // 过滤串一律用 try_new：EnvFilter::new 遇到解析不了的指令会静默丢掉，全丢光时一条都不写。
    let spec = std::env::var("TRANSLATE_TOOL_LOG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "info".into());
    let filter = tracing_subscriber::EnvFilter::try_new(&spec).unwrap_or_else(|e| {
        eprintln!("TRANSLATE_TOOL_LOG={spec:?} 解析失败（{e}），退回 info");
        tracing_subscriber::EnvFilter::new("info")
    });
    tracing_subscriber::fmt().with_writer(file).with_ansi(false).with_env_filter(filter).init();
}

/// 删掉超过 keep_days 天没动过的日志
fn prune_old_logs(dir: &std::path::Path, keep_days: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days * 86_400);
    for e in entries.flatten() {
        let is_log = e.file_name().to_string_lossy().contains("translate-tool.log");
        let old = e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false);
        if is_log && old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// 已经有一个实例在跑就返回 false。
///
/// 没有这道闸的后果实测过：第二个实例的三个全局热键全部注册失败（HotKey already registered），
/// 本地服务端口也被占，用户看到的是「装好了但快捷键没反应」。
#[cfg(windows)]
fn claim_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "Global\\translate-tool-single-instance".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        // 句柄故意不关：进程退出时系统自动释放
        !h.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

#[cfg(not(windows))]
fn claim_single_instance() -> bool {
    true
}

fn main() -> eframe::Result<()> {
    let dir = Config::dir();
    init_logging(&dir);
    if !claim_single_instance() {
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
