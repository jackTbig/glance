#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crossbeam_channel::bounded;
use live_caption::asr::{spawn_asr, AsrConfig};
use live_caption::audio::capture::spawn_capture;
use live_caption::config::Config;
use live_caption::segmenter::{spawn_segmenter, SegMsg, SegmenterConfig};
use live_caption::translate::{spawn_translator, TranslatorConfig};
use live_caption::ui::CaptionApp;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn init_logging(dir: &str) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(dir)?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(7)
        .filename_prefix("live-caption")
        .filename_suffix("log")
        .build(dir)?;
    let (nb, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_writer(nb)
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    Ok(guard)
}

fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    let _guard = init_logging(&Config::resolve(&cfg.paths.logs_dir).to_string_lossy())?;
    tracing::info!("启动，配置文件 {}", Config::path().display());

    let (tx_audio, rx_audio) = bounded::<Vec<f32>>(256);
    let (tx_seg, rx_seg) = bounded::<SegMsg>(16);
    let (tx_tr, rx_tr) = bounded(64);
    let (tx_ui, rx_ui) = bounded(1024);
    let (tx_cfg, rx_cfg) = bounded::<TranslatorConfig>(4);
    let paused = Arc::new(AtomicBool::new(false));
    let final_pending = Arc::new(AtomicBool::new(false));

    spawn_capture(tx_audio, tx_ui.clone());
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
        final_pending.clone(),
        tx_ui.clone(),
    );
    spawn_asr(
        AsrConfig {
            model_path: cfg.asr.model_path.clone(),
            language: cfg.asr.language.clone(),
            threads: cfg.asr.threads,
            flash_attn: cfg.asr.flash_attn,
            min_audio_ctx: cfg.asr.min_audio_ctx,
        },
        rx_seg,
        tx_ui.clone(),
        tx_tr,
        final_pending,
    );
    spawn_translator(TranslatorConfig::from(&cfg.translate), rx_cfg, rx_tr, tx_ui.clone());

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
        Box::new(move |cc| Ok(Box::new(CaptionApp::new(cc, rx_ui, cfg, paused_ui, tx_cfg)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
