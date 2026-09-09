#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crossbeam_channel::bounded;
use live_caption::asr::{spawn_asr, AsrConfig};
use live_caption::audio::capture::spawn_capture;
use live_caption::config::Config;
use live_caption::messages::SegId;
use live_caption::segmenter::{spawn_segmenter, SegMsg, SegmenterConfig};
use live_caption::translate::{new_context, spawn_partial_translator, spawn_translator, TranslatorConfig};
use live_caption::control::{spawn_control, spawn_heartbeat, Bridge};
use live_caption::managed::{exit_code_now, Args, Link};
use live_caption::ui::CaptionApp;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let args = Args::parse(std::env::args());
    // 日志前缀按托管与否分开：被简译拉起时若和独立实例同名，两个进程同一天会
    // 打开同一个文件、各持一个句柄交错写，滚动时还互抢，日志基本没法看。
    let prefix = if args.managed { "live-caption-managed" } else { "live-caption" };
    let mut cfg = Config::load()?;
    let logs = Config::resolve(&cfg.paths.logs_dir);
    desktop_shell::logging::init(logs.parent().unwrap_or(&logs), prefix, "LIVE_CAPTION_LOG", 7);
    tracing::info!("启动，配置文件 {}，托管={}", Config::path().display(), args.managed);

    // 单实例闸。托管实例与独立实例各用各的名字：用户手动开着一个、简译又拉起一个时，
    // 两个进程会同时每秒写同一份 config.toml 和同一批日志。
    let lock_name = if args.managed { "live-caption-managed-single" } else { "live-caption-single" };
    if !desktop_shell::single_instance::claim(lock_name) {
        tracing::warn!("已有一个字幕实例在运行（{lock_name}），本次启动退出");
        return Ok(());
    }
    // 重装到别的目录（"仅我用"↔"所有用户"、换盘）后，配置里写死的模型路径会失效，这里自动改指到 exe 旁的 models 目录
    match cfg.heal_model_paths() {
        Ok(fixes) => {
            for f in &fixes {
                tracing::warn!("配置自愈：{f}");
            }
        }
        Err(e) => tracing::warn!("配置自愈后写回失败：{e}"),
    }

    let (tx_audio, rx_audio) = bounded::<Vec<f32>>(256);
    let (tx_seg, rx_seg) = bounded::<(SegMsg, std::time::Instant)>(16);
    let (tx_tr, rx_tr) = bounded(64);
    let (tx_ui, rx_ui) = bounded(1024);
    let (tx_cfg, rx_cfg) = bounded::<TranslatorConfig>(4);
    let (tx_partial, rx_partial) = bounded::<(SegId, String)>(4);
    let (tx_cfg2, rx_cfg2) = bounded::<TranslatorConfig>(4);
    let context = new_context();
    let paused = Arc::new(AtomicBool::new(false));

    spawn_capture(tx_audio, tx_ui.clone(), cfg.audio.device.clone());
    spawn_segmenter(
        SegmenterConfig {
            sample_rate: 16000,
            frame_ms: 20,
            energy_threshold: cfg.audio.energy_threshold,
            silence_ms: cfg.audio.silence_ms,
            min_segment_ms: cfg.audio.min_segment_ms,
            max_segment_ms: cfg.audio.max_segment_ms,
            partial_every_ms: cfg.audio.partial_every_ms,
        },
        rx_audio,
        tx_seg,
        paused.clone(),
        tx_ui.clone(),
    );
    spawn_asr(
        AsrConfig {
            model_path: cfg.asr.model_path.clone(),
            language: cfg.asr.language.clone(),
            threads: cfg.asr.threads,
            flash_attn: cfg.asr.flash_attn,
            min_audio_ctx: cfg.asr.min_audio_ctx,
            partial_window_ms: cfg.asr.partial_window_ms,
            partial_model_path: Some(cfg.asr.partial_model_path.clone()).filter(|p| !p.trim().is_empty()),
        },
        rx_seg,
        tx_ui.clone(),
        tx_tr,
        tx_partial,
    );
    spawn_translator(TranslatorConfig::from(&cfg.translate), rx_cfg, rx_tr, tx_ui.clone(), context.clone());
    spawn_partial_translator(TranslatorConfig::from(&cfg.translate), rx_cfg2, rx_partial, tx_ui.clone(), context);

    // 托管链路：控制端点（父 → 子）与心跳（子 → 父）方向相反，都只走 127.0.0.1
    let link = Link::new(args.clone());
    let (bridge, rx_cmd) = if args.managed {
        let (tx, rx) = bounded(16);
        let b = Bridge::new(tx);
        spawn_control(b.clone(), args.control_port);
        spawn_heartbeat(link.clone());
        (Some(b), Some(rx))
    } else {
        (None, None)
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("live-caption")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_inner_size([cfg.ui.width as f32, cfg.ui.height as f32]);
    if cfg.ui.x >= 0 && cfg.ui.y >= 0 {
        viewport = viewport.with_position([cfg.ui.x as f32, cfg.ui.y as f32]);
    }
    let options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Glow, ..Default::default() };
    let paused_ui = paused.clone();
    eframe::run_native(
        "live-caption",
        options,
        Box::new(move |cc| Ok(Box::new(CaptionApp::new(cc, rx_ui, cfg, paused_ui, vec![tx_cfg, tx_cfg2], link, rx_cmd, bridge)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    // 退出码告诉父进程这次是「用户退出」还是「请求重启」，不区分的话
    // 改完识别参数的主动重启会被当成崩溃，父进程进重试风暴。
    let code = exit_code_now();
    if code != 0 {
        tracing::info!("以退出码 {code} 退出（请求父进程重启）");
        std::process::exit(code);
    }
    Ok(())
}
