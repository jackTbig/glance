//! 用法：cargo run --release --example asr_bench -- <模型.bin> <wav 16k mono，16 位整型或 32 位浮点>
use live_caption::asr::{Asr, AsrConfig};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let model = args.get(1).expect("模型路径").clone();
    let wav = args.get(2).expect("wav 路径").clone();

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
    let mut asr = Asr::load(&AsrConfig { model_path: model, language: "en".into(), threads: 4 })?;
    let load_ms = t0.elapsed().as_millis();

    // 预热一次
    let _ = asr.transcribe(&pcm)?;

    let t1 = Instant::now();
    let text = asr.transcribe(&pcm)?;
    let infer_ms = t1.elapsed().as_millis();

    println!(
        "load_ms={load_ms} audio_sec={audio_sec:.1} infer_ms={infer_ms} rtf={:.2}",
        infer_ms as f32 / 1000.0 / audio_sec
    );
    println!("text={text}");
    Ok(())
}
