use crate::messages::{SegId, Stage, StageState, UiMsg};
use chrono::Local;
use crossbeam_channel::{Receiver, Sender};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

#[derive(Debug, Clone)]
pub struct AsrConfig {
    pub model_path: String,
    pub language: String,
    pub threads: i32,
}

pub struct Asr {
    _ctx: WhisperContext,
    state: WhisperState,
    cfg: AsrConfig,
    prev_text: String,
}

/// whisper.cpp 要求输入 ≥ 1 s，不足补零到 1.2 s
const MIN_SAMPLES: usize = 16000 * 12 / 10;

/// whisper 编码器默认按 30 s 窗（1500 个 audio ctx）算，短段也要付 30 s 的编码开销；
/// 按实际时长收缩 audio_ctx 可把每段识别从固定约 3.5 s 压下来（whisper.cpp stream 例程同款做法）。
/// 下限留 512 保证识别质量。
fn audio_ctx_for(samples: usize) -> i32 {
    let sec = samples as f32 / 16000.0;
    let ctx = (sec / 30.0 * 1500.0).ceil() as i32;
    ctx.clamp(512, 1500)
}

impl Asr {
    pub fn load(cfg: &AsrConfig) -> anyhow::Result<Self> {
        let mut cparams = WhisperContextParameters::default();
        cparams.flash_attn = true;
        let ctx = WhisperContext::new_with_params(&cfg.model_path, cparams)
            .map_err(|e| anyhow::anyhow!("加载模型失败 {}: {e:?}", cfg.model_path))?;
        let state = ctx.create_state().map_err(|e| anyhow::anyhow!("create_state: {e:?}"))?;
        Ok(Self { _ctx: ctx, state, cfg: cfg.clone(), prev_text: String::new() })
    }

    pub fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        let mut audio = pcm.to_vec();
        if audio.len() < MIN_SAMPLES {
            audio.resize(MIN_SAMPLES, 0.0);
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.cfg.language));
        params.set_translate(false);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_n_threads(self.cfg.threads);
        params.set_audio_ctx(audio_ctx_for(audio.len()));
        if !self.prev_text.is_empty() {
            params.set_initial_prompt(&self.prev_text);
        }

        self.state.full(params, &audio).map_err(|e| anyhow::anyhow!("full: {e:?}"))?;
        let n = self.state.full_n_segments();
        let mut parts = Vec::new();
        for i in 0..n {
            if let Some(seg) = self.state.get_segment(i) {
                parts.push(seg.to_str_lossy().map_err(|e| anyhow::anyhow!("{e:?}"))?.trim().to_string());
            }
        }
        let text = parts.join(" ").trim().to_string();
        if text.chars().any(|c| c.is_alphanumeric()) {
            // 只留最后约 200 个字符做下一段的提示词
            let tail: String = text.chars().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
            self.prev_text = tail;
        }
        Ok(text)
    }
}

/// 识别线程：rx_seg 收音频段；ui 收英文；tx_tr 送翻译队列
pub fn spawn_asr(
    cfg: AsrConfig,
    rx_seg: Receiver<Vec<f32>>,
    ui: Sender<UiMsg>,
    tx_tr: Sender<(SegId, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("asr".into())
        .spawn(move || {
            let mut asr = match Asr::load(&cfg) {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!("{e}");
                    let _ = ui.send(UiMsg::Status { stage: Stage::Asr, state: StageState::Err(e.to_string()) });
                    return;
                }
            };
            let _ = ui.send(UiMsg::Status { stage: Stage::Asr, state: StageState::Ok });
            let mut next_id: SegId = 1;
            while let Ok(seg) = rx_seg.recv() {
                let at = Local::now();
                let t0 = std::time::Instant::now();
                match asr.transcribe(&seg) {
                    Ok(text) if text.chars().any(|c| c.is_alphanumeric()) => {
                        tracing::info!(
                            "asr {}ms audio={:.1}s text={text:?}",
                            t0.elapsed().as_millis(),
                            seg.len() as f32 / 16000.0
                        );
                        let id = next_id;
                        next_id += 1;
                        let _ = ui.send(UiMsg::English { id, at, text: text.clone() });
                        // 满了由翻译线程按积压规则处理；这里不阻塞
                        let _ = tx_tr.try_send((id, text));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("识别失败，跳过：{e}");
                    }
                }
            }
        })
        .expect("spawn asr")
}

#[cfg(test)]
mod tests {
    use super::audio_ctx_for;

    #[test]
    fn audio_ctx_scales_with_length_and_clamps() {
        assert_eq!(audio_ctx_for(16000 * 2), 512, "2 s 取下限");
        assert_eq!(audio_ctx_for(16000 * 15), 750, "15 s 取一半");
        assert_eq!(audio_ctx_for(16000 * 40), 1500, "超 30 s 封顶");
    }
}
