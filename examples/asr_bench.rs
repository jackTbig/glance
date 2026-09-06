//! 识别基准。用法：
//!   cargo run --release --example asr_bench -- <模型.bin> <wav 16k mono> [threads=4] [flash_attn=1] [min_audio_ctx=512] [runs=3]
//! 打印每次推理毫秒（取最小/中位）与识别文本。
use live_caption::asr::{Asr, AsrConfig};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let model = args.get(1).expect("模型路径").clone();
    let wav = args.get(2).expect("wav 路径").clone();
    let threads: i32 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(4);
    let flash_attn = args.get(4).map(|s| s == "1").unwrap_or(true);
    let min_audio_ctx: i32 = args.get(5).map(|s| s.parse().unwrap()).unwrap_or(512);
    let runs: usize = args.get(6).map(|s| s.parse().unwrap()).unwrap_or(3);

    let mut reader = hound::WavReader::open(&wav)?;
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16000, "基准音频必须 16 kHz");
    assert_eq!(spec.channels, 1, "基准音频必须单声道");
    let pcm: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
    };
    let audio_sec = pcm.len() as f32 / 16000.0;

    let t0 = Instant::now();
    let mut asr = Asr::load(&AsrConfig { model_path: model.clone(), language: "en".into(), threads, flash_attn, min_audio_ctx, partial_window_ms: 0, partial_model_path: None })?;
    let load_ms = t0.elapsed().as_millis();

    let mut times = Vec::new();
    let mut text = String::new();
    for _ in 0..runs {
        let t1 = Instant::now();
        text = asr.transcribe(&pcm)?;
        times.push(t1.elapsed().as_millis());
    }
    times.sort();
    let name = std::path::Path::new(&model).file_name().unwrap().to_string_lossy();
    println!(
        "model={name} threads={threads} flash={flash_attn} min_ctx={min_audio_ctx} audio={audio_sec:.1}s load={load_ms}ms min={}ms median={}ms",
        times[0],
        times[times.len() / 2]
    );
    println!("text={text}");
    Ok(())
}
