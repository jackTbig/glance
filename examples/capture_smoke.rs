//! 放一段英文视频，然后 cargo run --release --example capture_smoke → target/smoke16k.wav
use crossbeam_channel::bounded;
use live_caption::audio::capture::{spawn_capture, TARGET_RATE};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let (tx, rx) = bounded::<Vec<f32>>(64);
    let (ui_tx, ui_rx) = bounded(64);
    let _h = spawn_capture(tx, ui_tx, String::new());
    let mut samples = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) {
            samples.extend(chunk);
        }
        while let Ok(m) = ui_rx.try_recv() {
            println!("{m:?}");
        }
    }
    let out = std::env::args().nth(1).unwrap_or_else(|| "smoke16k.wav".into());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&out, spec)?;
    for s in &samples {
        w.write_sample(*s)?;
    }
    w.finalize()?;
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!(
        "captured {:.1}s @16k mono, peak={peak:.3} -> {out}",
        samples.len() as f32 / TARGET_RATE as f32
    );
    Ok(())
}
