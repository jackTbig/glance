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
    /// 采集哪个播放设备的声音（友好名）；空 = 跟随系统默认播放设备（默认设备切换会自动跟过去）
    pub device: String,
}
impl Default for AudioCfg {
    fn default() -> Self {
        Self { energy_threshold: 0.01, silence_ms: 400, min_segment_ms: 600, max_segment_ms: 8000, partial_every_ms: 400, device: String::new() }
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
    /// 中间识别（逐字上屏）用的小模型，空 = 同主模型
    pub partial_model_path: String,
}
impl Default for AsrCfg {
    fn default() -> Self {
        // 安装版：exe\models\ 里有模型就用它；开发机：退回 local-asr 的 q5 与 D:\models\whisper
        let bundled = exe_dir().join("models");
        let (model_path, models_dir) = match first_model_in(&bundled) {
            Some(m) => (m.to_string_lossy().into_owned(), bundled.to_string_lossy().into_owned()),
            None => (r"D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin".into(), r"D:\models\whisper".into()),
        };
        // 中间识别默认用 small.en（约 0.2 s 一次）；找不到就空着（复用主模型）
        let small = Path::new(&models_dir).join("ggml-small.en.bin");
        let partial_model_path = if small.is_file() { small.to_string_lossy().into_owned() } else { String::new() };
        Self { model_path, models_dir, language: "en".into(), threads: 4, flash_attn: false, min_audio_ctx: 512, partial_model_path }
    }
}

impl AsrCfg {
    /// 安装目录变了（"仅我用"↔"所有用户"切换、换盘、改目录）之后，配置里写死的模型绝对路径会失效，
    /// 表现为"加载模型失败 InitError"、识别线程退出、设置窗模型下拉为空。
    /// 这里把失效路径改指到 `bundled`（exe 旁的 models\）里的同名文件；主模型同名不在就用目录里现有的；
    /// 小模型不在就清空（回落到主模型）。返回每处改动的说明，空 = 没动。`bundled` 不存在时什么都不做。
    pub fn heal_paths(&mut self, bundled: &Path) -> Vec<String> {
        let mut fixes = Vec::new();
        if !bundled.is_dir() {
            return fixes;
        }
        let same_name_in_bundled = |p: &str| -> Option<PathBuf> {
            let name = Path::new(p).file_name()?;
            let cand = bundled.join(name);
            cand.is_file().then_some(cand)
        };
        if !self.model_path.trim().is_empty() && !Path::new(&self.model_path).is_file() {
            if let Some(new) = same_name_in_bundled(&self.model_path).or_else(|| first_model_in(bundled)) {
                let new = new.to_string_lossy().into_owned();
                fixes.push(format!("主模型 {} 不存在，改用 {}", self.model_path, new));
                self.model_path = new;
            }
        }
        if !self.partial_model_path.trim().is_empty() && !Path::new(&self.partial_model_path).is_file() {
            match same_name_in_bundled(&self.partial_model_path) {
                Some(new) => {
                    let new = new.to_string_lossy().into_owned();
                    fixes.push(format!("中间识别模型 {} 不存在，改用 {}", self.partial_model_path, new));
                    self.partial_model_path = new;
                }
                None => {
                    fixes.push(format!("中间识别模型 {} 不存在且安装目录里没有，改为同主模型", self.partial_model_path));
                    self.partial_model_path.clear();
                }
            }
        }
        if !Path::new(&self.models_dir).is_dir() {
            let new = bundled.to_string_lossy().into_owned();
            fixes.push(format!("模型目录 {} 不存在，改用 {}", self.models_dir, new));
            self.models_dir = new;
        }
        fixes
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
    /// 说话过程中每来一条新的中间英文就翻成灰色临时中文（同一时刻只有一个在飞）
    pub partial_translate: bool,
    /// 翻译时带最近几句英中对照当上下文（0 = 不带）
    pub context_sentences: usize,
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
            partial_translate: true,
            context_sentences: 2,
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
    /// 多少秒没有语音就缩成迷你条（0 = 不自动）
    pub auto_mini_secs: u32,
    /// 有新语音时自动从迷你条展开
    pub auto_restore: bool,
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
            auto_mini_secs: 20,
            auto_restore: true,
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

    /// 安装目录变动后修正配置里失效的模型路径（见 `AsrCfg::heal_paths`），有改动就立即写回。
    /// 在日志初始化之后调用，返回的说明由调用方打日志。
    pub fn heal_model_paths(&mut self) -> anyhow::Result<Vec<String>> {
        let fixes = self.asr.heal_paths(&exe_dir().join("models"));
        if !fixes.is_empty() {
            self.save()?;
        }
        Ok(fixes)
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
        assert_eq!(back.audio.silence_ms, 400);
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
    fn translate_partial_defaults() {
        let cfg = Config::default();
        assert!(cfg.translate.partial_translate);
        assert_eq!(cfg.translate.context_sentences, 2);
        let old: Config = toml::from_str("[translate]\nbackend = \"openai\"\n").unwrap();
        assert!(old.translate.partial_translate, "旧配置文件没有这几项也要有默认值");
    }

    #[test]
    fn resolve_keeps_absolute_and_anchors_relative() {
        assert_eq!(Config::resolve(r"D:\x\y"), PathBuf::from(r"D:\x\y"));
        assert!(Config::resolve("logs").ends_with("logs"));
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-heal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn heal_rewrites_dangling_paths_to_bundled_models() {
        // 场景：先"仅我用"装到 old\，再"所有用户"装到 new\ 并卸掉旧的——配置里的绝对路径全部失效
        let root = scratch("moved");
        let old = root.join("old").join("models");
        let new = root.join("new").join("models");
        touch(&new.join("ggml-large-v3-turbo-q5_0.bin"));
        touch(&new.join("ggml-small.en.bin"));
        let mut asr = AsrCfg {
            model_path: old.join("ggml-large-v3-turbo-q5_0.bin").to_string_lossy().into_owned(),
            models_dir: old.to_string_lossy().into_owned(),
            partial_model_path: old.join("ggml-small.en.bin").to_string_lossy().into_owned(),
            ..AsrCfg::default()
        };
        let fixes = asr.heal_paths(&new);
        assert_eq!(fixes.len(), 3, "{fixes:?}");
        assert_eq!(asr.model_path, new.join("ggml-large-v3-turbo-q5_0.bin").to_string_lossy());
        assert_eq!(asr.models_dir, new.to_string_lossy());
        assert_eq!(asr.partial_model_path, new.join("ggml-small.en.bin").to_string_lossy());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn heal_leaves_valid_paths_alone() {
        let root = scratch("valid");
        let dir = root.join("models");
        let main = dir.join("a.bin");
        touch(&main);
        let bundled = root.join("bundled");
        touch(&bundled.join("a.bin"));
        let mut asr = AsrCfg { model_path: main.to_string_lossy().into_owned(), models_dir: dir.to_string_lossy().into_owned(), partial_model_path: String::new(), ..AsrCfg::default() };
        let before = asr.clone();
        assert!(asr.heal_paths(&bundled).is_empty());
        assert_eq!(asr, before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn heal_falls_back_to_any_bundled_model_and_clears_missing_partial() {
        // 精简安装只带 turbo；配置想要的文件名不在，就用目录里有的；小模型没了就清空（回落到主模型）
        let root = scratch("compact");
        let bundled = root.join("models");
        touch(&bundled.join("ggml-large-v3-turbo-q5_0.bin"));
        let gone = root.join("gone");
        let mut asr = AsrCfg {
            model_path: gone.join("ggml-medium.en.bin").to_string_lossy().into_owned(),
            models_dir: gone.to_string_lossy().into_owned(),
            partial_model_path: gone.join("ggml-small.en.bin").to_string_lossy().into_owned(),
            ..AsrCfg::default()
        };
        let fixes = asr.heal_paths(&bundled);
        assert_eq!(fixes.len(), 3, "{fixes:?}");
        assert_eq!(asr.model_path, bundled.join("ggml-large-v3-turbo-q5_0.bin").to_string_lossy());
        assert_eq!(asr.models_dir, bundled.to_string_lossy());
        assert_eq!(asr.partial_model_path, "");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn heal_without_bundled_dir_changes_nothing() {
        // 开发机：exe 旁没有 models\，路径失效也不乱改（留给用户在设置里选）
        let root = scratch("nobundle");
        let mut asr = AsrCfg { model_path: root.join("x.bin").to_string_lossy().into_owned(), models_dir: root.join("nope").to_string_lossy().into_owned(), partial_model_path: String::new(), ..AsrCfg::default() };
        let before = asr.clone();
        assert!(asr.heal_paths(&root.join("bundled")).is_empty());
        assert_eq!(asr, before);
        let _ = std::fs::remove_dir_all(&root);
    }
}
