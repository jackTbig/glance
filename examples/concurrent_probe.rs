//! 探针：两个 whisper 上下文（turbo 定稿 + small.en 中间识别）分别在两个线程里同时推理，Vulkan 后端会不会出错、各自耗时比串行多多少。
//! 上下文先在主线程顺序初始化（交接卡：不能并发初始化），只并发 `full`。
//! 用法：cargo run --release --example concurrent_probe -- <主模型.bin> <小模型.bin> <wav 16k mono> [轮数]
use live_caption::asr::{Asr, AsrConfig};
use std::sync::{Arc, Barrier};
use std::time::Instant;

fn load(model: &str) -> anyhow::Result<Asr> {
    Asr::load(&AsrConfig { model_path: model.to_string(), language: "en".into(), threads: 4, flash_attn: false, min_audio_ctx: 512, partial_model_path: None })
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let rounds: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(5);
    let mut reader = hound::WavReader::open(&args[3])?;
    let pcm: Vec<f32> = match reader.spec().sample_format {
        hound::SampleFormat::Int => reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
    };
    // 中间识别通常只解最近几秒，取前 4 s 当中间识别的输入
    let short: Vec<f32> = pcm[..pcm.len().min(16000 * 4)].to_vec();
    let mut big = load(&args[1])?;
    let mut small = load(&args[2])?;

    println!("== 串行基准");
    let mut big_seq = Vec::new();
    let mut small_seq = Vec::new();
    for _ in 0..rounds {
        let t = Instant::now();
        big.transcribe(&pcm)?;
        big_seq.push(t.elapsed().as_millis());
        let t = Instant::now();
        small.transcribe(&short)?;
        small_seq.push(t.elapsed().as_millis());
    }
    println!("turbo 整段 {big_seq:?} ms\nsmall 4s {small_seq:?} ms");

    println!("== 并发：两个线程同时开跑 {rounds} 轮");
    let barrier = Arc::new(Barrier::new(2));
    let pcm_a = pcm.clone();
    let b1 = barrier.clone();
    let h_big = std::thread::spawn(move || -> anyhow::Result<Vec<u128>> {
        let mut v = Vec::new();
        for _ in 0..rounds {
            b1.wait();
            let t = Instant::now();
            let text = big.transcribe(&pcm_a)?;
            v.push(t.elapsed().as_millis());
            if text.trim().is_empty() {
                anyhow::bail!("turbo 并发时返回空文本");
            }
        }
        Ok(v)
    });
    let b2 = barrier.clone();
    let h_small = std::thread::spawn(move || -> anyhow::Result<Vec<u128>> {
        let mut v = Vec::new();
        for _ in 0..rounds {
            b2.wait();
            let t = Instant::now();
            let text = small.transcribe(&short)?;
            v.push(t.elapsed().as_millis());
            if text.trim().is_empty() {
                anyhow::bail!("small 并发时返回空文本");
            }
        }
        Ok(v)
    });
    let r_big = h_big.join().expect("turbo 线程 panic");
    let r_small = h_small.join().expect("small 线程 panic");
    match (&r_big, &r_small) {
        (Ok(b), Ok(s)) => {
            println!("turbo 并发 {b:?} ms\nsmall 并发 {s:?} ms");
            let med = |v: &Vec<u128>| {
                let mut x = v.clone();
                x.sort();
                x[x.len() / 2]
            };
            println!(
                "中位数：turbo 串行 {} → 并发 {}；small 串行 {} → 并发 {}",
                med(&big_seq),
                med(b),
                med(&small_seq),
                med(s)
            );
            println!("结论：并发推理正常");
        }
        _ => {
            println!("turbo: {r_big:?}\nsmall: {r_small:?}");
            println!("结论：并发推理失败");
        }
    }
    Ok(())
}
