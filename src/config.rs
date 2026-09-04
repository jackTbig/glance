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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioCfg {
    /// RMS 能量门限（0–1）
    pub energy_threshold: f32,
    pub silence_ms: u32,
    pub min_segment_ms: u32,
    pub max_segment_ms: u32,
    /// 说话过程中每累积多少毫秒发一次中间识别（0 = 关闭逐字上屏）
    pub partial_every_ms: u32,
}
impl Default for AudioCfg {
    fn default() -> Self {
        Self { energy_threshold: 0.01, silence_ms: 500, min_segment_ms: 1000, max_segment_ms: 8000, partial_every_ms: 800 }
    }
}

/// exe 所在目录（安装版的模型放在 exe 旁的 models\ 里）
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn first_model_in(dir: &Path) -> Option<PathBuf> {
    let mut bins: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |x| x == "bin"))
        .collect();
    bins.sort();
    // 优先 turbo（最准），其次任意一个
    bins.iter().find(|p| p.to_string_lossy().contains("turbo")).cloned().or_else(|| bins.first().cloned())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AsrCfg {
    pub model_path: String,
    /// 设置界面列出这个目录下的 *.bin 供选择
    pub models_dir: String,
    pub language: String,
    pub threads: i32,
    pub flash_attn: bool,
    /// audio_ctx 下限（1500 = 30 s 全窗；越小越快、越容易漏字）
    pub min_audio_ctx: i32,
}
impl Default for AsrCfg {
    fn default() -> Self {
        // 安装版：exe\models\ 里有模型就用它；开发机：退回 local-asr 的 q5 与 D:\models\whisper
        let bundled = exe_dir().join("models");
        let (model_path, models_dir) = match first_model_in(&bundled) {
            Some(m) => (m.to_string_lossy().into_owned(), bundled.to_string_lossy().into_owned()),
            None => (r"D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin".into(), r"D:\models\whisper".into()),
        };
        Self { model_path, models_dir, language: "en".into(), threads: 4, flash_attn: false, min_audio_ctx: 512 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TranslateCfg {
    /// claude | codex | openai
    pub backend: String,
    /// claude 可执行文件；写 "claude" 即按 PATH 与常见安装位置自动找
    pub claude_bin: String,
    #[serde(alias = "model")]
    pub claude_model: String,
    /// codex 可执行文件（PATH 里的 codex 或完整路径）
    pub codex_bin: String,
    /// 留空用 codex 默认模型
    pub codex_model: String,
    /// OpenAI 兼容接口：base_url 后拼 /chat/completions；DeepSeek 填 https://api.deepseek.com，
    /// 通义千问填 https://dashscope.aliyuncs.com/compatible-mode/v1
    pub openai_base_url: String,
    pub openai_api_key: String,
    pub openai_model: String,
    pub timeout_secs: u64,
    pub rotate_every: u32,
    pub max_backlog: usize,
}
impl Default for TranslateCfg {
    fn default() -> Self {
        Self {
            backend: "claude".into(),
            claude_bin: "claude".into(),
            claude_model: "sonnet".into(),
            codex_bin: "codex".into(),
            codex_model: String::new(),
            openai_base_url: "https://api.deepseek.com".into(),
            openai_api_key: String::new(),
            openai_model: "deepseek-chat".into(),
            timeout_secs: 15,
            rotate_every: 60,
            max_backlog: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiCfg {
    pub font_size: f32,
    pub opacity: f32,
    /// 中文字体；找不到时依次退回微软雅黑 / 黑体 / 宋体
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
    /// 打字机效果：每秒显示多少字符（0 = 整句直接出现）
    pub typewriter_cps: f32,
    /// 全局热键（改后重启生效）：切回可点击 / 切换穿透。写法如 "Ctrl+Alt+C"、"Ctrl+Shift+F9"
    pub hotkey_clickable: String,
    pub hotkey_passthrough: String,
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
            typewriter_cps: 60.0,
            hotkey_clickable: "Ctrl+Alt+C".into(),
            hotkey_passthrough: "Ctrl+Alt+P".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PathsCfg {
    /// 相对路径以配置文件所在目录为基准
    pub transcripts_dir: String,
    pub logs_dir: String,
}
impl Default for PathsCfg {
    fn default() -> Self {
        Self { transcripts_dir: "transcripts".into(), logs_dir: "logs".into() }
    }
}

impl Config {
    /// 数据目录：开发模式（工作目录有 config.toml）就是工作目录；
    /// 安装版用 %APPDATA%\live-caption（Program Files 不可写）
    pub fn dir() -> PathBuf {
        if Path::new("config.toml").exists() {
            return PathBuf::from(".");
        }
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("live-caption")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// 把配置里的相对路径解析到数据目录下
    pub fn resolve(p: &str) -> PathBuf {
        let pb = PathBuf::from(p);
        if pb.is_absolute() {
            pb
        } else {
            Self::dir().join(pb)
        }
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
        std::fs::create_dir_all(Self::dir())?;
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
        assert_eq!(back.translate.backend, "claude");
        assert!((back.ui.opacity - 0.75).abs() < 1e-6);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[ui]\nfont_size = 30.0\n").unwrap();
        assert_eq!(cfg.ui.font_size, 30.0);
        assert_eq!(cfg.audio.max_segment_ms, 8000);
    }

    #[test]
    fn old_model_key_still_reads_into_claude_model() {
        let cfg: Config = toml::from_str("[translate]\nmodel = \"haiku\"\n").unwrap();
        assert_eq!(cfg.translate.claude_model, "haiku");
    }

    #[test]
    fn resolve_keeps_absolute_and_anchors_relative() {
        assert_eq!(Config::resolve(r"D:\x\y"), PathBuf::from(r"D:\x\y"));
        assert!(Config::resolve("logs").ends_with("logs"));
    }
}
