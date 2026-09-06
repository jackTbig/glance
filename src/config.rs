use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 当前配置格式版本（见 `Config::migrate`）
pub const CONFIG_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// 配置格式版本：旧文件没有这项（= 0），启动时按 `migrate` 一次性把调优过的默认值补上
    pub version: u32,
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
        // 2026-09-06 Jack 试用后定下：0.3 s 就出首条中间结果、每 0.3 s 刷新、6 s 起在词间隙软切（不硬切）
        Self { energy_threshold: 0.01, silence_ms: 400, min_segment_ms: 300, max_segment_ms: 6000, partial_every_ms: 300, device: String::new() }
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
    /// 中间识别只解最近这么长的窗口（前面已定的文字用分段时间戳固定），0 = 每次重解整句
    pub partial_window_ms: u32,
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
        // 中间识别默认用 base.en（2026-09-06 长句验收：5 s 窗 0.21 s，英文落后语音中位 245 ms；small.en 0.61 s / 791 ms，词命中率几乎相同）；
        // 没有 base.en 退到 small.en；都没有就空着（复用主模型）
        let partial_model_path = ["ggml-base.en.bin", "ggml-small.en.bin"]
            .iter()
            .map(|n| Path::new(&models_dir).join(n))
            .find(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        // min_audio_ctx 384（2026-09-06 基准：turbo 5 s 片段 1095→893 ms、2 s 片段 1777→663 ms，文本相同；320 在短片段上不稳定）
        Self { model_path, models_dir, language: "en".into(), threads: 4, flash_attn: false, min_audio_ctx: 384, partial_window_ms: 5000, partial_model_path }
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
    /// 每行块给英文、中文各预留几行高度（1 = 一行；2 = 长句换行也不顶版面，短句下面留白）
    pub reserve_rows: u32,
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
            typewriter_cps: 0.0,
            hotkey_clickable: "Ctrl+Alt+C".into(),
            hotkey_passthrough: "Ctrl+Alt+P".into(),
            auto_mini_secs: 20,
            auto_restore: true,
            reserve_rows: 1,
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
            let mut cfg: Config = toml::from_str(&s)?;
            if cfg.migrate() {
                cfg.save()?;
            }
            Ok(cfg)
        } else {
            let mut cfg = Config::default();
            cfg.version = CONFIG_VERSION;
            cfg.save()?;
            Ok(cfg)
        }
    }

    /// 旧配置文件一次性升级：2026-09-06 试用定下的切句/显示参数只在版本号落后时补上（用户之后自己改的值不再被动）。
    /// 返回是否有改动。
    pub fn migrate(&mut self) -> bool {
        if self.version >= CONFIG_VERSION {
            return false;
        }
        if self.version < 2 {
            let d = AudioCfg::default();
            self.audio.silence_ms = d.silence_ms;
            self.audio.min_segment_ms = d.min_segment_ms;
            self.audio.max_segment_ms = d.max_segment_ms;
            self.audio.partial_every_ms = d.partial_every_ms;
            self.ui.typewriter_cps = UiCfg::default().typewriter_cps;
            self.asr.partial_window_ms = AsrCfg::default().partial_window_ms;
        }
        if self.version < 3 {
            // 定稿编码窗下限 512 → 384（短段定稿快约 40%，文本相同）：只动还是旧默认值的
            if self.asr.min_audio_ctx == 512 {
                self.asr.min_audio_ctx = 384;
            }
            // 中间识别模型 small.en → base.en（快 3 倍、词命中率相同）：只在旧值是 small.en 且同目录有 base.en 时换
            let p = Path::new(&self.asr.partial_model_path);
            if p.file_name().map_or(false, |n| n == "ggml-small.en.bin") {
                let base = p.with_file_name("ggml-base.en.bin");
                if base.is_file() {
                    self.asr.partial_model_path = base.to_string_lossy().into_owned();
                }
            }
        }
        self.version = CONFIG_VERSION;
        true
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
        assert_eq!(cfg.audio.max_segment_ms, 6000);
    }

    #[test]
    fn old_model_key_still_reads_into_claude_model() {
        let cfg: Config = toml::from_str("[translate]\nmodel = \"haiku\"\n").unwrap();
        assert_eq!(cfg.translate.claude_model, "haiku");
    }

    #[test]
    fn old_config_without_version_gets_tuned_defaults_once() {
        // 1.0.0/1.0.1 安装版留下的 0.2.0 时代参数
        let mut cfg: Config = toml::from_str("[audio]\nsilence_ms = 500\nmin_segment_ms = 1000\nmax_segment_ms = 8000\npartial_every_ms = 1500\n[ui]\ntypewriter_cps = 60.0\nfont_size = 30.0\n").unwrap();
        assert_eq!(cfg.version, 0);
        assert!(cfg.migrate());
        assert_eq!((cfg.audio.silence_ms, cfg.audio.min_segment_ms, cfg.audio.max_segment_ms, cfg.audio.partial_every_ms), (400, 300, 6000, 300));
        assert_eq!(cfg.ui.typewriter_cps, 0.0);
        assert_eq!(cfg.asr.partial_window_ms, 5000);
        assert_eq!(cfg.ui.font_size, 30.0, "与本次调优无关的用户设置不动");
        assert_eq!(cfg.version, CONFIG_VERSION);
        assert!(!cfg.migrate(), "升级过就不再动");
        // 已是当前版本的文件，用户自己改的值保持
        let mut newer: Config = toml::from_str(&format!("version = {CONFIG_VERSION}\n[audio]\nmin_segment_ms = 800\n")).unwrap();
        assert!(!newer.migrate());
        assert_eq!(newer.audio.min_segment_ms, 800);
        // 新生成的默认配置带当前版本号，写回后再读不会触发升级
        let s = toml::to_string(&Config { version: CONFIG_VERSION, ..Config::default() }).unwrap();
        let mut back: Config = toml::from_str(&s).unwrap();
        assert!(!back.migrate());
    }

    #[test]
    fn migration_v3_swaps_small_en_for_base_en_only_when_present() {
        let root = scratch("v3");
        let dir = root.join("models");
        touch(&dir.join("ggml-small.en.bin"));
        let small = dir.join("ggml-small.en.bin").to_string_lossy().into_owned();
        // 同目录没有 base.en：不换
        let mut a: Config = toml::from_str(&format!("version = 2\n[asr]\npartial_model_path = '{}'\n", small)).unwrap();
        assert!(a.migrate());
        assert_eq!(a.asr.partial_model_path, small);
        // 有 base.en：换过去
        touch(&dir.join("ggml-base.en.bin"));
        let mut b: Config = toml::from_str(&format!("version = 2\n[asr]\npartial_model_path = '{}'\n", small)).unwrap();
        assert!(b.migrate());
        assert_eq!(b.asr.partial_model_path, dir.join("ggml-base.en.bin").to_string_lossy());
        // 用户指定的其他模型不动；旧默认 512 的编码窗下限换成 384，用户改过的值（如 640）不动
        let mut c: Config = toml::from_str("version = 2\n[asr]\npartial_model_path = 'D:/x/ggml-tiny.en.bin'\nmin_audio_ctx = 512\n").unwrap();
        assert!(c.migrate());
        assert_eq!(c.asr.partial_model_path, "D:/x/ggml-tiny.en.bin");
        assert_eq!(c.asr.min_audio_ctx, 384);
        let mut d: Config = toml::from_str("version = 2\n[asr]\nmin_audio_ctx = 640\n").unwrap();
        assert!(d.migrate());
        assert_eq!(d.asr.min_audio_ctx, 640);
        let _ = std::fs::remove_dir_all(&root);
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
