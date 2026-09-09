use live_caption::asr::{Asr, AsrConfig};
use std::sync::Mutex;

const MODEL: &str = r"D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin";

/// ggml 的 Vulkan 后端不能在两个线程里同时初始化两个上下文（会 ACCESS_VIOLATION），测试串行化
static GPU_LOCK: Mutex<()> = Mutex::new(());

fn cfg() -> AsrConfig {
    AsrConfig { model_path: MODEL.into(), language: "en".into(), threads: 4, flash_attn: false, min_audio_ctx: 512, partial_window_ms: 0, partial_model_path: None }
}

#[test]
#[ignore = "需要模型与 GPU：cargo test --release --test asr_e2e -- --ignored"]
fn transcribes_english_fixture() {
    let _g = GPU_LOCK.lock().unwrap();
    let mut reader = hound::WavReader::open("tests/fixtures/en-10s.wav").unwrap();
    let pcm: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    let mut asr = Asr::load(&cfg()).unwrap();
    let text = asr.transcribe(&pcm).unwrap().to_lowercase();
    assert!(text.contains("roadmap"), "text={text}");
    assert!(text.contains("gpu"), "text={text}");
}

#[test]
#[ignore = "需要模型与 GPU"]
fn windowed_partial_decode_keeps_all_words_and_advances_commit() {
    use live_caption::asr::PartialWindow;
    let _g = GPU_LOCK.lock().unwrap();
    let mut reader = hound::WavReader::open("tests/fixtures/en-10s.wav").unwrap();
    let pcm: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    let mut c = cfg();
    c.partial_window_ms = 4000;
    let mut asr = Asr::load(&c).unwrap();
    let full = asr.transcribe(&pcm).unwrap().to_lowercase();
    // 模拟切句器：每 0.3 s 一份到目前为止的全部音频
    let mut win = PartialWindow::default();
    let mut shown = String::new();
    let mut committed_seen = Vec::new();
    let step = 16000 * 3 / 10;
    let mut n = step;
    while n <= pcm.len() {
        shown = asr.transcribe_partial(&pcm[..n], &mut win).unwrap();
        committed_seen.push(win.committed_samples);
        n += step;
    }
    let shown = shown.to_lowercase();
    assert!(*committed_seen.last().unwrap() > 0, "10 s 音频、4 s 窗口，应有已定部分：{committed_seen:?}");
    assert!(committed_seen.windows(2).all(|w| w[1] >= w[0]), "已定位置只前进不后退");
    for key in ["good morning", "roadmap", "data center", "gpu"] {
        assert!(shown.contains(key), "滑窗结果缺 {key:?}：shown={shown} full={full}");
    }
    let words: Vec<&str> = shown.split_whitespace().collect();
    let dup = words.windows(2).filter(|w| w[0] == w[1] && w[0].len() > 3).count();
    assert_eq!(dup, 0, "窗口接缝不该出现连续重复词：{shown}");
}

#[test]
#[ignore = "需要模型与 GPU"]
fn short_audio_is_padded_not_error() {
    let _g = GPU_LOCK.lock().unwrap();
    let mut asr = Asr::load(&cfg()).unwrap();
    let half_sec = vec![0.0f32; 8000];
    assert!(asr.transcribe(&half_sec).is_ok());
}
