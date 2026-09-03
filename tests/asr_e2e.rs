use live_caption::asr::{Asr, AsrConfig};
use std::sync::Mutex;

const MODEL: &str = r"D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin";

/// ggml 的 Vulkan 后端不能在两个线程里同时初始化两个上下文（会 ACCESS_VIOLATION），测试串行化
static GPU_LOCK: Mutex<()> = Mutex::new(());

#[test]
#[ignore = "需要模型与 GPU：cargo test --release --test asr_e2e -- --ignored"]
fn transcribes_english_fixture() {
    let _g = GPU_LOCK.lock().unwrap();
    let mut reader = hound::WavReader::open("tests/fixtures/en-10s.wav").unwrap();
    let pcm: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    let mut asr = Asr::load(&AsrConfig { model_path: MODEL.into(), language: "en".into(), threads: 4 }).unwrap();
    let text = asr.transcribe(&pcm).unwrap().to_lowercase();
    assert!(text.contains("roadmap"), "text={text}");
    assert!(text.contains("gpu"), "text={text}");
}

#[test]
#[ignore = "需要模型与 GPU"]
fn short_audio_is_padded_not_error() {
    let _g = GPU_LOCK.lock().unwrap();
    let mut asr = Asr::load(&AsrConfig { model_path: MODEL.into(), language: "en".into(), threads: 4 }).unwrap();
    let half_sec = vec![0.0f32; 8000];
    assert!(asr.transcribe(&half_sec).is_ok());
}
