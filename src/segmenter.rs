use crate::messages::{Stage, StageState, UiMsg};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    pub sample_rate: u32,
    pub frame_ms: u32,
    pub energy_threshold: f32,
    pub silence_ms: u32,
    pub min_segment_ms: u32,
    pub max_segment_ms: u32,
    /// 说话过程中每累积这么多毫秒新音频就发一次中间快照（0 = 关闭）
    pub partial_every_ms: u32,
}

/// 切句器输出：中间快照（当前这句到目前为止的全部音频）或最终段
#[derive(Debug, Clone, PartialEq)]
pub enum SegMsg {
    Partial(Vec<f32>),
    Final(Vec<f32>),
}

pub struct Segmenter {
    cfg: SegmenterConfig,
    frame_len: usize,
    /// 不足一帧的余数
    pending: Vec<f32>,
    /// 当前段
    buf: Vec<f32>,
    in_speech: bool,
    silence_acc_ms: u32,
    /// 上次发中间快照之后新增的样本数
    since_partial: usize,
}

impl Segmenter {
    pub fn new(cfg: SegmenterConfig) -> Self {
        let frame_len = (cfg.sample_rate * cfg.frame_ms / 1000) as usize;
        Self { cfg, frame_len, pending: Vec::new(), buf: Vec::new(), in_speech: false, silence_acc_ms: 0, since_partial: 0 }
    }

    fn ms_to_samples(&self, ms: u32) -> usize {
        (self.cfg.sample_rate * ms / 1000) as usize
    }

    /// 喂入任意长度样本，返回本次产生的消息（中间快照与最终段）
    pub fn push(&mut self, samples: &[f32]) -> Vec<SegMsg> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(samples);
        let mut start = 0;
        while start + self.frame_len <= self.pending.len() {
            let frame: Vec<f32> = self.pending[start..start + self.frame_len].to_vec();
            let rms = (frame.iter().map(|v| v * v).sum::<f32>() / frame.len() as f32).sqrt();
            let voiced = rms >= self.cfg.energy_threshold;
            if voiced {
                if !self.in_speech {
                    self.in_speech = true;
                    self.buf.clear();
                    self.since_partial = 0;
                }
                self.silence_acc_ms = 0;
                self.buf.extend_from_slice(&frame);
                self.since_partial += frame.len();
                if self.buf.len() >= self.ms_to_samples(self.cfg.max_segment_ms) {
                    out.push(SegMsg::Final(std::mem::take(&mut self.buf)));
                    self.in_speech = false;
                    self.since_partial = 0;
                } else if self.cfg.partial_every_ms > 0
                    && self.buf.len() >= self.ms_to_samples(self.cfg.min_segment_ms)
                    && self.since_partial >= self.ms_to_samples(self.cfg.partial_every_ms)
                {
                    out.push(SegMsg::Partial(self.buf.clone()));
                    self.since_partial = 0;
                }
            } else if self.in_speech {
                self.buf.extend_from_slice(&frame);
                self.silence_acc_ms += self.cfg.frame_ms;
                if self.silence_acc_ms >= self.cfg.silence_ms {
                    if let Some(seg) = self.finish() {
                        out.push(SegMsg::Final(seg));
                    }
                }
            }
            start += self.frame_len;
        }
        self.pending.drain(..start);
        out
    }

    /// 没有音频到达时调用（等价于 elapsed_ms 的静音）
    pub fn idle(&mut self, elapsed_ms: u32) -> Option<SegMsg> {
        if !self.in_speech {
            return None;
        }
        self.silence_acc_ms += elapsed_ms;
        if self.silence_acc_ms >= self.cfg.silence_ms {
            self.finish().map(SegMsg::Final)
        } else {
            None
        }
    }

    fn finish(&mut self) -> Option<Vec<f32>> {
        self.in_speech = false;
        self.silence_acc_ms = 0;
        self.since_partial = 0;
        let seg = std::mem::take(&mut self.buf);
        if seg.len() >= self.ms_to_samples(self.cfg.min_segment_ms) {
            Some(seg)
        } else {
            None
        }
    }
}

/// 切句线程：rx_audio 收 16 kHz 单声道块；tx_seg 发消息；paused 为真时丢弃音频
pub fn spawn_segmenter(
    cfg: SegmenterConfig,
    rx_audio: Receiver<Vec<f32>>,
    tx_seg: Sender<SegMsg>,
    paused: Arc<AtomicBool>,
    ui: Sender<UiMsg>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("segmenter".into())
        .spawn(move || {
            let mut seg = Segmenter::new(cfg);
            let send = |m: SegMsg| -> bool { tx_seg.send(m).is_ok() };
            loop {
                match rx_audio.recv_timeout(Duration::from_millis(100)) {
                    Ok(chunk) => {
                        if paused.load(Ordering::Relaxed) {
                            continue;
                        }
                        for m in seg.push(&chunk) {
                            if !send(m) {
                                return;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(m) = seg.idle(100) {
                            if !send(m) {
                                return;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = ui.send(UiMsg::Status {
                            stage: Stage::Capture,
                            state: StageState::Err("采集线程已退出".into()),
                        });
                        return;
                    }
                }
            }
        })
        .expect("spawn segmenter")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SegmenterConfig {
        SegmenterConfig {
            sample_rate: 16000,
            frame_ms: 20,
            energy_threshold: 0.01,
            silence_ms: 500,
            min_segment_ms: 1000,
            max_segment_ms: 8000,
            partial_every_ms: 0,
        }
    }
    fn tone(ms: u32) -> Vec<f32> {
        let n = (16000 * ms / 1000) as usize;
        (0..n).map(|i| (i as f32 * 0.2).sin() * 0.3).collect()
    }
    fn silence(ms: u32) -> Vec<f32> {
        vec![0.0; (16000 * ms / 1000) as usize]
    }
    fn finals(v: Vec<SegMsg>) -> Vec<Vec<f32>> {
        v.into_iter().filter_map(|m| if let SegMsg::Final(s) = m { Some(s) } else { None }).collect()
    }

    #[test]
    fn silence_only_emits_nothing() {
        let mut s = Segmenter::new(cfg());
        assert!(s.push(&silence(5000)).is_empty());
        assert!(s.idle(2000).is_none());
    }

    #[test]
    fn speech_then_silence_emits_one_segment() {
        let mut s = Segmenter::new(cfg());
        assert!(s.push(&tone(3000)).is_empty());
        let segs = finals(s.push(&silence(600)));
        assert_eq!(segs.len(), 1);
        let len_ms = segs[0].len() as u32 * 1000 / 16000;
        assert!((3400..=3600).contains(&len_ms), "len_ms={len_ms}");
    }

    #[test]
    fn short_burst_is_dropped() {
        let mut s = Segmenter::new(cfg());
        s.push(&tone(400));
        assert!(s.push(&silence(600)).is_empty());
    }

    #[test]
    fn long_speech_is_force_cut_at_max() {
        let mut s = Segmenter::new(cfg());
        let segs = finals(s.push(&tone(10000)));
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].len(), 16000 * 8);
    }

    #[test]
    fn idle_finishes_pending_speech() {
        let mut s = Segmenter::new(cfg());
        s.push(&tone(2000));
        assert!(s.idle(300).is_none());
        let seg = s.idle(300).expect("累计 600ms 静音应出段");
        assert!(matches!(seg, SegMsg::Final(ref v) if v.len() >= 16000 * 2));
    }

    #[test]
    fn partials_are_emitted_every_interval_then_final() {
        let mut c = cfg();
        c.partial_every_ms = 1000;
        let mut s = Segmenter::new(c);
        let msgs = s.push(&tone(3500));
        let partials: Vec<usize> = msgs.iter().filter_map(|m| if let SegMsg::Partial(v) = m { Some(v.len()) } else { None }).collect();
        assert_eq!(partials, vec![16000, 32000, 48000], "每 1 s 一次快照，且快照是到目前为止的全部音频");
        assert!(finals(msgs).is_empty());
        let end = s.push(&silence(600));
        assert_eq!(finals(end).len(), 1);
        // 新的一句从零开始计快照
        let again = s.push(&tone(1500));
        let p2: Vec<usize> = again.iter().filter_map(|m| if let SegMsg::Partial(v) = m { Some(v.len()) } else { None }).collect();
        assert_eq!(p2, vec![16000]);
    }
}
