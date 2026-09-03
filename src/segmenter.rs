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
}

impl Segmenter {
    pub fn new(cfg: SegmenterConfig) -> Self {
        let frame_len = (cfg.sample_rate * cfg.frame_ms / 1000) as usize;
        Self { cfg, frame_len, pending: Vec::new(), buf: Vec::new(), in_speech: false, silence_acc_ms: 0 }
    }

    fn ms_to_samples(&self, ms: u32) -> usize {
        (self.cfg.sample_rate * ms / 1000) as usize
    }

    /// 喂入任意长度样本，返回本次完成的段
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
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
                }
                self.silence_acc_ms = 0;
                self.buf.extend_from_slice(&frame);
                if self.buf.len() >= self.ms_to_samples(self.cfg.max_segment_ms) {
                    out.push(std::mem::take(&mut self.buf));
                    self.in_speech = false;
                }
            } else if self.in_speech {
                self.buf.extend_from_slice(&frame);
                self.silence_acc_ms += self.cfg.frame_ms;
                if self.silence_acc_ms >= self.cfg.silence_ms {
                    if let Some(seg) = self.finish() {
                        out.push(seg);
                    }
                }
            }
            start += self.frame_len;
        }
        self.pending.drain(..start);
        out
    }

    /// 没有音频到达时调用（等价于 elapsed_ms 的静音）
    pub fn idle(&mut self, elapsed_ms: u32) -> Option<Vec<f32>> {
        if !self.in_speech {
            return None;
        }
        self.silence_acc_ms += elapsed_ms;
        if self.silence_acc_ms >= self.cfg.silence_ms {
            self.finish()
        } else {
            None
        }
    }

    fn finish(&mut self) -> Option<Vec<f32>> {
        self.in_speech = false;
        self.silence_acc_ms = 0;
        let seg = std::mem::take(&mut self.buf);
        if seg.len() >= self.ms_to_samples(self.cfg.min_segment_ms) {
            Some(seg)
        } else {
            None
        }
    }
}

/// 切句线程：rx_audio 收 16 kHz 单声道块；tx_seg 发完成的段；paused 为真时丢弃音频
pub fn spawn_segmenter(
    cfg: SegmenterConfig,
    rx_audio: Receiver<Vec<f32>>,
    tx_seg: Sender<Vec<f32>>,
    paused: Arc<AtomicBool>,
    ui: Sender<UiMsg>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("segmenter".into())
        .spawn(move || {
            let mut seg = Segmenter::new(cfg);
            loop {
                match rx_audio.recv_timeout(Duration::from_millis(100)) {
                    Ok(chunk) => {
                        if paused.load(Ordering::Relaxed) {
                            continue;
                        }
                        for s in seg.push(&chunk) {
                            if tx_seg.send(s).is_err() {
                                return;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(s) = seg.idle(100) {
                            if tx_seg.send(s).is_err() {
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
        }
    }
    fn tone(ms: u32) -> Vec<f32> {
        let n = (16000 * ms / 1000) as usize;
        (0..n).map(|i| (i as f32 * 0.2).sin() * 0.3).collect()
    }
    fn silence(ms: u32) -> Vec<f32> {
        vec![0.0; (16000 * ms / 1000) as usize]
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
        let segs = s.push(&silence(600));
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
        let segs = s.push(&tone(10000));
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].len(), 16000 * 8);
    }

    #[test]
    fn idle_finishes_pending_speech() {
        let mut s = Segmenter::new(cfg());
        s.push(&tone(2000));
        assert!(s.idle(300).is_none());
        let seg = s.idle(300).expect("累计 600ms 静音应出段");
        assert!(seg.len() >= 16000 * 2);
    }
}
