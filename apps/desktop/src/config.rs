//! 配置：TOML，带版本迁移。首次运行从 live-caption 导入后端设置。
use ai_translate::{Backend, Scene};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION: u32 = 1;
/// 应用名，同时决定 %APPDATA% 下的数据目录名
pub const APP_NAME: &str = "translate-tool";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub backend: BackendCfg,
    pub scenes: SceneCfg,
    pub hotkeys: HotkeyCfg,
    pub ui: UiCfg,
    pub server: ServerCfg,
    pub glossary: Vec<GlossaryItem>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            backend: BackendCfg::default(),
            scenes: SceneCfg::default(),
            hotkeys: HotkeyCfg::default(),
            ui: UiCfg::default(),
            server: ServerCfg::default(),
            glossary: Vec::new(),
        }
    }
}

/// 三个后端各自的连接参数
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackendCfg {
    pub claude_bin: String,
    pub claude_model: String,
    pub codex_bin: String,
    pub codex_model: String,
    /// base_url 后拼 /chat/completions
    pub openai_base_url: String,
    pub openai_api_key: String,
    pub openai_model: String,
    pub timeout_secs: u64,
    /// OpenAI 兼容后端的并发上限（整页翻译用）
    pub concurrency: usize,
    /// 整页翻译每请求打包多少段
    pub batch_size: usize,
    pub max_retries: u32,
}

impl Default for BackendCfg {
    fn default() -> Self {
        Self {
            claude_bin: "claude".into(),
            claude_model: "sonnet".into(),
            codex_bin: "codex".into(),
            codex_model: String::new(),
            openai_base_url: "https://api.deepseek.com".into(),
            openai_api_key: String::new(),
            openai_model: "deepseek-chat".into(),
            timeout_secs: 30,
            concurrency: 8,
            batch_size: 10,
            max_retries: 3,
        }
    }
}

/// 每个场景用哪个后端。整页固定 openai（claude/codex 是常驻子进程，撑不住并发）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SceneCfg {
    pub full_page: String,
    pub selection: String,
    pub hover: String,
    pub screenshot: String,
    /// 每个场景各自的模型；留空用 backend 段里该后端的默认模型
    pub models: BTreeMap<String, String>,
}

impl Default for SceneCfg {
    fn default() -> Self {
        Self {
            full_page: "openai".into(),
            selection: "openai".into(),
            hover: "openai".into(),
            screenshot: "openai".into(),
            models: BTreeMap::new(),
        }
    }
}

impl SceneCfg {
    pub fn backend_of(&self, scene: Scene) -> Backend {
        let s = match scene {
            Scene::FullPage => &self.full_page,
            Scene::Selection => &self.selection,
            Scene::Hover => &self.hover,
            Scene::Screenshot => &self.screenshot,
        };
        let b = Backend::parse(s);
        // 整页只能走 openai
        if scene == Scene::FullPage && b != Backend::OpenAi {
            tracing::warn!("整页翻译不支持 {} 后端，已回退到 openai", b.as_str());
            return Backend::OpenAi;
        }
        b
    }

    pub fn model_of(&self, scene: Scene) -> String {
        self.models.get(scene.as_str()).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HotkeyCfg {
    /// 划词翻译
    pub selection: String,
    /// 截屏翻译
    pub screenshot: String,
    /// 显示 / 隐藏最近一次结果面板
    pub toggle_panel: String,
    /// 悬停翻译的修饰键：alt | ctrl | shift | win | none（none = 关闭悬停）
    pub hover_modifier: String,
    /// 悬停多久才触发（毫秒）
    pub hover_delay_ms: u64,
}

impl Default for HotkeyCfg {
    fn default() -> Self {
        Self {
            selection: "Ctrl+Alt+T".into(),
            screenshot: "Ctrl+Alt+S".into(),
            toggle_panel: "Ctrl+Alt+X".into(),
            hover_modifier: "alt".into(),
            hover_delay_ms: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiCfg {
    /// 选中文字后是否在选区旁弹小图标
    pub selection_icon: bool,
    /// 小图标多久不点自动消失（毫秒）
    pub icon_timeout_ms: u64,
    pub font_size: f32,
    /// 面板最大宽高
    pub panel_width: f32,
    pub panel_max_height: f32,
    /// 这些进程内不弹图标、不响应悬停（快捷键仍有效），小写可执行文件名
    pub excluded_processes: Vec<String>,
}

impl Default for UiCfg {
    fn default() -> Self {
        Self {
            selection_icon: true,
            icon_timeout_ms: 2500,
            font_size: 15.0,
            panel_width: 420.0,
            panel_max_height: 460.0,
            excluded_processes: vec!["code.exe".into(), "devenv.exe".into(), "windowsterminal.exe".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerCfg {
    pub enabled: bool,
    pub port: u16,
    /// 允许哪些扩展来源调用；留空表示允许任何 chrome-extension:// 来源
    pub allowed_extension_ids: Vec<String>,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self { enabled: true, port: 42731, allowed_extension_ids: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GlossaryItem {
    pub src: String,
    pub dst: String,
}

impl Config {
    /// 数据目录：开发模式（工作目录有 config.toml）就是工作目录；安装版用 %APPDATA%\translate-tool
    pub fn dir() -> PathBuf {
        if Path::new("config.toml").exists() {
            return PathBuf::from(".");
        }
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join(APP_NAME)
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
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
            let imported = cfg.import_from_live_caption();
            cfg.save()?;
            if imported {
                tracing::info!("已从 live-caption 导入后端设置");
            }
            Ok(cfg)
        }
    }

    /// 返回 true 表示有改动、需要写回
    pub fn migrate(&mut self) -> bool {
        if self.version >= CONFIG_VERSION {
            return false;
        }
        // 以后加新默认值：if self.version < N { ... }
        self.version = CONFIG_VERSION;
        true
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = Self::dir();
        std::fs::create_dir_all(&dir)?;
        let s = toml::to_string_pretty(self)?;
        std::fs::write(Self::path(), s)?;
        Ok(())
    }

    /// 首次运行时从 live-caption 的 config.toml 借一份后端设置（Key 与 Base URL）
    pub fn import_from_live_caption(&mut self) -> bool {
        let candidates = [
            std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("live-caption").join("config.toml")),
            Some(PathBuf::from(r"D:\My_data\Claude\live-caption\config.toml")),
        ];
        for p in candidates.into_iter().flatten() {
            let Ok(s) = std::fs::read_to_string(&p) else { continue };
            let Ok(v) = s.parse::<toml::Table>() else { continue };
            let Some(t) = v.get("translate").and_then(|t| t.as_table()) else { continue };
            let get = |k: &str| t.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let key = get("openai_api_key");
            if key.is_empty() {
                continue;
            }
            self.backend.openai_api_key = key;
            for (field, k) in [
                (&mut self.backend.openai_base_url, "openai_base_url"),
                (&mut self.backend.openai_model, "openai_model"),
                (&mut self.backend.claude_bin, "claude_bin"),
                (&mut self.backend.claude_model, "claude_model"),
                (&mut self.backend.codex_bin, "codex_bin"),
            ] {
                let val = get(k);
                if !val.is_empty() {
                    *field = val;
                }
            }
            return true;
        }
        false
    }

    /// 转成翻译核心的配置
    pub fn translator_config(&self) -> ai_translate::TranslatorConfig {
        use ai_translate::translator::SceneRoute;
        let b = &self.backend;
        let mut routes = std::collections::HashMap::new();
        for s in Scene::ALL {
            routes.insert(s, SceneRoute { backend: self.scenes.backend_of(s), model: self.scenes.model_of(s) });
        }
        ai_translate::TranslatorConfig {
            base: ai_translate::BackendConfig {
                backend: Backend::OpenAi,
                claude_bin: b.claude_bin.clone(),
                claude_model: b.claude_model.clone(),
                codex_bin: b.codex_bin.clone(),
                codex_model: b.codex_model.clone(),
                openai_base_url: b.openai_base_url.clone(),
                openai_api_key: b.openai_api_key.clone(),
                openai_model: b.openai_model.clone(),
                timeout: std::time::Duration::from_secs(b.timeout_secs.max(1)),
                command_override: Vec::new(),
            },
            routes,
            concurrency: b.concurrency.clamp(1, 64),
            batch_size: b.batch_size.clamp(1, 50),
            retry: ai_translate::RetryPolicy {
                max_retries: b.max_retries.min(10),
                base_delay: std::time::Duration::from_secs(1),
            },
            glossary: ai_translate::Glossary::new(
                self.glossary
                    .iter()
                    .map(|g| ai_translate::GlossaryEntry { src: g.src.clone(), dst: g.dst.clone() })
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_toml() {
        let s = toml::to_string_pretty(&Config::default()).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back, Config::default());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("version = 1\n[backend]\nopenai_model = \"x\"\n").unwrap();
        assert_eq!(cfg.backend.openai_model, "x");
        assert_eq!(cfg.backend.claude_model, "sonnet", "没写的字段用默认值");
        assert_eq!(cfg.hotkeys.selection, "Ctrl+Alt+T");
    }

    #[test]
    fn migrate_only_bumps_old_versions() {
        let mut old: Config = toml::from_str("version = 0\n").unwrap();
        assert!(old.migrate());
        assert_eq!(old.version, CONFIG_VERSION);
        assert!(!old.migrate(), "已是最新版不再改");
    }

    #[test]
    fn full_page_falls_back_to_openai() {
        let mut c = Config::default();
        c.scenes.full_page = "claude".into();
        assert_eq!(c.scenes.backend_of(Scene::FullPage), Backend::OpenAi);
        c.scenes.hover = "claude".into();
        assert_eq!(c.scenes.backend_of(Scene::Hover), Backend::Claude);
    }

    #[test]
    fn translator_config_carries_routes_and_glossary() {
        let mut c = Config::default();
        c.scenes.selection = "claude".into();
        c.scenes.models.insert("selection".into(), "haiku".into());
        c.glossary.push(GlossaryItem { src: "pod".into(), dst: "Pod".into() });
        let t = c.translator_config();
        assert_eq!(t.route(Scene::Selection).backend, Backend::Claude);
        assert_eq!(t.backend_cfg(Scene::Selection).claude_model, "haiku");
        assert_eq!(t.glossary.entries().len(), 1);
    }

    #[test]
    fn clamps_out_of_range_numbers() {
        let mut c = Config::default();
        c.backend.concurrency = 999;
        c.backend.batch_size = 0;
        let t = c.translator_config();
        assert_eq!(t.concurrency, 64);
        assert_eq!(t.batch_size, 1);
    }
}
