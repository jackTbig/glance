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
    /// 中间识别只解最近这么长的窗口（前面已定的文字用分段时间戳固定），0 = 每次重解整句
    pub partial_window_ms: u32,
    /// 中间识别专用的小模型（如 small.en）；None 或与主模型相同则复用主模型
    pub partial_model_path: Option<String>,
}

pub struct Asr {
    _ctx: WhisperContext,
    state: WhisperState,
    /// 中间识别用的第二套模型（更小更快）
    partial: Option<(WhisperContext, WhisperState)>,
    cfg: AsrConfig,
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

// 不再把上一句当 initial_prompt 喂给 whisper（2026-09-06 日志坐实：提示词让 small.en 的中间识别 6 s 音频只吐一个
// "Please"、让 turbo 定稿吞掉开头几个词甚至 8 s 音频只出 "So."）。两个模型都裸跑，标点与大小写靠模型自身。

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
        let mut asr = Self { _ctx: ctx, state, partial, cfg: cfg.clone() };
        // 预热：首次推理要编译 Vulkan shader、分配显存，实测比稳态慢 5 s 以上，提前在加载阶段付掉
        let t0 = std::time::Instant::now();
        let _ = asr.transcribe(&vec![0.0f32; MIN_SAMPLES])?;
        let _ = asr.transcribe_partial(&vec![0.0f32; MIN_SAMPLES], &mut PartialWindow::default());
        tracing::info!("asr 预热完成 {}ms", t0.elapsed().as_millis());
        Ok(asr)
    }

    /// 最终段识别（主模型）
    pub fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        run_with(&mut self.state, &self.cfg, pcm)
    }

    /// 中间快照识别（小模型，没有就用主模型），`pcm` 是这句到目前为止的全部音频。
    /// `partial_window_ms > 0` 时只解最近一个窗口：窗口超长就带分段时间戳解一次，把结束在最后 1.5 s 之前的分段
    /// 记为"已定"（文字与对应音频长度存在 `win` 里，切点贴到附近最轻的一帧），下一次从已定处往后解——
    /// 长句的每次中间识别耗时不再随句长增长，且同一段音频不会被解两遍（不会产生接缝重复）。
    /// 注意不能挂 whisper 的 abort 回调：Vulkan 后端只要设了回调解码就返回 -6（实测，与返回值无关）。
    pub fn transcribe_partial(&mut self, pcm: &[f32], win: &mut PartialWindow) -> anyhow::Result<String> {
        let cfg = &self.cfg;
        let state = match self.partial.as_mut() {
            Some((_, st)) => st,
            None => &mut self.state,
        };
        let window_samples = cfg.partial_window_ms as usize * 16;
        if window_samples == 0 {
            return run_with(state, cfg, pcm);
        }
        let start = win.committed_samples.min(pcm.len());
        let window = &pcm[start..];
        if window.len() <= window_samples {
            let text = run_with(state, cfg, window)?;
            return Ok(collapse_repeats(&join_text(&win.committed_text, &strip_overlap(&win.committed_text, &text))));
        }
        let segs = run_segments(state, cfg, window, true)?;
        let limit_cs = (window.len().saturating_sub(PARTIAL_KEEP_SAMPLES) / 160) as i64;
        let (committed, end_cs, rest) = split_committable(&segs, limit_cs);
        if end_cs > 0 {
            // 分段时间戳有 ±0.1–0.2 s 误差，切点贴到附近最轻的一帧，别切在词中间
            let approx = (end_cs as usize) * 160;
            let lo = approx.saturating_sub(SNAP_BEFORE_SAMPLES);
            let hi = (approx + SNAP_AFTER_SAMPLES).min(window.len());
            let cut = crate::segmenter::quietest_frame_end(window, 320, lo, hi).min(window.len());
            win.committed_samples = start + cut;
            win.committed_text = join_text(&win.committed_text, &strip_overlap(&win.committed_text, &committed.join(" ")));
        }
        Ok(collapse_repeats(&join_text(&win.committed_text, &strip_overlap(&win.committed_text, &rest.join(" ")))))
    }
}

/// 滑窗中间识别：最后这么长的音频不"定"，留给后续快照修正
const PARTIAL_KEEP_SAMPLES: usize = 16000 * 3 / 2;
/// 切点在分段时间戳附近找最轻的一帧：往前最多 300 ms、往后最多 100 ms（时间戳偏晚会切进下一个词，宁可偏早——重叠的词有接缝去重兜底）
const SNAP_BEFORE_SAMPLES: usize = 16000 * 30 / 100;
const SNAP_AFTER_SAMPLES: usize = 16000 * 10 / 100;

/// 滑窗中间识别的状态：一个话轮里已经"定下"的前半段文字与对应的音频长度
#[derive(Debug, Default, Clone)]
pub struct PartialWindow {
    pub id: SegId,
    pub committed_text: String,
    pub committed_samples: usize,
}

impl PartialWindow {
    /// 换了话轮就清零
    pub fn reset_for(&mut self, id: SegId) {
        if self.id != id {
            *self = PartialWindow { id, ..Default::default() };
        }
    }
}

/// 接缝去重：切点贴帧后下一窗可能把已定尾部的一两个词再解一遍，把 `new` 开头与 `committed` 结尾重复的词（≤4 个）去掉。
/// 比较时忽略大小写与标点。
fn strip_overlap(committed: &str, new: &str) -> String {
    let norm = |w: &str| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let old_words: Vec<&str> = committed.split_whitespace().collect();
    let new_words: Vec<&str> = new.split_whitespace().collect();
    let max = old_words.len().min(new_words.len()).min(4);
    let mut drop = 0;
    for k in (1..=max).rev() {
        let tail = &old_words[old_words.len() - k..];
        let head = &new_words[..k];
        if tail.iter().zip(head).all(|(a, b)| norm(a) == norm(b) && !norm(a).is_empty()) {
            drop = k;
            break;
        }
    }
    new_words[drop..].join(" ")
}

fn join_text(a: &str, b: &str) -> String {
    match (a.trim(), b.trim()) {
        ("", b) => b.to_string(),
        (a, "") => a.to_string(),
        (a, b) => format!("{a} {b}"),
    }
}

/// 窗口超长时的切分：结束时间 ≤ limit 的分段按顺序进"已定"，但只定到最后一个**以标点结尾**的分段为止——
/// 从句边界处才有真正的停顿、时间戳也准；词中间的分段边界（"quarterly | roadmap"）时间戳会偏进下一个词。
/// 遇到第一个超过 limit 的分段就停，其余（含没到标点的尾巴）留在窗口里。返回（已定的文字，已定到的时间 cs，留在窗口里的文字）
fn split_committable(segs: &[(i64, i64, String)], limit_cs: i64) -> (Vec<String>, i64, Vec<String>) {
    let mut committed = Vec::new();
    let mut end = 0;
    let mut pending: Vec<String> = Vec::new();
    let mut rest = Vec::new();
    for (_, t1, text) in segs {
        if !rest.is_empty() || *t1 <= 0 || *t1 > limit_cs {
            rest.push(text.clone());
            continue;
        }
        pending.push(text.clone());
        if text.trim_end().ends_with(['.', ',', '!', '?', ';', ':']) {
            committed.append(&mut pending);
            end = *t1;
        }
    }
    pending.append(&mut rest);
    (committed, end, pending)
}

fn run_with(state: &mut WhisperState, cfg: &AsrConfig, pcm: &[f32]) -> anyhow::Result<String> {
    let segs = run_segments(state, cfg, pcm, false)?;
    let parts: Vec<String> = segs.into_iter().map(|(_, _, t)| t).collect();
    Ok(collapse_repeats(parts.join(" ").trim()))
}

/// 跑一次 whisper，返回各分段（起止时间 cs，文字）。`timestamps` 为真时开分段/词级时间戳并按 `max_len` 拆短分段，
/// 供滑窗提交用；平时关掉（关掉更快，也不改变原有识别结果）。
fn run_segments(state: &mut WhisperState, cfg: &AsrConfig, pcm: &[f32], timestamps: bool) -> anyhow::Result<Vec<(i64, i64, String)>> {
    let mut audio = pcm.to_vec();
    if audio.len() < MIN_SAMPLES {
        audio.resize(MIN_SAMPLES, 0.0);
    }

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(&cfg.language));
    params.set_translate(false);
    params.set_no_timestamps(!timestamps);
    if timestamps {
        params.set_token_timestamps(true);
        params.set_max_len(40);
        // 只在词边界拆分段：否则 max_len 会把 "roadmap" 拆成 "road" + "map" 两段，切点落进词里
        params.set_split_on_word(true);
    }
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_n_threads(cfg.threads);
    params.set_audio_ctx(audio_ctx_for(audio.len(), cfg.min_audio_ctx));

    state.full(params, &audio).map_err(|e| anyhow::anyhow!("full: {e:?}"))?;
    let n = state.full_n_segments();
    let mut out = Vec::new();
    for i in 0..n {
        if let Some(seg) = state.get_segment(i) {
            let text = seg.to_str_lossy().map_err(|e| anyhow::anyhow!("{e:?}"))?.trim().to_string();
            if !text.is_empty() {
                out.push((seg.start_timestamp(), seg.end_timestamp(), text));
            }
        }
    }
    Ok(out)
}

fn has_words(t: &str) -> bool {
    t.chars().any(|c| c.is_alphanumeric())
}

/// whisper 对重复音节会把同一子句连吐几遍（日志实例："O , that's just O ." ×3）：
/// 按 . ! ? 切成子句，连续相同的（忽略大小写、空白与标点）只留第一句。单词级重复是正常口语，不碰。
fn collapse_repeats(text: &str) -> String {
    let mut clauses: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?') {
            clauses.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        clauses.push(cur);
    }
    let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect::<String>();
    let mut out: Vec<String> = Vec::new();
    let mut last_key = String::new();
    for cl in clauses {
        let k = key(&cl);
        if !k.is_empty() && k == last_key {
            continue;
        }
        last_key = k;
        out.push(cl);
    }
    out.concat().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 解一份中间快照：有词就发界面与临时翻译（临时翻译通道满了就丢，它只要最新一条）
fn decode_partial(asr: &mut Asr, pcm: &[f32], id: SegId, win: &mut PartialWindow, ui: &Sender<UiMsg>, tx_partial: &Sender<(SegId, String)>) {
    win.reset_for(id);
    let t0 = std::time::Instant::now();
    match asr.transcribe_partial(pcm, win) {
        Ok(text) if has_words(&text) => {
            tracing::info!("asr partial {}ms audio={:.1}s text={text:?}", t0.elapsed().as_millis(), pcm.len() as f32 / 16000.0);
            let _ = ui.send(UiMsg::EnglishPartial { id, text: text.clone() });
            let _ = tx_partial.try_send((id, text));
        }
        Ok(_) => {}
        Err(e) => tracing::info!("asr partial 放弃 {}ms：{e}", t0.elapsed().as_millis()),
    }
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
    /// 定稿沿用当前话轮的 id（没有中间结果就新分配）；之后的中间结果属于下一话轮
    pub(crate) fn final_id(&mut self) -> SegId {
        self.cur.take().unwrap_or_else(|| self.alloc())
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
            let mut win = PartialWindow::default();
            while let Ok(first) = rx_seg.recv() {
                let mut batch = vec![first];
                while let Ok(m) = rx_seg.try_recv() {
                    batch.push(m);
                }
                let finals: Vec<Vec<f32>> = batch.iter().filter_map(|m| if let SegMsg::Final(v) = m { Some(v.clone()) } else { None }).collect();
                if finals.is_empty() {
                    // 只有中间快照：识别最新的一份
                    let Some(SegMsg::Partial(pcm)) = batch.pop() else { continue };
                    let id = ids.partial_id();
                    decode_partial(&mut asr, &pcm, id, &mut win, &ui, &tx_partial);
                    continue;
                }
                // 队尾若是下一句的中间快照（切句器发完 Final 才会再发 Partial），让它插队先解：
                // 否则新句子的英文要排在上一句约 1 s 的定稿解码后面。id 仍按消息顺序分配。
                let trailing = match batch.last() {
                    Some(SegMsg::Partial(p)) => Some(p.clone()),
                    _ => None,
                };
                let final_ids: Vec<SegId> = finals.iter().map(|_| ids.final_id()).collect();
                if let Some(pcm) = trailing {
                    let id = ids.partial_id();
                    decode_partial(&mut asr, &pcm, id, &mut win, &ui, &tx_partial);
                }
                for (seg, id) in finals.into_iter().zip(final_ids) {
                    let at = Local::now();
                    let t0 = std::time::Instant::now();
                    match asr.transcribe(&seg) {
                        Ok(text) if has_words(&text) => {
                            tracing::info!("asr {}ms audio={:.1}s text={text:?}", t0.elapsed().as_millis(), seg.len() as f32 / 16000.0);
                            let _ = ui.send(UiMsg::English { id, at, text: text.clone() });
                            // 满了由翻译线程按积压规则处理；这里不阻塞
                            let _ = tx_tr.try_send((id, text));
                        }
                        Ok(_) => {
                            // 这句识别为空（多半是噪声）：把这个话轮可能挂着的中间结果清掉
                            let _ = ui.send(UiMsg::EnglishPartial { id, text: String::new() });
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
    use super::audio_ctx_for;

    #[test]
    fn collapse_repeats_folds_consecutive_identical_clauses() {
        use super::collapse_repeats;
        assert_eq!(collapse_repeats("O , that's just O . O , that's just O . O , that's just O ."), "O , that's just O .");
        assert_eq!(collapse_repeats("Thank you. Thank you. Thank you."), "Thank you.");
        assert_eq!(collapse_repeats("If n is a thousand, if n is a million, it doesn't matter."), "If n is a thousand, if n is a million, it doesn't matter.", "不同的子句不动");
        assert_eq!(collapse_repeats("Good morning everyone!"), "Good morning everyone!");
        assert_eq!(collapse_repeats("Many, many approaches to doing this."), "Many, many approaches to doing this.", "单词级的重复是正常口语，不折叠");
        assert_eq!(collapse_repeats("So we say fuzzy. so we say fuzzy? Because"), "So we say fuzzy. Because", "大小写与标点不同也算同一子句");
    }

    #[test]
    fn split_committable_takes_leading_segments_before_limit_in_order() {
        use super::split_committable;
        let segs = vec![(0, 120, "Good morning".to_string()), (120, 260, "everyone,".to_string()), (260, 410, "today we".to_string()), (410, 520, "will".to_string())];
        // 上限 300 cs：前两段已定，第三段起留在窗口
        let (c, end, rest) = split_committable(&segs, 300);
        assert_eq!(c, vec!["Good morning", "everyone,"]);
        assert_eq!(end, 260);
        assert_eq!(rest, vec!["today we", "will"]);
        // 第一段就超限：什么都不定
        let (c, end, rest) = split_committable(&segs, 100);
        assert!(c.is_empty() && end == 0 && rest.len() == 4);
        // 中间有个超限的分段：之后的即使更短也不跳着定（保持顺序）
        let odd = vec![(0, 100, "a.".to_string()), (100, 400, "b".to_string()), (400, 450, "c".to_string())];
        let (c, end, rest) = split_committable(&odd, 300);
        assert_eq!((c, end, rest), (vec!["a.".to_string()], 100, vec!["b".to_string(), "c".to_string()]));
        // 只定到最后一个以标点结尾的分段：没到标点的尾巴留在窗口里
        let np = vec![(0, 100, "Good morning".to_string()), (100, 200, "everyone,".to_string()), (200, 280, "today we".to_string())];
        let (c, end, rest) = split_committable(&np, 300);
        assert_eq!((c, end, rest), (vec!["Good morning".to_string(), "everyone,".to_string()], 200, vec!["today we".to_string()]));
        let none = vec![(0, 100, "Good morning".to_string()), (100, 200, "everyone".to_string())];
        assert_eq!(split_committable(&none, 300).1, 0, "一路没有标点就不定");
    }

    #[test]
    fn strip_overlap_drops_repeated_seam_words() {
        use super::strip_overlap;
        assert_eq!(strip_overlap("today we will", "today we will review the map"), "review the map");
        assert_eq!(strip_overlap("Good morning everyone,", "Everyone. Today we"), "Today we", "忽略大小写与标点");
        assert_eq!(strip_overlap("quarterly roadmap,", "discuss the migration"), "discuss the migration", "没有重叠不动");
        assert_eq!(strip_overlap("", "hello"), "hello");
        assert_eq!(strip_overlap("a b c d e", "b c d e f"), "f", "最多 4 词");
    }

    #[test]
    fn join_text_skips_empty_sides() {
        use super::join_text;
        assert_eq!(join_text("", "b"), "b");
        assert_eq!(join_text("a ", ""), "a");
        assert_eq!(join_text("a", " b"), "a b");
    }

    #[test]
    fn partial_window_resets_only_on_new_utterance() {
        let mut w = super::PartialWindow { id: 3, committed_text: "x".into(), committed_samples: 100 };
        w.reset_for(3);
        assert_eq!(w.committed_samples, 100);
        w.reset_for(4);
        assert_eq!((w.id, w.committed_samples, w.committed_text.as_str()), (4, 0, ""));
    }

    #[test]
    fn id_alloc_shares_id_between_partials_and_final() {
        let mut a = super::IdAlloc::default();
        assert_eq!(a.partial_id(), 1);
        assert_eq!(a.partial_id(), 1, "同一话轮的中间结果共用 id");
        assert_eq!(a.final_id(), 1, "定稿沿用");
        assert_eq!(a.partial_id(), 2, "下一话轮新 id");
        assert_eq!(a.final_id(), 2);
        assert_eq!(a.final_id(), 3, "没有中间结果直接定稿也能分配，id 不复用");
        // 插队场景：批里先有第 4 句的定稿、再有第 5 句的中间结果——按消息顺序分配，定稿先拿 id
        assert_eq!(a.partial_id(), 4);
        let final_id = a.final_id();
        let next_partial = a.partial_id();
        assert_eq!((final_id, next_partial), (4, 5));
    }

    #[test]
    fn audio_ctx_scales_with_length_and_clamps() {
        assert_eq!(audio_ctx_for(16000 * 2, 512), 512, "2 s 取下限");
        assert_eq!(audio_ctx_for(16000 * 15, 512), 750, "15 s 取一半");
        assert_eq!(audio_ctx_for(16000 * 40, 512), 1500, "超 30 s 封顶");
        assert_eq!(audio_ctx_for(16000 * 2, 256), 256, "下限可配");
    }

}
