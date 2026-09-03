use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub audio: AudioCfg,
    pub asr: AsrCfg,
    pub translate: TranslateCfg,
    pub ui: UiCfg,
    pub paths: PathsCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioCfg {
    /// RMS 能量门限（0–1）
    pub energy_threshold: f32,
    pub silence_ms: u32,
    pub min_segment_ms: u32,
    pub max_segment_ms: u32,
}
impl Default for AudioCfg {
    fn default() -> Self {
        Self { energy_threshold: 0.01, silence_ms: 500, min_segment_ms: 1000, max_segment_ms: 8000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrCfg {
    pub model_path: String,
    pub language: String,
    pub threads: i32,
    pub flash_attn: bool,
    /// audio_ctx 下限（1500 = 30 s 全窗；越小越快、越容易漏字）
    pub min_audio_ctx: i32,
}
impl Default for AsrCfg {
    fn default() -> Self {
        Self {
            model_path: r"D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin".into(),
            language: "en".into(),
            threads: 4,
            flash_attn: true,
            min_audio_ctx: 512,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslateCfg {
    /// claude 可执行文件完整路径
    pub claude_bin: String,
    pub model: String,
    pub timeout_secs: u64,
    pub rotate_every: u32,
    pub max_backlog: usize,
}
impl Default for TranslateCfg {
    fn default() -> Self {
        Self {
            claude_bin: r"C:\Users\13601\.local\bin\claude.exe".into(),
            model: "sonnet".into(),
            timeout_secs: 15,
            rotate_every: 60,
            max_backlog: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    pub font_size: f32,
    pub opacity: f32,
    pub font_path: String,
    /// 窗口外框位置与大小（屏幕像素）；x/y 为 -1 表示首次启动用系统默认位置
    pub x: i32,
    pub y: i32,
    pub width: u32,
    /// 最小高度；实际高度随内容在 [height, max_height] 内自动伸缩（下边缘固定向上长）
    pub height: u32,
    pub max_height: u32,
    /// 同时显示的最近段数
    pub lines: usize,
}
impl Default for UiCfg {
    fn default() -> Self {
        Self {
            font_size: 22.0,
            opacity: 0.75,
            font_path: r"C:\Windows\Fonts\msyh.ttc".into(),
            x: -1,
            y: -1,
            width: 1100,
            height: 120,
            max_height: 480,
            lines: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsCfg {
    pub transcripts_dir: String,
    pub logs_dir: String,
}
impl Default for PathsCfg {
    fn default() -> Self {
        Self { transcripts_dir: "transcripts".into(), logs_dir: "logs".into() }
    }
}

impl Config {
    /// 工作目录下的 config.toml 优先（cargo run 时），否则 exe 同目录
    pub fn path() -> PathBuf {
        let cwd = PathBuf::from("config.toml");
        if cwd.exists() {
            return cwd;
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join("config.toml")
    }

    pub fn load() -> anyhow::Result<Config> {
        let p = Self::path();
        if p.exists() {
            let s = std::fs::read_to_string(&p)?;
            Ok(toml::from_str(&s)?)
        } else {
            let cfg = Config::default();
            cfg.save()?;
            Ok(cfg)
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::write(Self::path(), toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip() {
        let cfg = Config::default();
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.audio.silence_ms, 500);
        assert_eq!(back.asr.language, "en");
        assert_eq!(back.translate.rotate_every, 60);
        assert!((back.ui.opacity - 0.75).abs() < 1e-6);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[ui]\nfont_size = 30.0\n").unwrap();
        assert_eq!(cfg.ui.font_size, 30.0);
        assert_eq!(cfg.audio.max_segment_ms, 8000);
    }
}
