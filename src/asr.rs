use crate::messages::{SegId, Stage, StageState, UiMsg};
use crate::segmenter::SegMsg;
use chrono::Local;
use crossbeam_channel::{Receiver, Sender};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

#[derive(Debug, Clone)]
pub struct AsrConfig {
    pub model_path: String,
    pub language: String,
    pub threads: i32,
    pub flash_attn: bool,
    /// audio_ctx 下限（whisper 编码窗 1500 = 30 s；越小越快、越容易漏字）
    pub min_audio_ctx: i32,
    /// 中间识别专用的小模型（如 small.en）；None 或与主模型相同则复用主模型
    pub partial_model_path: Option<String>,
}

pub struct Asr {
    _ctx: WhisperContext,
    state: WhisperState,
    /// 中间识别用的第二套模型（更小更快）
    partial: Option<(WhisperContext, WhisperState)>,
    cfg: AsrConfig,
    prev_text: String,
}

/// whisper.cpp 要求输入 ≥ 1 s，不足补零到 1.2 s
const MIN_SAMPLES: usize = 16000 * 12 / 10;

/// whisper 编码器默认按 30 s 窗（1500 个 audio ctx）算，短段也要付 30 s 的编码开销；
/// 按实际时长收缩 audio_ctx 可把每段识别时间压下来（whisper.cpp stream 例程同款做法）。
fn audio_ctx_for(samples: usize, min_ctx: i32) -> i32 {
    let sec = samples as f32 / 16000.0;
    let ctx = (sec / 30.0 * 1500.0).ceil() as i32;
    ctx.clamp(min_ctx.clamp(64, 1500), 1500)
}

/// 上一段文本作提示词时，whisper 会模仿它的风格：没标点、全小写的提示会让后面的段也丢标点。
/// 这里只留末尾约 200 字符，首字母大写、末尾补句号，把风格拉回"有标点的书面句"。
fn prompt_tail(text: &str) -> String {
    let tail: String = text.chars().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
    let tail = tail.trim();
    if tail.is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(tail.len() + 1);
    let mut chars = tail.chars();
    if let Some(first) = chars.next() {
        s.extend(first.to_uppercase());
        s.push_str(chars.as_str());
    }
    if !s.ends_with(['.', '!', '?']) {
        s.push('.');
    }
    s
}

impl Asr {
    pub fn load(cfg: &AsrConfig) -> anyhow::Result<Self> {
        let mut cparams = WhisperContextParameters::default();
        cparams.flash_attn = cfg.flash_attn;
        let ctx = WhisperContext::new_with_params(&cfg.model_path, cparams)
            .map_err(|e| anyhow::anyhow!("加载模型失败 {}: {e:?}", cfg.model_path))?;
        let state = ctx.create_state().map_err(|e| anyhow::anyhow!("create_state: {e:?}"))?;
        let partial = match cfg.partial_model_path.as_deref().map(str::trim).filter(|p| !p.is_empty() && *p != cfg.model_path.as_str()) {
            Some(p) => {
                let mut cp = WhisperContextParameters::default();
                cp.flash_attn = cfg.flash_attn;
                match WhisperContext::new_with_params(p, cp) {
                    Ok(c) => match c.create_state() {
                        Ok(st) => {
                            tracing::info!("中间识别模型：{p}");
                            Some((c, st))
                        }
                        Err(e) => {
                            tracing::warn!("中间识别模型 create_state 失败，改用主模型：{e:?}");
                            None
                        }
                    },
                    Err(e) => {
                        tracing::warn!("中间识别模型 {p} 加载失败，改用主模型：{e:?}");
                        None
                    }
                }
            }
            None => None,
        };
        let mut asr = Self { _ctx: ctx, state, partial, cfg: cfg.clone(), prev_text: String::new() };
        // 预热：首次推理要编译 Vulkan shader、分配显存，实测比稳态慢 5 s 以上，提前在加载阶段付掉
        let t0 = std::time::Instant::now();
        let _ = asr.transcribe(&vec![0.0f32; MIN_SAMPLES])?;
        let _ = asr.transcribe_partial(&vec![0.0f32; MIN_SAMPLES]);
        asr.prev_text.clear();
        tracing::info!("asr 预热完成 {}ms", t0.elapsed().as_millis());
        Ok(asr)
    }

    /// 最终段识别：结果进入下一段的提示词
    pub fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        let text = run_with(&mut self.state, &self.cfg, &self.prev_text, pcm)?;
        if text.chars().any(|c| c.is_alphanumeric()) {
            self.prev_text = prompt_tail(&text);
        }
        Ok(text)
    }

    /// 中间快照识别：不更新提示词。
    /// 注意不能挂 whisper 的 abort 回调：Vulkan 后端只要设了回调解码就返回 -6（实测，与返回值无关）；
    /// 中间识别本身只要 0.3–0.5 s，最终段最多多等这么久，不值得中止。
    pub fn transcribe_partial(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        match self.partial.as_mut() {
            Some((_, st)) => run_with(st, &self.cfg, &self.prev_text, pcm),
            None => run_with(&mut self.state, &self.cfg, &self.prev_text, pcm),
        }
    }
}

fn run_with(state: &mut WhisperState, cfg: &AsrConfig, prev_text: &str, pcm: &[f32]) -> anyhow::Result<String> {
    {
        let mut audio = pcm.to_vec();
        if audio.len() < MIN_SAMPLES {
            audio.resize(MIN_SAMPLES, 0.0);
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&cfg.language));
        params.set_translate(false);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_n_threads(cfg.threads);
        params.set_audio_ctx(audio_ctx_for(audio.len(), cfg.min_audio_ctx));
        if !prev_text.is_empty() {
            params.set_initial_prompt(prev_text);
        }

        state.full(params, &audio).map_err(|e| anyhow::anyhow!("full: {e:?}"))?;
        let n = state.full_n_segments();
        let mut parts = Vec::new();
        for i in 0..n {
            if let Some(seg) = state.get_segment(i) {
                parts.push(seg.to_str_lossy().map_err(|e| anyhow::anyhow!("{e:?}"))?.trim().to_string());
            }
        }
        Ok(parts.join(" ").trim().to_string())
    }
}

fn has_words(t: &str) -> bool {
    t.chars().any(|c| c.is_alphanumeric())
}

/// 话轮 id：第一个中间结果时分配，同话轮的中间结果与定稿共用；作废不复用
#[derive(Default)]
pub(crate) struct IdAlloc {
    next: SegId,
    cur: Option<SegId>,
}

impl IdAlloc {
    fn alloc(&mut self) -> SegId {
        self.next += 1;
        self.next
    }
    pub(crate) fn partial_id(&mut self) -> SegId {
        match self.cur {
            Some(i) => i,
            None => {
                let i = self.alloc();
                self.cur = Some(i);
                i
            }
        }
    }
    pub(crate) fn final_id(&mut self) -> SegId {
        self.cur.take().unwrap_or_else(|| self.alloc())
    }
    /// 定稿为空（噪声）：当前话轮作废
    pub(crate) fn discard(&mut self) {
        self.cur = None;
    }
}

/// 识别线程：rx_seg 收切句消息；ui 收英文（中间/最终）；tx_tr 送定稿翻译队列；tx_partial 送临时翻译（满了就丢）。
/// 队列里同时有 Final 与 Partial 时只处理 Final（Partial 作废）；只有 Partial 时取最新的一份。
pub fn spawn_asr(
    cfg: AsrConfig,
    rx_seg: Receiver<SegMsg>,
    ui: Sender<UiMsg>,
    tx_tr: Sender<(SegId, String)>,
    tx_partial: Sender<(SegId, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("asr".into())
        .spawn(move || {
            let _ = ui.send(UiMsg::Status { stage: Stage::Asr, state: StageState::Warn("模型加载中".into()) });
            let mut asr = match Asr::load(&cfg) {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!("{e}");
                    let _ = ui.send(UiMsg::Status { stage: Stage::Asr, state: StageState::Err(e.to_string()) });
                    return;
                }
            };
            let _ = ui.send(UiMsg::Status { stage: Stage::Asr, state: StageState::Ok });
            let mut ids = IdAlloc::default();
            while let Ok(first) = rx_seg.recv() {
                let mut batch = vec![first];
                while let Ok(m) = rx_seg.try_recv() {
                    batch.push(m);
                }
                let finals: Vec<Vec<f32>> = batch.iter().filter_map(|m| if let SegMsg::Final(v) = m { Some(v.clone()) } else { None }).collect();
                if finals.is_empty() {
                    // 只有中间快照：识别最新的一份
                    let Some(SegMsg::Partial(pcm)) = batch.pop() else { continue };
                    let t0 = std::time::Instant::now();
                    match asr.transcribe_partial(&pcm) {
                        Ok(text) if has_words(&text) => {
                            tracing::info!("asr partial {}ms audio={:.1}s text={text:?}", t0.elapsed().as_millis(), pcm.len() as f32 / 16000.0);
                            let id = ids.partial_id();
                            let _ = ui.send(UiMsg::EnglishPartial { id, text: text.clone() });
                            // 满了就丢：临时翻译线程只要最新一条
                            let _ = tx_partial.try_send((id, text));
                        }
                        Ok(_) => {}
                        Err(e) => tracing::info!("asr partial 放弃 {}ms：{e}", t0.elapsed().as_millis()),
                    }
                    continue;
                }
                for seg in finals {
                    let at = Local::now();
                    let t0 = std::time::Instant::now();
                    match asr.transcribe(&seg) {
                        Ok(text) if has_words(&text) => {
                            tracing::info!("asr {}ms audio={:.1}s text={text:?}", t0.elapsed().as_millis(), seg.len() as f32 / 16000.0);
                            let id = ids.final_id();
                            let _ = ui.send(UiMsg::English { id, at, text: text.clone() });
                            // 满了由翻译线程按积压规则处理；这里不阻塞
                            let _ = tx_tr.try_send((id, text));
                        }
                        Ok(_) => {
                            // 这句识别为空（多半是噪声）：把可能挂着的中间结果清掉，话轮作废
                            let _ = ui.send(UiMsg::EnglishPartial { id: 0, text: String::new() });
                            ids.discard();
                        }
                        Err(e) => tracing::warn!("识别失败，跳过：{e}"),
                    }
                }
            }
        })
        .expect("spawn asr")
}

#[cfg(test)]
mod tests {
    use super::{audio_ctx_for, prompt_tail};

    #[test]
    fn id_alloc_shares_id_between_partials_and_final() {
        let mut a = super::IdAlloc::default();
        assert_eq!(a.partial_id(), 1);
        assert_eq!(a.partial_id(), 1, "同一话轮的中间结果共用 id");
        assert_eq!(a.final_id(), 1, "定稿沿用");
        assert_eq!(a.partial_id(), 2, "下一话轮新 id");
        a.discard();
        assert_eq!(a.final_id(), 3, "作废的 id 不复用；没有中间结果直接定稿也能分配");
    }

    #[test]
    fn audio_ctx_scales_with_length_and_clamps() {
        assert_eq!(audio_ctx_for(16000 * 2, 512), 512, "2 s 取下限");
        assert_eq!(audio_ctx_for(16000 * 15, 512), 750, "15 s 取一半");
        assert_eq!(audio_ctx_for(16000 * 40, 512), 1500, "超 30 s 封顶");
        assert_eq!(audio_ctx_for(16000 * 2, 256), 256, "下限可配");
    }

    #[test]
    fn prompt_tail_restores_sentence_style() {
        assert_eq!(prompt_tail("good morning everyone"), "Good morning everyone.");
        assert_eq!(prompt_tail("Already fine!"), "Already fine!");
        assert_eq!(prompt_tail("   "), "");
        let long = "a".repeat(300);
        assert_eq!(prompt_tail(&long).chars().count(), 201, "200 字符 + 句号");
    }
}
