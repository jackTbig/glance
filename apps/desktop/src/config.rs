//! 配置：TOML，带版本迁移。首次运行从 live-caption 导入后端设置。
use ai_translate::{Backend, Scene};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CONFIG_VERSION: u32 = 3;
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
    /// 翻译方向：en2zh（默认，英译中）/ zh2en（中译英）。
    /// 认不出来的值一律当默认方向，别因为手改错配置就翻不动
    #[serde(default = "default_direction")]
    pub direction: String,
    ///  OpenAI 兼容服务的历史记录（地址+Key+模型），设置页一键切换
    #[serde(default)]
    pub services: Vec<ServiceProfile>,
}

/// 一条 OpenAI 兼容服务档案（Jack 2026-09-27：多个模型服务间切换，历史保存）
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServiceProfile {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// 该服务的请求体要不要带 enable_thinking。参数是「按服务」的：切服务时跟着档案走，
    /// 百炼的 false 串到 DeepSeek 官方（对未知参数容忍度未验证）会全场景报错。
    /// None = 不带参数（服务默认行为）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
}

/// 历史最多留几条：再多下拉就没法看了
pub const SERVICE_HISTORY_MAX: usize = 8;

/// 自动命名：主机名 · 模型名。Key 不进名字（名字会显示在界面上）
pub fn service_auto_name(base_url: &str, model: &str) -> String {
    let t = base_url.trim();
    let host = t.split_once("://").map(|x| x.1).unwrap_or(t);
    let host = host.split(['/', '?']).next().unwrap_or(host);
    let host = if host.is_empty() { t } else { host };
    format!("{host} · {}", model.trim())
}

/// 把当前组合写进历史：按（地址、Key、模型）去重，命中的挪到最前，超上限丢最旧。
/// 同名不同组合的加序号，别在下拉里长成两个一样的名字
pub fn upsert_service(list: Vec<ServiceProfile>, cur: &ServiceProfile) -> Vec<ServiceProfile> {
    let same = |p: &ServiceProfile| p.base_url == cur.base_url && p.api_key == cur.api_key && p.model == cur.model;
    let mut out: Vec<ServiceProfile> = list.into_iter().filter(|p| !same(p)).collect();
    let mut cur = cur.clone();
    if cur.name.trim().is_empty() {
        cur.name = service_auto_name(&cur.base_url, &cur.model);
    }
    let base_name = cur.name.clone();
    let mut n = 2;
    while out.iter().any(|p| p.name == cur.name) {
        cur.name = format!("{base_name} #{n}");
        n += 1;
    }
    out.insert(0, cur);
    out.truncate(SERVICE_HISTORY_MAX);
    out
}

fn default_direction() -> String {
    ai_translate::Direction::default().as_str().to_string()
}

/// 服务切换（已存服务 / 服务商预设）的逻辑：地址/Key/model 覆盖到位，
/// `enable_thinking` **仅当当前未设**（None）时才用档案的值补——
/// 避免选「DeepSeek 预设」（None）把之前手动写的 `false` 擦掉，
/// 切回「通义千问」也不会改用户的逻辑。放出来给 ui.rs 两处下拉复用，便于单测。
pub fn pick_service_into(dst: &mut BackendCfg, src: &ServiceProfile) {
    dst.openai_base_url = src.base_url.clone();
    dst.openai_api_key = src.api_key.clone();
    dst.openai_model = src.model.clone();
    if dst.enable_thinking.is_none() {
        dst.enable_thinking = src.enable_thinking;
    }
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
            direction: default_direction(),
            services: Vec::new(),
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
    /// 请求体带不带 `enable_thinking`：不写 = 不带（服务默认行为）；false = 关思考。
    /// 百炼网关的 deepseek-v4-flash 默认开思考，翻译慢 12 倍，该服务要写 false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
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
            openai_model: "deepseek-flash".into(),
            timeout_secs: 30,
            concurrency: 8,
            batch_size: 10,
            max_retries: 3,
            enable_thinking: None,
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
            // 取词释义不单独配后端：它是划词/悬停的一种结果形态，跟着划词走
            Scene::Word => &self.selection,
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
    /// 一次性动作：按了就进入截屏框选
    pub screenshot: String,
    /// 显示 / 隐藏最近一次结果面板
    pub toggle_panel: String,
    /// **开关**悬停取词：开了才待命，关了按修饰键也不响应
    pub toggle_hover: String,
    /// **开关**划词小图标
    pub toggle_selection: String,
    /// 悬停取词开着时，按住哪个键 + 鼠标停留才调用：alt | ctrl | shift | win
    /// 字幕：无条件切回可点击。**这一条是穿透模式唯一的退路，别删**——
    /// 穿透开着时字幕窗对鼠标完全无感，托盘又在简译这边。
    #[serde(default = "default_caption_clickable")]
    pub caption_clickable: String,
    /// 字幕：切换鼠标穿透
    #[serde(default = "default_caption_passthrough")]
    pub caption_passthrough: String,
    pub hover_modifier: String,
    /// 悬停多久才触发（毫秒）
    pub hover_delay_ms: u64,
    /// 旧字段，保留只为读得进老配置；语义已由 toggle_selection 接管
    #[serde(default, skip_serializing)]
    pub selection: String,
}

impl Default for HotkeyCfg {
    fn default() -> Self {
        Self {
            // 别用 Ctrl+Alt+T / S / X / D / W：本机实测这些已被其他程序占用（错误码 1409）。
            // 注册失败时 ui.rs 会自动往下试 FALLBACK_HOTKEYS 里的备选。
            screenshot: "Ctrl+Alt+G".into(),
            toggle_panel: "Ctrl+Alt+Q".into(),
            // Ctrl+Alt+H 在本机被别的程序占了（探针实测），用 J
            toggle_hover: "Ctrl+Alt+J".into(),
            toggle_selection: "Ctrl+Alt+Y".into(),
            caption_clickable: default_caption_clickable(),
            caption_passthrough: default_caption_passthrough(),
            hover_modifier: "ctrl".into(),
            hover_delay_ms: 300,
            selection: String::new(),
        }
    }
}

fn default_caption_clickable() -> String {
    "Ctrl+Alt+C".into()
}

fn default_caption_passthrough() -> String {
    "Ctrl+Alt+P".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiCfg {
    /// 悬停取词是否待命。关掉后按住修饰键也不响应（两段式的第一段）
    pub hover_enabled: bool,
    /// 选中文字后是否在选区旁弹小图标
    pub selection_icon: bool,
    /// 小图标多久不点自动消失（毫秒）
    pub icon_timeout_ms: u64,
    pub font_size: f32,
    /// 面板最大宽高
    pub panel_width: f32,
    pub panel_max_height: f32,
    /// 这些进程内不弹图标、不响应悬停（快捷键仍有效），小写可执行文件名。
    /// **默认为空**：预置名单会让用户在自己常用的窗口里悬停毫无反应，
    /// 而且失败是静默的，很难想到是被名单挡了（Jack 2026-09-09 实测踩过）。
    pub excluded_processes: Vec<String>,
    /// 取到单个词时先给一份离线词典的释义，模型的词条回来再替换（Jack 2026-09-20 要求）。
    /// 关掉就只等模型。
    #[serde(default = "default_true")]
    pub offline_dict: bool,
    /// 截屏翻译用覆盖式（原图上按行盖译文，点击切换原文/译文，有道那种）。
    /// 关掉就退回老样子：一张卡片给整段译文。
    #[serde(default = "default_true")]
    pub region_overlay: bool,
    /// 悬停取到的文字**已经是目标语言**时，干脆不弹卡片（Jack 2026-09-14 要求）。
    /// 悬停是被动触发的：鼠标扫过中文就弹一张「这已经是中文」的卡片非常烦人。
    /// 划词与截屏是用户主动点的，不受这个开关影响——不弹反而像是没反应。
    #[serde(default = "default_true")]
    pub hover_skip_target_lang: bool,
}

fn default_true() -> bool {
    true
}

impl Default for UiCfg {
    fn default() -> Self {
        Self {
            hover_enabled: true,
            selection_icon: true,
            icon_timeout_ms: 2500,
            font_size: 15.0,
            panel_width: 420.0,
            panel_max_height: 460.0,
            excluded_processes: Vec::new(),
            offline_dict: true,
            region_overlay: true,
            hover_skip_target_lang: true,
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
        if self.version < 2 {
            // v1 预置了 code/devenv/windowsterminal 三个例外，结果是用户在这些窗口里
            // 悬停完全没反应且没有任何提示。只在用户没自己改过（三项原样）时清掉。
            let v1_default = ["code.exe", "devenv.exe", "windowsterminal.exe"];
            let unchanged = self.ui.excluded_processes.len() == v1_default.len()
                && v1_default.iter().all(|d| self.ui.excluded_processes.iter().any(|x| x.eq_ignore_ascii_case(d)));
            if unchanged {
                self.ui.excluded_processes.clear();
            }
        }
        if self.version < 3 {
            // v2 的 hotkeys.selection 是「直接翻译选中内容」，v3 改成两段式：
            // 三键管开关、简单键管调用。老的那个键顺势变成划词图标的开关键。
            if !self.hotkeys.selection.trim().is_empty() && self.hotkeys.toggle_selection.trim().is_empty() {
                self.hotkeys.toggle_selection = std::mem::take(&mut self.hotkeys.selection);
            }
            self.hotkeys.selection = String::new();
            // v2 用 hover_modifier = "none" 表示关闭悬停，v3 用独立的开关表示
            if self.hotkeys.hover_modifier.trim().eq_ignore_ascii_case("none") {
                self.ui.hover_enabled = false;
                self.hotkeys.hover_modifier = "ctrl".into();
            }
        }
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
                enable_thinking: b.enable_thinking,
                command_override: Vec::new(),
            },
            routes,
            concurrency: b.concurrency.clamp(1, 64),
            batch_size: b.batch_size.clamp(1, 50),
            retry: ai_translate::RetryPolicy {
                max_retries: b.max_retries.min(10),
                base_delay: std::time::Duration::from_secs(1),
            },
            direction: ai_translate::Direction::parse(&self.direction),
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

    fn prof(base: &str, key: &str, model: &str) -> ServiceProfile {
        ServiceProfile { name: String::new(), base_url: base.into(), api_key: key.into(), model: model.into(), enable_thinking: None }
    }

    #[test]
    fn auto_name_uses_host_and_model_not_the_key() {
        assert_eq!(service_auto_name("https://api.deepseek.com/v1", "deepseek-flash"), "api.deepseek.com · deepseek-flash");
        assert_eq!(service_auto_name("", "m"), " · m");
    }

    #[test]
    fn upsert_dedupes_moves_to_front_and_caps() {
        let mut l = vec![prof("a", "k1", "m1"), prof("b", "k2", "m2")];
        l = upsert_service(l, &prof("b", "k2", "m2"));
        assert_eq!(l[0].base_url, "b", "命中的挪到最前");
        assert_eq!(l.len(), 2);
        l = upsert_service(l, &prof("c", "k3", "m3"));
        assert_eq!(l.len(), 3);
        for i in 0..10 {
            l = upsert_service(l, &prof(&format!("x{i}"), "k", "m"));
        }
        assert_eq!(l.len(), SERVICE_HISTORY_MAX, "超上限丢最旧");
        assert_eq!(l[0].base_url, "x9");
    }

    #[test]
    fn same_name_different_key_gets_a_suffix() {
        let mut l = vec![ServiceProfile { name: "gw · m".into(), base_url: "gw".into(), api_key: "k1".into(), model: "m".into(), enable_thinking: None }];
        l = upsert_service(l, &ServiceProfile { name: "gw · m".into(), base_url: "gw".into(), api_key: "k2".into(), model: "m".into(), enable_thinking: None });
        assert_eq!(l[0].name, "gw · m #2");
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn direction_defaults_to_chinese_and_round_trips() {
        let c = Config::default();
        assert_eq!(c.translator_config().direction, ai_translate::Direction::ToChinese, "默认英译中");
        let mut c2 = Config::default();
        c2.direction = "zh2en".into();
        assert_eq!(c2.translator_config().direction, ai_translate::Direction::ToEnglish);
        // 手改坏了也要能翻译，回到默认方向
        let mut c3 = Config::default();
        c3.direction = "乱写".into();
        assert_eq!(c3.translator_config().direction, ai_translate::Direction::ToChinese);
    }

    #[test]
    fn hover_skips_target_language_by_default() {
        assert!(Config::default().ui.hover_skip_target_lang, "鼠标扫过中文默认不弹卡片");
    }

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
        assert_eq!(cfg.hotkeys.toggle_selection, "Ctrl+Alt+Y");
    }

    #[test]
    fn v1_preset_exclusions_are_cleared_on_upgrade() {
        // v1 预置的三个例外让用户在自己常用的窗口里悬停毫无反应，升级时清掉
        let mut old: Config = toml::from_str(
            "version = 1
[ui]
excluded_processes = [\"code.exe\", \"devenv.exe\", \"windowsterminal.exe\"]
",
        )
        .unwrap();
        assert!(old.migrate());
        assert!(old.ui.excluded_processes.is_empty(), "预置名单应被清掉");

        // 用户自己改过的名单不动
        let mut mine: Config =
            toml::from_str("version = 1
[ui]
excluded_processes = [\"mygame.exe\"]
").unwrap();
        assert!(mine.migrate());
        assert_eq!(mine.ui.excluded_processes, vec!["mygame.exe".to_string()]);
    }

    #[test]
    fn v3_moves_old_selection_key_to_the_toggle() {
        // v2 的 hotkeys.selection 是「直接翻译选中内容」；v3 改成两段式后，
        // 这个键顺势变成划词图标的开关键，别让用户的设置凭空消失
        let mut c: Config =
            toml::from_str("version = 2
[hotkeys]
selection = \"Ctrl+Alt+Y\"
").unwrap();
        assert!(c.migrate());
        assert_eq!(c.hotkeys.toggle_selection, "Ctrl+Alt+Y");
        assert!(c.hotkeys.selection.is_empty(), "旧字段迁完要清空");
    }

    #[test]
    fn v3_turns_hover_none_into_an_explicit_switch() {
        // v2 用 hover_modifier = "none" 表示关闭悬停；v3 有独立开关了
        let mut c: Config =
            toml::from_str("version = 2
[hotkeys]
hover_modifier = \"none\"
").unwrap();
        assert!(c.migrate());
        assert!(!c.ui.hover_enabled, "原来是关的，迁完还得是关的");
        assert_eq!(c.hotkeys.hover_modifier, "ctrl", "修饰键要给个能用的默认值");
    }

    #[test]
    fn two_stage_defaults_are_sane() {
        let c = Config::default();
        // 第一段：开关用三键
        for k in [&c.hotkeys.toggle_hover, &c.hotkeys.toggle_selection] {
            assert_eq!(k.matches('+').count(), 2, "开关键用三键组合：{k}");
        }
        // 第二段：调用只按一个修饰键
        assert!(!c.hotkeys.hover_modifier.contains('+'), "调用键是单个修饰键");
        assert!(c.ui.hover_enabled && c.ui.selection_icon, "默认两个功能都待命");
    }

    #[test]
    fn default_has_no_excluded_processes() {
        assert!(Config::default().ui.excluded_processes.is_empty(), "默认不预置例外，要排除让用户自己加");
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

    /// enable_thinking 从配置文件透传到翻译核心：不写 = 不发参数（老配置文件加载后
    /// 行为不变）；写 false 才随请求发出去。序列化时 None 不落盘，配置文件不多一行
    #[test]
    fn enable_thinking_flows_from_toml_to_backend() {
        let c: Config = toml::from_str("version = 3\n").unwrap();
        assert_eq!(c.backend.enable_thinking, None);
        assert_eq!(c.translator_config().base.enable_thinking, None);
        let c2: Config = toml::from_str("version = 3\n[backend]\nenable_thinking = false\n").unwrap();
        assert_eq!(c2.translator_config().base.enable_thinking, Some(false));
        let saved = toml::to_string(&c).unwrap();
        assert!(!saved.contains("enable_thinking"), "None 不该写进配置文件：{saved}");
    }

    /// 切服务时：地址/Key/model 覆盖到位；enable_thinking **保留用户已设的值**，
    /// 仅当当前未设（None）时才用档案的值补——避免选 DeepSeek 预设（None）
    /// 把用户之前手动写的 false 擦掉
    #[test]
    fn pick_service_preserves_existing_enable_thinking() {
        let mk_src = |thinking: Option<bool>| ServiceProfile {
            name: "x".into(), base_url: "u".into(), api_key: "k".into(), model: "m".into(),
            enable_thinking: thinking,
        };
        // 已设为 false：选 None 档案不能擦掉
        let mut dst = BackendCfg { enable_thinking: Some(false), ..BackendCfg::default() };
        crate::config::pick_service_into(&mut dst, &mk_src(None));
        assert_eq!(dst.enable_thinking, Some(false), "已设 false 不能被 None 擦掉");
        // 已设为 false：选 Some(true) 也不能擦掉
        crate::config::pick_service_into(&mut dst, &mk_src(Some(true)));
        assert_eq!(dst.enable_thinking, Some(false), "已设 false 不能被 true 覆盖");
        // 当前是 None：选 Some(false) 补上
        dst.enable_thinking = None;
        crate::config::pick_service_into(&mut dst, &mk_src(Some(false)));
        assert_eq!(dst.enable_thinking, Some(false), "None 时应补档案的值");
        // 当前是 None：选 None 仍是 None
        dst.enable_thinking = None;
        crate::config::pick_service_into(&mut dst, &mk_src(None));
        assert_eq!(dst.enable_thinking, None);
        // URL/Key/model 总是被覆盖
        assert_eq!(dst.openai_base_url, "u");
        assert_eq!(dst.openai_api_key, "k");
        assert_eq!(dst.openai_model, "m");
    }

    /// 服务档案要记住 enable_thinking：切服务时参数跟着档案走，不串味
    /// （百炼的 false 带到 DeepSeek 官方，官方对未知参数的容忍度未验证）
    #[test]
    fn service_profile_carries_enable_thinking() {
        let l = upsert_service(
            vec![],
            &ServiceProfile { name: "gw".into(), base_url: "gw".into(), api_key: "k".into(), model: "m".into(), enable_thinking: Some(false) },
        );
        assert_eq!(l[0].enable_thinking, Some(false));
        // 老配置文件里没这个字段：读出来是 None 不报错；None 也不写回文件
        let p: ServiceProfile = toml::from_str("name = 'a'\nbase_url = 'b'\napi_key = 'k'\nmodel = 'm'\n").unwrap();
        assert_eq!(p.enable_thinking, None);
        assert!(!toml::to_string(&p).unwrap().contains("enable_thinking"));
    }
}
