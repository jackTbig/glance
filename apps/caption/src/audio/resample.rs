/// 交错多声道 → 单声道（各声道平均）
pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// 线性插值重采样，跨块保持相位连续。
/// 坐标系：索引 0 = 上一块最后一个样本（若有），1.. = 本块样本；
/// `pos` 是下一个输出点在该坐标系里的位置，处理完一块后把坐标系平移到本块末样本。
pub struct Resampler {
    step: f64,
    pos: f64,
    last: Option<f32>,
}

impl Resampler {
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        Self { step: from_hz as f64 / to_hz as f64, pos: 0.0, last: None }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.step - 1.0).abs() < 1e-12 {
            return input.to_vec();
        }
        if input.is_empty() {
            return Vec::new();
        }
        let offset = if self.last.is_some() { 1 } else { 0 };
        let total = input.len() + offset;
        let last = self.last;
        let sample = |i: usize| -> f32 {
            if offset == 1 && i == 0 {
                last.unwrap()
            } else {
                input[i - offset]
            }
        };
        let mut out = Vec::with_capacity(input.len() / self.step as usize + 2);
        // 只在能做插值（i+1 存在）时产出；余下位置留到下一块
        while self.pos < (total - 1) as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = sample(i);
            let b = sample(i + 1);
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= (total - 1) as f64;
        self.last = Some(*input.last().unwrap());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        let stereo = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(downmix_to_mono(&stereo, 2), vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn resample_48k_to_16k_length() {
        let mut r = Resampler::new(48000, 16000);
        let out = r.process(&vec![0.25; 4800]);
        assert!((1599..=1601).contains(&out.len()), "len={}", out.len());
        assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-6));
    }

    #[test]
    fn resample_is_continuous_across_chunks() {
        // 两块各 48 个样本 vs 一块 96 个，结果应一致
        let ramp: Vec<f32> = (0..96).map(|i| i as f32).collect();
        let mut a = Resampler::new(48000, 16000);
        let whole = a.process(&ramp);
        let mut b = Resampler::new(48000, 16000);
        let mut parts = b.process(&ramp[..48]);
        parts.extend(b.process(&ramp[48..]));
        assert_eq!(whole.len(), parts.len());
        for (x, y) in whole.iter().zip(parts.iter()) {
            assert!((x - y).abs() < 1e-4, "{x} vs {y}");
        }
    }

    #[test]
    fn same_rate_passthrough() {
        let mut r = Resampler::new(16000, 16000);
        assert_eq!(r.process(&[1.0, 2.0, 3.0]), vec![1.0, 2.0, 3.0]);
    }
}
