//! 探针：中间识别（带中止回调）与最终识别在同一段音频上各跑 3 次，看带回调是否会失败。
//! 用法：cargo run --release --example partial_probe -- <模型.bin> <wav 16k mono>
use live_caption::asr::{Asr, AsrConfig};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut reader = hound::WavReader::open(&args[2])?;
    let pcm: Vec<f32> = match reader.spec().sample_format {
        hound::SampleFormat::Int => reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
    };
    let mut asr = Asr::load(&AsrConfig { model_path: args[1].clone(), language: "en".into(), threads: 4, flash_attn: false, min_audio_ctx: 512, partial_window_ms: 0, partial_model_path: std::env::args().nth(3) })?;
    for i in 0..3 {
        let t = std::time::Instant::now();
        match asr.transcribe_partial(&pcm, &mut live_caption::asr::PartialWindow::default()) {
            Ok(text) => println!("partial#{i} ok {}ms {text:?}", t.elapsed().as_millis()),
            Err(e) => println!("partial#{i} ERR {}ms {e}", t.elapsed().as_millis()),
        }
        let t = std::time::Instant::now();
        match asr.transcribe(&pcm) {
            Ok(text) => println!("final#{i}   ok {}ms {text:?}", t.elapsed().as_millis()),
            Err(e) => println!("final#{i}   ERR {}ms {e}", t.elapsed().as_millis()),
        }
    }
    Ok(())
}
