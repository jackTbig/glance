//! egui 界面：译文浮窗、划词小图标、截屏框选、设置窗、托盘
use crate::config::Config;
use crate::dpi::Dpi;
use crate::engine::{Engine, Event, Job};
use crate::hotkey::{parse_hotkey, HoverModifier};
use crate::panel::{place, place_icon, PanelState, Phase};
use crate::watcher::{Trigger, WatchConfig, Watcher};
use ai_translate::Scene;
use crossbeam_channel::Receiver;
use egui::{Align2, Color32, CornerRadius, RichText, ViewportCommand, ViewportId};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use text_capture::{Capturer, Source};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 应用中文名（窗口标题、托盘提示都用它）
pub const APP_NAME_ZH: &str = "简译";

/// 设置窗左侧的分类。分成四类而不是一长串折叠项：
/// 折叠项要点开才知道里面有什么，且展开后内容全挤在左半边、右边一片空白。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    /// 主窗口的「翻译」页：手动输入单词或句子来翻（Jack 2026-09-22 要求）
    Translate,
    Service,
    Trigger,
    Glossary,
    Server,
    /// 运行环境自检：这台机器缺什么、缺了影响哪个功能、点一下能不能补上
    Env,
    /// 关于：版本、构建信息、各组件在哪、排障要问的那几条
    About,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 7] = [
        SettingsTab::Translate,
        SettingsTab::Service,
        SettingsTab::Trigger,
        SettingsTab::Glossary,
        SettingsTab::Server,
        SettingsTab::Env,
        SettingsTab::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::Translate => "翻译",
            SettingsTab::Service => "翻译服务",
            SettingsTab::Trigger => "触发方式",
            SettingsTab::Glossary => "术语表",
            SettingsTab::Server => "浏览器插件",
            SettingsTab::Env => "运行环境",
            SettingsTab::About => "关于",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SettingsTab::Translate => "输入单词或句子，自动判断英译中还是中译英",
            SettingsTab::Service => "接口地址、API Key、各场景用哪个后端",
            SettingsTab::Trigger => "快捷键、悬停、划词图标，以及不想被打扰的程序",
            SettingsTab::Glossary => "让专有名词按你定的译法翻",
            SettingsTab::Server => "安装或更新 Chrome 插件，以及它依赖的本地服务",
            SettingsTab::Env => "缺什么、影响哪个功能、能不能一键补上",
            SettingsTab::About => "版本、各组件装在哪、出问题时要提供的信息",
        }
    }
}

/// 设置项左侧标签的固定宽度，让每行的控件左边缘对齐
const LABEL_W: f32 = 76.0;
/// 长输入框的宽度
const FIELD_W: f32 = 196.0;

/// 用户配的热键被别的程序占了就往下试这些。
/// 常见组合（Ctrl+Alt+T/S/X/D/W、Ctrl+Shift+D）在实测机器上都已被占用，别放进来。
const FALLBACK_HOTKEYS: &[&str] = &[
    "Ctrl+Alt+Y", "Ctrl+Alt+G", "Ctrl+Alt+Q", "Ctrl+Alt+J", "Ctrl+Alt+K", "Ctrl+Alt+U", "Ctrl+Alt+B",
    "Ctrl+Shift+Y", "Ctrl+Shift+Q", "Ctrl+Shift+T",
    "Alt+Shift+Y", "Alt+Shift+Q", "Alt+Shift+Z",
    "Ctrl+Alt+F9", "Ctrl+Alt+F10", "Ctrl+Alt+F11",
];

/// 框选完到真正抓图之间等多久：够窗口挪走、桌面重新合成，又不至于让人觉得卡。
/// 界面按 33 ms 一帧跑，这里相当于等 5 帧。
const MASK_CLEAR_DELAY: Duration = Duration::from_millis(180);

/// 打开设置窗后连续摆位置的帧数。界面按 33 ms 一帧跑，5 帧约 165 ms。
/// 摆完就停，否则用户没法用鼠标调窗口大小。
const PLACE_FRAMES: u8 = 5;

/// 摆位最多重试几轮。有上限是为了不跟窗口管理器较劲成死循环——
/// 用户把窗口拖走之后，读回的位置本来就该跟目标不一致。
const MAX_PLACE_RETRIES: u8 = 3;

/// 截屏卡片跟着框宽走时的下限（逻辑像素）。框选得再窄，卡片也要放得下一行字
const MIN_REGION_PANEL_W: f32 = 240.0;

/// 覆盖窗口下方那条提示栏的高度（逻辑像素）。**它在截图区之外**，不压原文
const OVERLAY_BAR_H: f32 = 26.0;

/// 空闲时把主窗口丢到哪儿（**逻辑**坐标）。
///
/// **别再写 -32000**：egui 收的是逻辑坐标，200% 缩放下会乘成物理 -64000，
/// 超出 Win32 窗口坐标上限被系统夹到 -16384，于是「发了没到位 → 再发 → 窗口回报移动 →
/// 又重绘」每帧循环，空闲时吃掉整整一个核（2026-09-13 Jack 报「没操作也占 CPU」）。
/// -8000 在 200% 下是物理 -16000，仍在屏幕外，且系统不会夹。
const HIDDEN_POS: egui::Pos2 = egui::pos2(-8000.0, -8000.0);
const HIDDEN_SIZE: egui::Vec2 = egui::vec2(1.0, 1.0);

/// 界面里正在做什么
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Idle,
    /// 选区旁的小图标
    /// `target`：弹图标那一刻的前台窗口，也就是有选区的程序。点图标会把前台抢成简译自己，
    /// 取词前必须切回它，否则模拟的 Ctrl+C 发给了简译，报「没有选中文字」（2026-09-11 Jack 报）
    Icon { pos: (i32, i32), since: Instant, target: Option<isize>, clip_seq: u32 },
    /// 截屏覆盖：原图铺在框选的位置上，译文盖回每行原文（有道那种，Jack 2026-09-14 要求）
    Overlay,
    /// 译文面板
    Panel,
    /// 截屏框选中
    Selecting { start: Option<(i32, i32)> },
}

pub struct App {
    cfg: Config,
    translator: Arc<Mutex<ai_translate::Translator>>,
    capturer: Arc<Capturer>,
    engine: Engine,
    events: Receiver<Event>,
    triggers: Receiver<Trigger>,
    watcher: Watcher,
    _hotkeys: GlobalHotKeyManager,
    hotkey_ids: Vec<(u32, HotkeyAction)>,
    mode: Mode,
    panel: Option<PanelState>,
    /// 框选已完成、等遮罩窗从屏幕上撤走再抓图（物理矩形 + 框完的时刻）。
    /// 直接抓会把半透明遮罩一起拍进去，识别的是被压暗的画面。
    pending_region: Option<((i32, i32, u32, u32), Instant)>,
    /// 刚进入框选：给遮罩窗要一次焦点，不然 Esc 取消收不到按键
    focus_selector: bool,
    /// 挂起的托盘左键单击（位置 + 按下时刻）。等过了系统双击间隔还没等到双击，才弹菜单——
    /// 双击的第一次 UP 与单击在消息层面无法区分，只能靠等
    pending_tray_click: Option<((i32, i32), Instant)>,
    /// 上次记进日志的小图标位置。界面 30 fps 重画，不去重的话一秒刷 30 行
    logged_icon_at: Option<(i32, i32)>,
    /// 上次记进日志的卡片宽度，同样是为了不每帧刷屏
    logged_card_w: Option<f32>,
    /// 主窗口「翻译」页的状态
    manual: crate::manual::ManualState,
    /// 窗口已经开着时又被要求打开（换页）：下一帧把它提到前面、改标题，但不重新摆位
    settings_raise: bool,
    /// 窗口当前按哪类页定的尺寸（true = 翻译页）；None = 窗口没开
    settings_sized_as: Option<bool>,
    /// 下次跑 logic 时把窗口从最小化恢复。窗口最小化时 eframe 只跑 logic 不跑 ui，
    /// 恢复命令必须在 logic 里发，否则点托盘「打开」看着就是没反应（Jack 2026-09-24 实测）
    settings_unminimize: bool,
    /// 关于页里的词库信息（路径 + 条数）。开词库要读几 MB 索引，算一次就存着
    about_dict: Option<Option<(std::path::PathBuf, usize)>>,
    /// 上次记进日志的内容排版宽度。**窗口宽了而它没跟着宽 = 内容没铺满**
    logged_content_w: Option<f32>,
    /// 上一次发出去的「窗口摆到哪、多大」（逻辑坐标）。**没变就不要再发**——
    /// 每帧重发会让窗口回报 Resized/Moved，egui 收到又立刻重绘，自己把自己喂成死循环：
    /// 空闲时实测吃掉将近一整个核（2026-09-13 Jack 报「没操作也占 CPU」）。
    last_place: Option<(egui::Pos2, egui::Vec2)>,
    /// 帧率自查：空闲时本该 10 fps 上下，跑到几百帧就是有人在自激重绘（会吃掉一整个核）
    fps: (Instant, u32, u32),
    /// 同一个摆放目标已经重发了几次。**必须封顶**：目标若是系统不接受的位置，
    /// 「没到位就重发」会变成每帧重发的死循环
    place_tries: u8,
    /// 当前有效的任务序号，用来丢弃过期事件
    cur_seq: u64,
    /// 已经发了任务、但还没弹卡片（悬停用，见 `start_deferred`）：(场景标题, 来源, 锚点)
    pending_panel: Option<(&'static str, Source, (i32, i32))>,
    /// 截屏覆盖的当前状态（原图纹理 + 每行译文）
    overlay: Option<OverlayState>,
    /// 这次截屏框的位置（虚拟屏物理坐标）。原图要**铺回它自己的位置**才能与屏幕对齐
    overlay_rect: Option<(i32, i32, u32, u32)>,
    /// 上一帧左键是否按下，配 `poll_outside_click` 认按下沿
    lbtn_was_down: bool,
    /// 状态提示卡片什么时候自己收掉
    settings_open: bool,
    settings_tab: SettingsTab,
    /// 换页后把滚动条拉回顶部
    settings_scroll_top: bool,
    /// 设置窗这次打开是否已经摆过位置
    /// 设置窗还要再摆几帧位置。**不能只摆一帧**：窗口刚从「缩在屏幕外的 1x1 浮窗宿主」
    /// 切成设置窗时，`InnerSize` / `OuterPosition` 有可能在窗口还没完成状态切换时被丢掉，
    /// 表现是设置窗打开了（标题栏都变了）却停在 -16000,-16000 看不见。
    /// 安装版启动路径更长、时序不同，实测就撞上了这个（开发版一直没复现）。
    settings_place_frames: u8,
    /// 摆位的目标（逻辑坐标）。摆完读回实际位置与它比对，偏了就重摆；对上了就清空——
    /// 留着的话用户拖动设置窗会被一直拽回原位
    settings_want_pos: Option<egui::Pos2>,
    /// 已经重摆过几次，防止和窗口管理器互相较劲成死循环
    settings_place_retries: u8,
    draft: Config,
    settings_msg: String,
    /// 运行环境自检结果。None = 还没查或要求重查；查一次要读注册表、枚举 PATH、
    /// 列 OCR 语言包，不能放在每帧重绘里
    env_items: Option<Vec<desktop_shell::deps::Item>>,
    status: String,
    /// 服务句柄，重启服务时替换
    server: Option<crate::server::ServerHandle>,
    /// 实时字幕子进程。装了才可用；简译退出时它会被优雅关掉（见 caption.rs 的 Drop）
    caption: crate::caption::CaptionHost,
    /// 持有着就在，丢掉图标就没了
    _tray: Option<tray_icon::TrayIcon>,
    paused: bool,
    quit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HotkeyAction {
    /// 一次性动作：直接进入截屏框选
    Screenshot,
    TogglePanel,
    /// 两段式的第一段：开关某个功能的「待命」状态。
    /// Jack 2026-09-09 定的模型：三键组合按着别扭，只用来开关功能；
    /// 开着的时候用单键/双键 + 鼠标动作调用（悬停按 Ctrl、划词直接选中）。
    ToggleHover,
    ToggleSelection,
    /// 字幕：无条件切回可点击。**父进程只转发一个字幕热键的话，必须是这个**——
    /// 「切换穿透」在状态不同步时可能反而把穿透打开，而这一条永远往安全方向走。
    CaptionClickable,
    CaptionPassthrough,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> anyhow::Result<App> {
        desktop_shell::fonts::install(&cc.egui_ctx);
        let translator = Arc::new(Mutex::new(ai_translate::Translator::new(cfg.translator_config())?));
        let capturer = Arc::new(Capturer::new());
        let (engine, events) = Engine::spawn(translator.clone(), capturer.clone());
        // 把开关同步给引擎：不同步的话用户关掉了，重启又会自己回来
        engine.set_offline(cfg.ui.offline_dict);
        let (trig_tx, triggers) = crossbeam_channel::unbounded();
        let watcher = crate::watcher::spawn(watch_cfg(&cfg), trig_tx);

        let hk = GlobalHotKeyManager::new().map_err(|e| anyhow::anyhow!("热键管理器初始化失败：{e}"))?;
        let mut hotkey_ids = Vec::new();
        let mut hotkey_notes = Vec::new();
        let mut taken: Vec<String> = Vec::new();
        for (want, action, label) in [
            (&cfg.hotkeys.screenshot, HotkeyAction::Screenshot, "截屏"),
            (&cfg.hotkeys.toggle_hover, HotkeyAction::ToggleHover, "悬停开关"),
            (&cfg.hotkeys.toggle_selection, HotkeyAction::ToggleSelection, "划词开关"),
            (&cfg.hotkeys.toggle_panel, HotkeyAction::TogglePanel, "面板"),
            // 字幕的两个热键由简译统一注册再转发：托管模式下字幕自己不注册，
            // 两个进程各注册一遍的话后者必然失败（组合已被占）。
            (&cfg.hotkeys.caption_clickable, HotkeyAction::CaptionClickable, "字幕切回可点击"),
            (&cfg.hotkeys.caption_passthrough, HotkeyAction::CaptionPassthrough, "字幕穿透"),
        ] {
            match register_with_fallback(&hk, want, &taken) {
                Some((id, used)) => {
                    if used != *want {
                        tracing::warn!("{label}热键 {want} 被其他程序占用，已自动改用 {used}");
                        hotkey_notes.push(format!("{label} {used}（{want} 被占用）"));
                    } else {
                        hotkey_notes.push(format!("{label} {used}"));
                    }
                    taken.push(used);
                    hotkey_ids.push((id, action));
                }
                None => {
                    tracing::error!("{label}热键 {want} 与所有备选都注册不上，请在设置里换一个组合");
                    hotkey_notes.push(format!("{label} 未注册"));
                }
            }
        }

        let server = if cfg.server.enabled {
            match crate::server::spawn(cfg.server.port, cfg.server.allowed_extension_ids.clone(), translator.clone(), APP_VERSION) {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::error!("{e}");
                    None
                }
            }
        } else {
            None
        };

        // 一次性把 DPI 口径写进日志：Win32 坐标与 egui 窗口定位的单位不一致时，
        // 所有跟位置有关的功能（划词图标、译文面板、截屏遮罩）都会整体漂移
        #[cfg(windows)]
        unsafe {
            windows_link::link!("user32.dll" "system" fn GetDpiForSystem() -> u32);
            windows_link::link!("user32.dll" "system" fn GetThreadDpiAwarenessContext() -> isize);
            windows_link::link!("user32.dll" "system" fn GetAwarenessFromDpiAwarenessContext(value: isize) -> i32);
            let dpi = GetDpiForSystem();
            let aware = GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext());
            let (vx, vy, vw, vh) = text_capture::screen::virtual_screen();
            tracing::info!(
                "DPI 探测：system_dpi={dpi}（缩放 {}%） awareness={aware} 虚拟屏=({vx},{vy}) {vw}x{vh} egui_ppp={}",
                dpi * 100 / 96,
                cc.egui_ctx.pixels_per_point()
            );
        }

        let status = format!(
            "取词 {} · 识别 {} · 服务 {} · 热键 {}",
            if capturer.has_uia() { "就绪" } else { "不可用" },
            capturer.ocr_language().unwrap_or_else(|| "不可用".into()),
            server.as_ref().map(|s| format!("{}", s.port)).unwrap_or_else(|| "未启动".into()),
            hotkey_notes.join("，"),
        );
        tracing::info!("{status}");

        // 调试钩子：TT_MAIN_INPUT=<文字> 启动即开主窗口、填好输入并翻一次（截图与自测用）
        let main_input = std::env::var("TT_MAIN_INPUT").ok().filter(|t| !t.trim().is_empty());
        // TT_MAIN_LIVE=1 配合上面：只填进去走「边输入边查」，不提交
        let main_live_only = std::env::var("TT_MAIN_LIVE").is_ok_and(|v| v == "1");
        let open_settings_at_start = main_input.is_some() || std::env::var("TT_OPEN_SETTINGS").is_ok_and(|v| v != "0");
        Ok(App {
            draft: cfg.clone(),
            cfg,
            translator,
            capturer,
            engine,
            events,
            triggers,
            watcher,
            _hotkeys: hk,
            hotkey_ids,
            mode: Mode::Idle,
            panel: None,
            pending_region: None,
            focus_selector: false,
            pending_tray_click: None,
            last_place: None,
            fps: (Instant::now(), 0, 0),
            place_tries: 0,
            logged_icon_at: None,
            logged_card_w: None,
            about_dict: None,
            settings_sized_as: None,
            settings_unminimize: false,
            manual: crate::manual::ManualState {
                autosubmit: main_input.is_some() && !main_live_only,
                edited_at: (main_input.is_some() && main_live_only).then(Instant::now),
                input: main_input.clone().unwrap_or_default(),
                ..Default::default()
            },
            settings_raise: false,
            logged_content_w: None,
            cur_seq: 0,
            pending_panel: None,
            overlay: None,
            overlay_rect: None,
            lbtn_was_down: false,
            // 调试钩子：设 TT_OPEN_SETTINGS=1 启动即开设置窗（截图与自测用）
            settings_open: open_settings_at_start,
            // TT_SETTINGS_TAB=0..3 指定启动时停在哪一页（截图与自测用）
            settings_tab: std::env::var("TT_SETTINGS_TAB")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .and_then(|i| SettingsTab::ALL.get(i).copied())
                .unwrap_or(if main_input.is_some() { SettingsTab::Translate } else { SettingsTab::Service }),
            settings_scroll_top: false,
            // 语义是「还要摆几帧」：0 表示不用摆。启动即开设置窗时必须一起给上，
            // 否则窗口留在屏幕外的 -32000，标题栏都变了却看不见（踩过）
            settings_place_frames: if open_settings_at_start { PLACE_FRAMES } else { 0 },
            settings_want_pos: None,
            settings_place_retries: 0,
            settings_msg: String::new(),
            env_items: None,
            status,
            caption: crate::caption::CaptionHost::new(server.as_ref().map(|s| s.port).unwrap_or(0)),
            server,
            _tray: match crate::tray::build_app_tray(&format!("{APP_NAME_ZH} · 划词悬停截屏翻译")) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!("{e}");
                    None
                }
            },
            paused: false,
            quit: false,
        })
        .map(|mut app| {
            // 调试钩子：TT_DEMO_WORD=glance 启动即查一次词并弹卡片（截图与验收用）
            if let Ok(w) = std::env::var("TT_DEMO_WORD") {
                if !w.trim().is_empty() {
                    // TT_DEMO_AT=x,y 指定锚点（物理），不给了默认 (600,400)：验摆放方位用
                    let at = std::env::var("TT_DEMO_AT")
                        .ok()
                        .and_then(|v| {
                            let n: Vec<i32> = v.split(',').filter_map(|t| t.trim().parse().ok()).collect();
                            match n[..] {
                                [x, y] => Some((x, y)),
                                _ => None,
                            }
                        })
                        .unwrap_or((600, 400));
                    app.start(Job::Text { scene: Scene::Word, text: w.trim().into() }, Scene::Word.label_zh(), at, Source::Clipboard);
                    // 钉住卡片会跳过摆放逻辑（钉住后用 pos 不再 place），验摆放方位时不能钉：
                    // 只有 TT_DEMO_PIN=1 才钉；不钉时靠截图赶在自动收起之前（宽限 3 秒）
                    if std::env::var("TT_DEMO_PIN").is_ok_and(|v| v == "1") {
                        if let Some(p) = app.panel.as_mut() {
                            p.pinned = true;
                        }
                    }
                }
            }
            // 调试钩子：TT_DEMO_REGION=x,y,w,h 启动即对这块屏幕做一次截屏翻译并钉住卡片。
            // 截屏链路靠热键触发，而注入的按键不一定能触发系统热键，自动化只能走这条路。
            if let Ok(r) = std::env::var("TT_DEMO_REGION") {
                let n: Vec<i32> = r.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                if let [x, y, w, h] = n[..] {
                    // 跟着配置走：默认覆盖式，这样调试钩子验的就是用户真正看到的那条路
                    let overlay = app.cfg.ui.region_overlay;
                    app.overlay_rect = overlay.then_some((x, y, w as u32, h as u32));
                    app.start(
                        Job::Region { x, y, w: w as u32, h: h as u32, overlay },
                        "截屏翻译",
                        (x, y + h),
                        Source::Ocr,
                    );
                    if let Some(p) = app.panel.as_mut() {
                        p.source_rect = Some((x, y, w as u32, h as u32));
                        p.pinned = true;
                        // TT_DEMO_TEXT 直接把文字塞进卡片，绕开识别与翻译：
                        // 排版问题与识别无关，锁屏时又抓不到有字的画面
                        if let Ok(t) = std::env::var("TT_DEMO_TEXT") {
                            p.original = t.clone();
                            p.translated = t;
                            p.phase = crate::panel::Phase::Done;
                        }
                    }
                }
            }
            app
        })
    }

    fn start(&mut self, job: Job, scene_label: &'static str, anchor: (i32, i32), source: Source) {
        tracing::info!("{scene_label} 开始，锚点物理({},{}) job={job:?}", anchor.0, anchor.1);
        self.cur_seq = self.engine.submit(job);
        self.panel = Some(PanelState::new(scene_label, source, anchor));
        self.mode = Mode::Panel;
    }

    /// 与 [`Self::start`] 一样发任务，但**先不弹卡片**：等第一条有内容的回包再弹。
    ///
    /// 悬停专用。悬停是鼠标扫过就触发的被动动作，取到的若是目标语言（比如英译中时扫到中文），
    /// 整张卡片都不该出现；先弹再收会闪一下，比不弹更烦人。
    fn start_deferred(&mut self, job: Job, scene_label: &'static str, anchor: (i32, i32), source: Source) {
        tracing::info!("{scene_label} 开始（先不弹卡片），锚点物理({},{}) job={job:?}", anchor.0, anchor.1);
        self.cur_seq = self.engine.submit(job);
        self.panel = None;
        self.pending_panel = Some((scene_label, source, anchor));
    }

    /// 该点所在进程是否在例外列表里。
    /// 命中会静默吞掉悬停与小图标，所以一定要打日志：不然表现就是「按住 Alt 没反应」。
    fn excluded_at(&self, x: i32, y: i32) -> Option<String> {
        if self.cfg.ui.excluded_processes.is_empty() {
            return None;
        }
        let p = self.capturer.process_at(x, y)?;
        if self.cfg.ui.excluded_processes.iter().any(|e| e.eq_ignore_ascii_case(&p)) {
            tracing::debug!("({x},{y}) 属于 {p}，在例外进程列表里，不响应");
            Some(p)
        } else {
            None
        }
    }

    fn drain_triggers(&mut self) {
        while let Ok(t) = self.triggers.try_recv() {
            // 主窗口 / 设置窗占着根视口，浮动卡片画不出来；这时照样取词翻译只会白花请求，
            // 在主窗口输入框里拖选、按住 Ctrl 打字还会误触发
            if self.settings_open {
                tracing::debug!("窗口开着，忽略浮窗触发 {t:?}");
                continue;
            }
            match t {
                Trigger::SelectionEnded { x, y } => {
                    if !self.cfg.ui.selection_icon {
                        tracing::debug!("划词结束于({x},{y})，但设置里关了小图标");
                    } else if matches!(self.mode, Mode::Selecting { .. }) {
                        tracing::debug!("划词结束于({x},{y})，但正在框选，忽略");
                    } else if self.excluded_at(x, y).is_none() {
                        tracing::debug!("划词结束于物理({x},{y})，弹小图标");
                        let target = text_capture::clipboard::foreign_foreground_window();
                        tracing::info!("划词结束，弹图标；取词目标窗口 {target:?}");
                        let clip_seq = self.watcher.clip_seq_at_press();
                        self.mode = Mode::Icon { pos: (x, y), since: Instant::now(), target, clip_seq };
                    }
                }
                Trigger::Hovered { x, y } if !self.cfg.ui.hover_enabled => {
                    tracing::debug!("悬停触发于({x},{y})，但悬停取词没开（按开关热键可开）");
                }
                Trigger::Hovered { x, y } => {
                    if !matches!(self.mode, Mode::Idle | Mode::Icon { .. }) {
                        tracing::debug!("悬停触发于({x},{y})，但界面正忙（{:?}），忽略", self.mode);
                    } else if let Some(proc) = self.excluded_at(x, y) {
                        // 别静默吞掉：用户看到的会是「按住 Alt 完全没反应」，无从查起
                        tracing::info!("{proc} 在例外进程列表里，悬停不响应（设置里可移除）");
                        let mut st = PanelState::new("悬停翻译", Source::Uia, (x, y));
                        st.phase = Phase::Notice(format!("{proc} 在例外列表里
设置 → 触发方式里可以移除"));
                        self.panel = Some(st);
                        self.mode = Mode::Panel;
                        self.cur_seq = 0;
                    } else {
                        // 悬停不立刻弹卡片：取到的可能是目标语言（那就整张不出），
                        // 先记下来，等真取到要翻的内容再弹
                        let skip = self.cfg.ui.hover_skip_target_lang;
                        self.start_deferred(Job::Hover { x, y, skip_target_lang: skip }, "悬停翻译", (x, y), Source::Uia);
                    }
                }
            }
        }
    }

    fn drain_hotkeys(&mut self) {
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != global_hotkey::HotKeyState::Pressed {
                continue;
            }
            let Some((_, action)) = self.hotkey_ids.iter().find(|(id, _)| *id == ev.id).copied() else { continue };
            let (x, y) = text_capture::screen::cursor_pos();
            match action {
                HotkeyAction::ToggleHover => self.toggle_feature(Feature::Hover, (x, y)),
                HotkeyAction::CaptionClickable | HotkeyAction::CaptionPassthrough => {
                    let cmd = if action == HotkeyAction::CaptionClickable {
                        desktop_shell::caption_ipc::Command::Clickable
                    } else {
                        desktop_shell::caption_ipc::Command::TogglePassthrough
                    };
                    // 字幕没跑就静默忽略：这两个键对没开字幕的用户没有意义，
                    // 每次按都弹一张卡片反而烦人
                    if self.caption.running() {
                        if let Err(e) = self.caption.send(cmd) {
                            tracing::warn!("字幕热键命令没送到：{e}");
                        }
                    }
                }
                HotkeyAction::ToggleSelection => self.toggle_feature(Feature::Selection, (x, y)),
                HotkeyAction::Screenshot => {
                    tracing::info!("进入截屏框选");
                    self.mode = Mode::Selecting { start: None };
                    self.panel = None;
                    self.pending_region = None;
                    self.focus_selector = true;
                }
                HotkeyAction::TogglePanel => {
                    if self.panel.is_some() {
                        self.close_panel();
                    } else {
                        // 热键触发：用户的程序本来就在前台，不用切
                        self.start(Job::Selection { target: None, clip_seq: None }, "划词翻译", (x, y), Source::Clipboard);
                    }
                }
            }
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(e) = self.events.try_recv() {
            let seq = match &e {
                Event::Shot { seq, .. }
                | Event::Line { seq, .. }
                | Event::Offline { seq, .. }
                | Event::Captured { seq, .. }
                | Event::Delta { seq, .. }
                | Event::Done { seq, .. }
                | Event::Failed { seq, .. }
                | Event::Nothing { seq, .. }
                | Event::Skipped { seq } => *seq,
            };
            // 主窗口手动翻译的回包走自己那一份状态，不碰浮动卡片
            if self.manual.owns(seq) {
                self.manual.on_event(e);
                continue;
            }
            if seq != self.cur_seq {
                continue; // 过期
            }
            // 覆盖式截屏：原图先到，立刻铺上去；译文一行一行补进来
            if let Event::Shot { shot, .. } = &e {
                self.begin_overlay(shot.clone(), ctx);
                continue;
            }
            if let Event::Line { idx, text, .. } = &e {
                if let Some(o) = self.overlay.as_mut() {
                    if let Some(l) = o.lines.get_mut(*idx) {
                        l.dst = Some(text.clone());
                    }
                }
                continue;
            }
            // 覆盖已经铺出来了：Done 只用来撤掉「翻译中」的提示条，不弹卡片。
            // **没铺出来的失败不能吞**（识别不到文字就是这种）：覆盖模式先不弹卡片，
            // 再把失败也吞掉的话，用户框完一片什么反馈都没有（2026-09-14 实测撞到）。
            if self.overlay.is_some() {
                if let Event::Done { .. } | Event::Failed { .. } | Event::Nothing { .. } = &e {
                    if let Some(o) = self.overlay.as_mut() {
                        o.working = false;
                    }
                    if let Event::Failed { error, .. } = &e {
                        tracing::warn!("截屏覆盖失败：{error}");
                    }
                    continue;
                }
            }
            // 不用翻：静默收掉，连卡片都不弹（开关在设置里，默认开）
            if let Event::Skipped { .. } = e {
                tracing::info!("取到的已经是目标语言，跳过翻译（设置里可关掉「遇目标语言不弹卡片」）");
                self.pending_panel = None;
                self.close_panel();
                continue;
            }
            // 延后弹卡片的任务（悬停）：到这里说明确实有要翻的内容，补弹
            if self.panel.is_none() {
                if let Some((label, source, anchor)) = self.pending_panel.take() {
                    self.panel = Some(PanelState::new(label, source, anchor));
                    self.mode = Mode::Panel;
                }
            }
            let Some(p) = self.panel.as_mut() else { continue };
            match e {
                Event::Captured { text, source, scene, took_ms, .. } => {
                    p.original = text;
                    p.source = source;
                    // 取到的是单个词时引擎会改判成词典场景，标题跟着改，重试也才会走对提示词
                    p.scene_label = scene.label_zh();
                    let doing = if scene == Scene::Word { "查词中…" } else { "翻译中…" };
                    p.phase = Phase::Working(format!("{} {took_ms}ms · {doing}", source.label_zh()));
                }
                // 离线释义：先上屏顶着，等模型的词条到了再被下面的 Delta / Done 覆盖
                Event::Offline { text, .. } => {
                    if p.translated.trim().is_empty() {
                        p.translated = text;
                        p.offline = true;
                        p.phase = Phase::Working("离线释义 · 正在等模型…".into());
                    }
                }
                Event::Delta { text, .. } => {
                    p.translated = text;
                    p.offline = false;
                }
                Event::Done { text, took_ms, .. } => {
                    p.translated = text;
                    p.offline = false;
                    p.phase = Phase::Done;
                    tracing::info!("{} 完成 {took_ms}ms", p.scene_label);
                }
                // 失败与落空必须留 INFO 级日志：安装版默认只写 INFO，
                // 1.0.3 用户报「没有选中文字 / 取词无效」时日志里一个字都没有，无从查起
                Event::Failed { error, .. } => {
                    tracing::warn!("{} 失败：{error}", p.scene_label);
                    // 离线释义已经在卡片上了就留着，只把状态改成提示——
                    // 模型没回来不等于没东西可看，红字盖掉反而更糟
                    if p.offline {
                        p.phase = Phase::Notice(format!("模型没回来，显示的是离线词典的释义（{error}）"));
                    } else {
                        p.phase = Phase::Failed(error);
                    }
                }
                Event::Nothing { reason, .. } => {
                    tracing::info!("{} 没取到：{reason}", p.scene_label);
                    p.phase = Phase::Failed(reason);
                }
                // 上面已经处理并 continue 了，这三条只是让编译器满意
                Event::Skipped { .. } | Event::Shot { .. } | Event::Line { .. } => {}
            }
        }

    }

    /// 托盘点击 → 自己弹原生菜单（不挂 muda，见 tray.rs 顶部说明）
    fn drain_tray(&mut self, _ctx: &egui::Context, frame: &mut eframe::Frame) {
        use crate::tray::{
            MENU_CAPTION_CLICKABLE, MENU_CAPTION_MINI, MENU_CAPTION_PAUSE, MENU_CAPTION_SETTINGS, MENU_CAPTION_TOGGLE,
            MENU_DIRECTION, MENU_MAIN, MENU_PAUSE, MENU_QUIT, MENU_SETTINGS,
        };
        use desktop_shell::caption_ipc::Command;
        use desktop_shell::tray::{Action, Input};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

        // 双击托盘 = 开设置窗；单击弹菜单。判定逻辑（含「双击要撤销挂起的单击」）
        // 在 `desktop_shell::tray::reduce` 里，有单测盯着时序。
        let mut inputs = Vec::new();
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            match ev {
                TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => inputs.push(Input::LeftDouble),
                TrayIconEvent::Click { position, button, button_state: MouseButtonState::Up, .. } => {
                    let at = (position.x as i32, position.y as i32);
                    inputs.push(if button == MouseButton::Left { Input::LeftUp(at) } else { Input::RightUp(at) });
                }
                _ => {}
            }
        }
        let interval = desktop_shell::tray::double_click_interval();
        let (x, y) = match desktop_shell::tray::reduce(inputs, &mut self.pending_tray_click, Instant::now(), interval) {
            Action::Activate => {
                tracing::info!("双击托盘，打开主窗口");
                self.open_main();
                return;
            }
            Action::Menu(at) => at,
            Action::Nothing => return,
        };
        let hwnd = window_handle_of(frame);
        let mut items = vec![
            (MENU_MAIN, "翻译窗口…".to_string(), false),
            (MENU_SETTINGS, "设置…".to_string(), false),
            (MENU_PAUSE, "暂停取词".to_string(), self.paused),
            (
                MENU_DIRECTION,
                format!("翻译方向：{}", ai_translate::Direction::parse(&self.cfg.direction).label_zh()),
                false,
            ),
            (0, String::new(), false),
        ];
        for f in Feature::ALL {
            items.push((f.menu_id(), f.menu_label(&self.cfg), f.get(&self.cfg)));
        }
        // 实时字幕那一块。装了才显示——没装时列一堆点不动的项只会让人以为坏了。
        // 弹菜单前要**同步**读一次子进程的勾选态：暂停/迷你/穿透三个勾是它的状态，
        // 父进程自己猜不出来（所以控制端点必须双向，不能只做单向命令）。
        let caption_running = self.caption.running();
        let caption_state = if caption_running { self.caption.state() } else { None };
        if self.caption.installed() {
            items.push((0, String::new(), false));
            items.push((MENU_CAPTION_TOGGLE, (if caption_running { "关闭实时字幕" } else { "开启实时字幕" }).to_string(), caption_running));
            if let Some(st) = caption_state {
                items.push((MENU_CAPTION_PAUSE, (if st.paused { "字幕：继续识别" } else { "字幕：暂停识别" }).to_string(), st.paused));
                items.push((MENU_CAPTION_MINI, (if st.mini { "字幕：展开字幕条" } else { "字幕：收起为迷你条" }).to_string(), st.mini));
                items.push((MENU_CAPTION_CLICKABLE, "字幕：切回可点击".to_string(), !st.passthrough));
                items.push((MENU_CAPTION_SETTINGS, "字幕设置…".to_string(), false));
            }
        }
        items.push((0, String::new(), false));
        items.push((MENU_QUIT, "退出".to_string(), false));
        match crate::tray::show_menu(hwnd, x, y, &items) {
            Some(MENU_MAIN) => self.open_main(),
            Some(MENU_SETTINGS) => self.open_settings(),
            Some(MENU_DIRECTION) => self.flip_direction(),
            Some(id) if Feature::from_menu_id(id).is_some() => {
                self.toggle_feature(Feature::from_menu_id(id).unwrap(), (x, y));
            }
            Some(MENU_PAUSE) => {
                self.paused = !self.paused;
                self.watcher.set_paused(self.paused);
                tracing::info!("取词已{}", if self.paused { "暂停" } else { "恢复" });
            }
            Some(MENU_CAPTION_TOGGLE) => {
                if caption_running {
                    self.caption.stop();
                } else if let Err(e) = self.caption.start() {
                    tracing::error!("{e}");
                    self.notice("实时字幕", format!("{e}"), (x, y));
                }
            }
            Some(id @ (MENU_CAPTION_PAUSE | MENU_CAPTION_MINI | MENU_CAPTION_CLICKABLE | MENU_CAPTION_SETTINGS)) => {
                let cmd = match id {
                    MENU_CAPTION_PAUSE => Command::TogglePause,
                    MENU_CAPTION_MINI => Command::ToggleMini,
                    MENU_CAPTION_CLICKABLE => Command::Clickable,
                    _ => Command::OpenSettings,
                };
                if let Err(e) = self.caption.send(cmd) {
                    tracing::warn!("字幕命令 {} 没送到：{e}", cmd.as_str());
                }
            }
            Some(MENU_QUIT) => self.request_quit(),
            _ => {}
        }
    }

    /// 切换一个功能开关：热键与托盘菜单都走这里。
    ///
    /// 改的是 `cfg`，**同时要改 `draft`**——否则设置窗的「保存并应用」无端亮蓝、
    /// 或者用户随后在设置窗点保存又把这次切换覆盖回去。
    fn toggle_feature(&mut self, f: Feature, at: (i32, i32)) {
        let on = !f.get(&self.cfg);
        f.set(&mut self.cfg, on);
        f.set(&mut self.draft, on);
        match f {
            Feature::Hover => self.watcher.set_config(watch_cfg(&self.cfg)),
            Feature::OfflineDict => self.engine.set_offline(on),
            // 其余几项每次触发时现读配置，改完即生效
            Feature::Selection | Feature::RegionOverlay | Feature::HoverSkip => {}
        }
        tracing::info!("{}{}", f.name(), if on { "已开启" } else { "已关闭" });
        let msg = f.notice(&self.cfg, on);
        self.notice(f.name(), msg, at);
        if let Err(e) = self.cfg.save() {
            tracing::warn!("开关已切换但配置没存上：{e}");
        }
    }

    /// 弹一张只报状态的卡片（没有重试按钮），1.6 秒后自己收
    fn notice(&mut self, title: &'static str, msg: String, at: (i32, i32)) {
        let mut st = PanelState::new(title, Source::Uia, at);
        st.phase = Phase::Notice(msg);
        self.panel = Some(st);
        self.mode = Mode::Panel;
        self.cur_seq = 0;
    }

    fn close_panel(&mut self) {
        self.logged_icon_at = None;
        self.panel = None;
        self.mode = Mode::Idle;
    }

    /// 设置窗里有没有还没保存的改动。
    ///
    /// 直接比 `draft` 与 `cfg`：打开设置窗时 `draft` 从 `cfg` 克隆，保存成功时 `cfg` 取 `draft`。
    /// 所以**任何绕开设置窗改 `cfg` 的地方都要同步改 `draft`**（热键开关悬停/划词就是这么做的），
    /// 否则用户什么都没动，保存按钮也亮着。
    fn settings_dirty(&self) -> bool {
        self.draft != self.cfg
    }

    /// 换翻译方向（托盘菜单）。**必须重建 translator**：方向进的是提示词，
    /// 只改配置不重建的话，切了以后还按老方向翻，用户会以为开关坏了
    fn flip_direction(&mut self) {
        let next = ai_translate::Direction::parse(&self.cfg.direction).flip();
        self.cfg.direction = next.as_str().to_string();
        self.draft.direction = self.cfg.direction.clone();
        match ai_translate::Translator::new(self.cfg.translator_config()) {
            Ok(t) => *self.translator.lock().unwrap() = t,
            Err(e) => tracing::warn!("换方向后重建翻译器失败：{e}"),
        }
        tracing::info!("翻译方向切换为{}", next.label_zh());
        let (x, y) = text_capture::screen::cursor_pos();
        self.notice("翻译方向", format!("已切换为{}", next.label_zh()), (x, y));
        let _ = self.cfg.save();
    }

    fn apply_settings(&mut self) {
        match ai_translate::Translator::new(self.draft.translator_config()) {
            Ok(t) => {
                *self.translator.lock().unwrap() = t;
                self.cfg = self.draft.clone();
                self.watcher.set_config(watch_cfg(&self.cfg));
                self.engine.set_offline(self.cfg.ui.offline_dict);
                match self.cfg.save() {
                    Ok(()) => self.settings_msg = "已保存并生效（热键与服务端口改动需重启应用）".into(),
                    Err(e) => self.settings_msg = format!("保存失败：{e}"),
                }
            }
            Err(e) => self.settings_msg = format!("设置有误：{e}"),
        }
    }
}

/// 先试用户配的，占用了就依次试备选（跳过本次已用掉的）。返回 (热键 id, 实际用的组合)
fn register_with_fallback(hk: &GlobalHotKeyManager, want: &str, taken: &[String]) -> Option<(u32, String)> {
    let mut candidates: Vec<String> = vec![want.to_string()];
    candidates.extend(FALLBACK_HOTKEYS.iter().map(|s| s.to_string()));
    for c in candidates {
        if taken.iter().any(|t| t.eq_ignore_ascii_case(&c)) {
            continue;
        }
        let Ok(k) = parse_hotkey(&c) else { continue };
        if hk.register(k).is_ok() {
            return Some((k.id(), c));
        }
    }
    None
}

/// 托盘菜单与热键都能切的功能开关。每项对应 `UiCfg` 里的一个 bool。
///
/// 加一项要改这里的 `ALL` / `menu_id` / 读写 / 文案，外加 `tray.rs` 的常量；
/// 生效方式不是「每次现读配置」的，还要在 `App::toggle_feature` 里补一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Hover,
    Selection,
    RegionOverlay,
    OfflineDict,
    HoverSkip,
}

impl Feature {
    pub const ALL: [Feature; 5] =
        [Feature::Hover, Feature::Selection, Feature::RegionOverlay, Feature::OfflineDict, Feature::HoverSkip];

    pub fn menu_id(self) -> u32 {
        use crate::tray::*;
        match self {
            Feature::Hover => MENU_HOVER,
            Feature::Selection => MENU_SELECTION,
            Feature::RegionOverlay => MENU_REGION_OVERLAY,
            Feature::OfflineDict => MENU_OFFLINE_DICT,
            Feature::HoverSkip => MENU_HOVER_SKIP,
        }
    }

    pub fn from_menu_id(id: u32) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.menu_id() == id)
    }

    pub fn get(self, cfg: &Config) -> bool {
        match self {
            Feature::Hover => cfg.ui.hover_enabled,
            Feature::Selection => cfg.ui.selection_icon,
            Feature::RegionOverlay => cfg.ui.region_overlay,
            Feature::OfflineDict => cfg.ui.offline_dict,
            Feature::HoverSkip => cfg.ui.hover_skip_target_lang,
        }
    }

    pub fn set(self, cfg: &mut Config, on: bool) {
        match self {
            Feature::Hover => cfg.ui.hover_enabled = on,
            Feature::Selection => cfg.ui.selection_icon = on,
            Feature::RegionOverlay => cfg.ui.region_overlay = on,
            Feature::OfflineDict => cfg.ui.offline_dict = on,
            Feature::HoverSkip => cfg.ui.hover_skip_target_lang = on,
        }
    }

    /// 通知卡片的标题
    pub fn name(self) -> &'static str {
        match self {
            Feature::Hover => "悬停取词",
            Feature::Selection => "划词翻译",
            Feature::RegionOverlay => "截屏译文覆盖",
            Feature::OfflineDict => "离线词典",
            Feature::HoverSkip => "悬停跳过目标语言",
        }
    }

    /// 菜单上的字：把「怎么用」带上，光一个名字用户不知道勾了会怎样
    pub fn menu_label(self, cfg: &Config) -> String {
        match self {
            Feature::Hover => {
                format!("悬停取词（按住 {}）", HoverModifier::parse(&cfg.hotkeys.hover_modifier).label_zh())
            }
            Feature::Selection => "划词后弹「译」图标".into(),
            Feature::RegionOverlay => "截屏译文盖回原文位置".into(),
            Feature::OfflineDict => "查词先给离线释义".into(),
            Feature::HoverSkip => format!(
                "悬停到{}时不弹卡片",
                ai_translate::Direction::parse(&cfg.direction).target_zh()
            ),
        }
    }

    pub fn notice(self, cfg: &Config, on: bool) -> String {
        match (self, on) {
            (Feature::Hover, true) => format!(
                "已开启：按住 {} 把鼠标停在文字上就翻",
                HoverModifier::parse(&cfg.hotkeys.hover_modifier).label_zh()
            ),
            (Feature::Hover, false) => "已关闭：按住修饰键也不再响应".into(),
            (Feature::Selection, true) => "已开启：选中文字后点旁边的「译」".into(),
            (Feature::Selection, false) => "已关闭：选中文字不再弹图标（快捷键仍可用）".into(),
            (Feature::RegionOverlay, true) => "已开启：截屏后译文盖在原图上，点一下切换原文".into(),
            (Feature::RegionOverlay, false) => "已关闭：截屏后给一张整段译文卡片".into(),
            (Feature::OfflineDict, true) => "已开启：查单词先秒出本地释义，模型回来再替换".into(),
            (Feature::OfflineDict, false) => "已关闭：查单词只等模型".into(),
            (Feature::HoverSkip, true) => "已开启：悬停到目标语言的文字不再弹卡片".into(),
            (Feature::HoverSkip, false) => "已关闭：悬停到目标语言的文字也会弹卡片".into(),
        }
    }
}

fn watch_cfg(cfg: &Config) -> WatchConfig {
    WatchConfig {
        // 功能没开就让监视线程直接别判定，省得白跑
        hover_modifier: if cfg.ui.hover_enabled {
            HoverModifier::parse(&cfg.hotkeys.hover_modifier)
        } else {
            HoverModifier::None
        },
        hover_delay: Duration::from_millis(cfg.hotkeys.hover_delay_ms.clamp(80, 5000)),
        ..Default::default()
    }
}

impl eframe::App for App {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// 窗口被系统判定为不可见时，eframe **只跑这里、不跑 `ui()`**
    /// （见 eframe 的 `update_logic_only`）。本应用平时就把主窗口缩成 1×1 丢在
    /// -32000，关掉设置窗之后 Windows 会发 `Occluded(true)`，于是 `ui()` 停摆——
    /// 表现是「关了设置窗以后托盘双击、热键全都没反应了」，而进程还活着，极难查。
    ///
    /// 所以事件泵必须在这里也跑一遍，并在有活儿时 `request_repaint` 把绘制拉回来。
    /// **往 `ui()` 开头加新的事件泵时，记得同步加到这里。**
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.quit {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            return;
        }
        self.drain_hotkeys();
        self.drain_triggers();
        self.drain_events(ctx);
        self.drain_tray(ctx, frame);
        self.poll_outside_click();
        if self.settings_open && self.settings_unminimize {
            self.settings_unminimize = false;
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        self.caption.reap();
        self.count_frame(false);
        // 有事要显示就把绘制拉回来；没事也保持一个低频心跳，
        // 否则托盘与热键这类外部事件没有任何东西驱动
        ctx.request_repaint_after(Duration::from_millis(if self.settings_open || self.mode != Mode::Idle { 16 } else { 100 }));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.quit {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            return;
        }
        self.drain_hotkeys();
        self.drain_triggers();
        self.drain_events(&ctx);
        self.drain_tray(&ctx, _frame);
        self.poll_outside_click();
        // 回收退出的字幕子进程，并按退出码决定要不要重新拉起（改识别参数会请求重启）。
        // 用 try_wait 不用 wait：后者会把 UI 线程堵死。
        self.caption.reap();

        // 框选完等遮罩真的从屏幕上撤走再抓图（见 draw_region_selector 末尾）
        if let Some((rect, at)) = self.pending_region {
            if at.elapsed() >= MASK_CLEAR_DELAY {
                self.pending_region = None;
                let (x, y, w, h) = rect;
                let overlay = self.cfg.ui.region_overlay;
                self.overlay_rect = overlay.then_some((x, y, w, h));
                if overlay {
                    // 覆盖模式没有卡片：原图会铺回框选的位置，译文盖在每行原文上
                    self.start_deferred(Job::Region { x, y, w, h, overlay }, "截屏翻译", (x, y + h as i32), Source::Ocr);
                } else {
                    self.start(Job::Region { x, y, w, h, overlay }, "截屏翻译", (x, y + h as i32), Source::Ocr);
                    // 卡片与框选区同宽、左边缘对齐（Jack 2026-09-13 要求），摆放时按它算
                    if let Some(p) = self.panel.as_mut() {
                        p.source_rect = Some((x, y, w, h));
                    }
                }
            }
        }

        // 小图标超时消失
        if let Mode::Icon { since, .. } = self.mode {
            if since.elapsed() > Duration::from_millis(self.cfg.ui.icon_timeout_ms) {
                self.mode = Mode::Idle;
            }
        }

        // 设置界面独占主视口（原因见 draw_settings 的注释）
        if self.settings_open {
            self.draw_settings(ui);
        } else {
            match self.mode.clone() {
                Mode::Idle => self.draw_hidden(ui),
                Mode::Icon { pos, target, clip_seq, .. } => self.draw_icon(ui, pos, target, clip_seq),
                Mode::Panel => self.draw_panel(ui),
                Mode::Overlay => self.draw_overlay(ui),
                Mode::Selecting { start } => self.draw_region_selector(ui, start),
            }
        }
        self.count_frame(true);
        // 有东西在屏幕上时 30 fps，空闲时 10 fps——事件泵跟着这个节奏跑，
        // 再慢会让热键与托盘点击有明显延迟
        ctx.request_repaint_after(Duration::from_millis(if self.settings_open || self.mode != Mode::Idle { 33 } else { 100 }));
    }
}

/// 截屏覆盖的运行时状态
struct OverlayState {
    /// 框选区（虚拟屏物理坐标）
    rect: (i32, i32, u32, u32),
    /// 原图纹理。**必须留着句柄**：句柄一 drop，纹理就被回收，画出来是空白
    tex: egui::TextureHandle,
    lines: Vec<crate::overlay::Line>,
    /// 每行盖字用的底色（从原图上采的）
    bg: Vec<Color32>,
    /// 正在显示译文还是原文。点一下切换（有道那种）
    show_dst: bool,
    /// 还在翻的时候给个提示条
    working: bool,
}

impl App {
    /// 摆放主窗口（逻辑坐标）。**与上一次相同就不发**，理由见 `last_place`。
    ///
    /// 光比较目标还不够：命令偶尔会丢（设置窗那边踩过），所以顺带把窗口报回来的实际位置
    /// 与目标比一比，偏了就再发一次——既不空转，又能自己纠偏。
    /// 每秒记一行帧数（ui / logic 各一个计数）。空闲时应当是个位数，
    /// 数字异常大就说明重绘在自激——2026-09-13 排「没操作也占 CPU」时加的。
    fn count_frame(&mut self, drew: bool) {
        if drew {
            self.fps.1 += 1;
        } else {
            self.fps.2 += 1;
        }
        if self.fps.0.elapsed() >= Duration::from_secs(1) {
            tracing::debug!("每秒帧数：绘制 {} 次，只跑逻辑 {} 次", self.fps.1, self.fps.2);
            self.fps = (Instant::now(), 0, 0);
        }
    }

    fn place_window(&mut self, ctx: &egui::Context, pos: egui::Pos2, size: egui::Vec2) {
        let same_target = self.last_place == Some((pos, size));
        let landed = ctx
            .input(|i| i.viewport().outer_rect.map(|r| r.min))
            .is_none_or(|actual| (actual.x - pos.x).abs() <= 2.0 && (actual.y - pos.y).abs() <= 2.0);
        if !same_target {
            self.place_tries = 0;
        }
        // 到位了、或者同一个目标已经试够次数，就不再发——**不封顶就会每帧重发**
        if same_target && (landed || self.place_tries >= MAX_PLACE_RETRIES) {
            return;
        }
        self.place_tries = self.place_tries.saturating_add(1);
        if !landed && self.place_tries >= MAX_PLACE_RETRIES {
            tracing::warn!(
                "窗口摆不到目标{pos:?}，试了 {} 次就不试了，窗口实际在 {:?}",
                self.place_tries,
                ctx.input(|i| i.viewport().outer_rect)
            );
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        self.last_place = Some((pos, size));
    }

    /// 收到原图：建纹理、采每行底色，切到覆盖模式。
    ///
    /// 底色在这里一次采好、存进 `OverlayState`：每帧重采要读几百个像素，
    /// 30 fps 下白烧 CPU（「空闲占满一个核」那一课）。
    fn begin_overlay(&mut self, shot: std::sync::Arc<text_capture::RegionShot>, ctx: &egui::Context) {
        let (w, h) = (shot.image.width() as usize, shot.image.height() as usize);
        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], shot.image.as_raw());
        let tex = ctx.load_texture("region-shot", img, egui::TextureOptions::LINEAR);
        let px = |x: i32, y: i32| -> Option<[u8; 4]> {
            if x < 0 || y < 0 || x >= shot.image.width() as i32 || y >= shot.image.height() as i32 {
                return None;
            }
            Some(shot.image.get_pixel(x as u32, y as u32).0)
        };
        let lines: Vec<crate::overlay::Line> = shot
            .lines
            .iter()
            .map(|l| crate::overlay::Line { rect: l.rect, src: l.text.clone(), dst: None })
            .collect();
        let bg = lines
            .iter()
            .map(|l| {
                let c = crate::overlay::background_of(&px, l.rect, 3);
                Color32::from_rgb(c[0], c[1], c[2])
            })
            .collect();
        let rect = self.overlay_rect.unwrap_or((0, 0, w as u32, h as u32));
        self.overlay = Some(OverlayState { rect, tex, lines, bg, show_dst: true, working: true });
        self.panel = None;
        self.pending_panel = None;
        self.mode = Mode::Overlay;
    }

    /// 覆盖式截屏：把原图铺回框选的位置，再把译文盖到每行原文上。
    ///
    /// 点一下在译文 / 原文之间切换（有道那种）；Esc 或右上角 ✕ 关掉。
    fn draw_overlay(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        let dpi = Dpi::from_ctx(&ctx);
        let Some(region) = self.overlay.as_ref().map(|o| o.rect) else {
            self.mode = Mode::Idle;
            return;
        };
        // 提示栏放在截图区**之外**：放里面会压住原文（2026-09-14 Jack 截图指出）
        let bar_phys = dpi.to_physical_len(OVERLAY_BAR_H).max(1);
        let screen = text_capture::screen::virtual_screen();
        let (win, img_y, bar_y) = crate::overlay::layout(region, bar_phys, screen);
        self.place_window(&ctx, dpi.to_logical_pos((win.0, win.1)), dpi.to_logical_size((win.2, win.3)));
        let img_top = dpi.to_logical_len(img_y as f32);
        let bar_top = dpi.to_logical_len(bar_y as f32);
        let img_h = dpi.to_logical_len(region.3 as f32);

        let mut close = false;
        let mut toggle = false;
        // 拖动位移先记在这里、闭包外再落到 overlay 上：闭包已经借走了 `o`（不可变），
        // 在闭包里再 `as_mut` 就是同一字段上的第二次借用，编译不过
        let mut drag_by = (0i32, 0i32);
        let o = self.overlay.as_ref().expect("上面刚判过");
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root_ui, |ui| {
            let win_rect = ui.max_rect();
            // 截图铺在它自己那一块；窗口比它高出一条，那条留给提示栏
            let full = egui::Rect::from_min_size(
                win_rect.min + egui::vec2(0.0, img_top),
                egui::vec2(win_rect.width(), img_h),
            );
            let painter = ui.painter();
            painter.image(o.tex.id(), full, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);

            if o.show_dst {
                // 排版算在 overlay::plan 里（纯函数、有测试盯着），这里只负责照着画
                let bg: Vec<[u8; 4]> = o.bg.iter().map(|c| [c.r(), c.g(), c.b(), 255]).collect();
                let img = (o.tex.size()[0] as i32, o.tex.size()[1] as i32);
                let ops = crate::overlay::plan(&o.lines, &bg, img, (full.width(), full.height()), |t, size| {
                    ctx.fonts_mut(|f| {
                        f.layout_no_wrap(t.to_string(), egui::FontId::proportional(size), Color32::WHITE).size().x
                    })
                });
                for op in ops {
                    let r = egui::Rect::from_min_size(
                        full.min + egui::vec2(op.rect.0, op.rect.1),
                        egui::vec2(op.rect.2, op.rect.3),
                    );
                    painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(op.bg[0], op.bg[1], op.bg[2]));
                    let fg = if op.dark_text { Color32::from_gray(20) } else { Color32::from_gray(245) };
                    painter.text(
                        egui::pos2(r.left() + 1.0, r.center().y),
                        Align2::LEFT_CENTER,
                        &op.text,
                        egui::FontId::proportional(op.font_size),
                        fg,
                    );
                }
            }
            // 提示栏：独占窗口下（或上）的一条，绝不压住截图内容
            let bar = egui::Rect::from_min_size(
                win_rect.min + egui::vec2(0.0, bar_top),
                egui::vec2(win_rect.width(), OVERLAY_BAR_H),
            );
            painter.rect_filled(bar, CornerRadius::same(4), Color32::from_rgb(250, 250, 248));
            painter.rect_stroke(
                bar,
                CornerRadius::same(4),
                egui::Stroke::new(1.0, Color32::from_gray(185)),
                egui::StrokeKind::Inside,
            );
            let hint = if o.working {
                "翻译中…"
            } else if o.show_dst {
                "译文 · 点击看原文"
            } else {
                "原文 · 点击看译文"
            };
            painter.text(
                egui::pos2(bar.left() + 8.0, bar.center().y),
                Align2::LEFT_CENTER,
                hint,
                egui::FontId::proportional(12.0),
                Color32::from_gray(60),
            );
            painter.text(
                egui::pos2(bar.right() - 10.0, bar.center().y),
                Align2::RIGHT_CENTER,
                "✕ Esc",
                egui::FontId::proportional(12.0),
                Color32::from_gray(90),
            );

            // 提示栏当手柄拖动整个覆盖窗：截屏译文也是浮窗，钉死在原地会挡着用户要看的东西
            // （Jack 2026-09-24「所有悬浮窗口需要可以拖动」）。与点击不冲突：egui 的 drag
            // 要指针移动过阈值才成立，原地点一下仍走下面的关闭/切换。
            let bar_drag = ui.interact(bar, ui.id().with("overlay-drag"), egui::Sense::drag());
            if bar_drag.hovered() || bar_drag.dragged() {
                ui.ctx().set_cursor_icon(if bar_drag.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            if bar_drag.dragged() {
                let d = bar_drag.drag_delta();
                drag_by = (dpi.to_physical_len(d.x), dpi.to_physical_len(d.y));
            }

            let resp = ui.allocate_response(win_rect.size(), egui::Sense::click());
            if resp.clicked() {
                // 点在右上角那条上就是关闭，点别处切换原文/译文
                let at = ui.input(|i| i.pointer.latest_pos());
                if at.is_some_and(|p| bar.contains(p)) {
                    close = true;
                } else {
                    toggle = true;
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });

        if drag_by != (0, 0) {
            if let Some(o) = self.overlay.as_mut() {
                o.rect.0 += drag_by.0;
                o.rect.1 += drag_by.1;
            }
        }
        if toggle {
            if let Some(o) = self.overlay.as_mut() {
                o.show_dst = !o.show_dst;
            }
        }
        if close {
            self.overlay = None;
            self.overlay_rect = None;
            self.mode = Mode::Idle;
        }
    }

    /// 浮窗不计时自收，等用户点到浮窗外面才关（Jack 2026-09-24 定）。
    ///
    /// 用左键**最高位的按下沿**认点击：浮窗开着时界面 60 fps 轮询，人手点击（约 60 ms 以上）
    /// 不会漏；开窗口的那一下按下沿在它之前就被消费了，不会误关刚弹的窗。
    /// （不用最低位「自上次查询以来按下过」：它只对产生输入的线程可靠，实测本线程读不到。）
    /// 「点没点在浮窗上」不用矩形算，问系统**点击处是哪个窗口**（`WindowFromPoint`）：
    /// 是自己进程的浮窗就不关。矩形判据会和卡片高度的逐帧变化赛跑——卡片从离线短条
    /// 长成完整词条的那几帧里，点在「看得到的卡片上」却落在旧矩形外，误关（Jack 2026-09-24 实测）。
    fn poll_outside_click(&mut self) {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, WindowFromPoint};
        let down = unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } & 0x8000u16 as i16 != 0;
        let edge = down && !self.lbtn_was_down;
        self.lbtn_was_down = down;
        if !edge {
            return;
        }
        if !matches!(self.mode, Mode::Panel | Mode::Overlay) {
            return;
        }
        let (cx, cy) = text_capture::screen::cursor_pos();
        let mut pid = 0u32;
        unsafe {
            let hwnd = WindowFromPoint(POINT { x: cx, y: cy });
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid == std::process::id() {
            return; // 点在自己的浮窗上
        }
        tracing::info!("点到浮窗外面 物理({cx},{cy})（窗口属进程 {pid}），收起浮窗");
        if matches!(self.mode, Mode::Overlay) {
            self.overlay = None;
            self.overlay_rect = None;
            self.mode = Mode::Idle;
        } else {
            self.close_panel();
        }
    }

    /// 空闲时把主窗口缩到看不见的一格（egui 的根视口不能真的隐藏）
    fn draw_hidden(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.place_window(&ctx, HIDDEN_POS, HIDDEN_SIZE);
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |_| {});
    }

    fn draw_icon(&mut self, ui: &mut egui::Ui, pos: (i32, i32), target: Option<isize>, clip_seq: u32) {
        let ctx = ui.ctx().clone();
        let dpi = Dpi::from_ctx(&ctx);
        // 摆放算在物理坐标系里（锚点来自 GetCursorPos），图标边长是逻辑像素，先换成物理
        let screen = text_capture::screen::virtual_screen();
        const ICON: f32 = 30.0;
        let (x, y) = place_icon(pos, dpi.to_physical_len(ICON).max(1) as u32, screen, dpi.to_physical_len(4.0));
        let logical = dpi.to_logical_pos((x, y));
        if self.logged_icon_at != Some((x, y)) {
            self.logged_icon_at = Some((x, y));
            tracing::debug!("小图标：锚点物理({},{}) → 摆到物理({x},{y}) → 逻辑{logical:?} ppp={}", pos.0, pos.1, dpi.ppp());
        }
        self.place_window(&ctx, logical, egui::vec2(ICON, ICON));
        let mut clicked = false;
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let r = ui.allocate_response(egui::vec2(ICON, ICON), egui::Sense::click());
            // 点了没反应时分清是 egui 没收到按键，还是收到了但没判成点击
            ui.input(|i| {
                if i.pointer.any_pressed() || i.pointer.any_released() {
                    tracing::debug!(
                        "小图标收到鼠标按键：按下={} 松开={} 位置={:?} 图标区={:?}",
                        i.pointer.any_pressed(),
                        i.pointer.any_released(),
                        i.pointer.interact_pos(),
                        r.rect
                    );
                }
            });
            let painter = ui.painter();
            let bg = if r.hovered() { Color32::from_rgb(60, 120, 220) } else { Color32::from_rgb(40, 90, 180) };
            painter.rect_filled(r.rect, CornerRadius::same(8), bg);
            painter.text(r.rect.center(), Align2::CENTER_CENTER, "译", egui::FontId::proportional(16.0), Color32::WHITE);
            if r.clicked() {
                clicked = true;
            }
        });
        if clicked {
            // 关键证据：点图标这一下有没有把前台抢成简译自己（None = 前台就是简译）
            tracing::info!(
                "点了划词图标：此刻前台外部窗口 {:?}，弹图标时记下的目标 {target:?}",
                text_capture::clipboard::foreign_foreground_window()
            );
            self.start(Job::Selection { target, clip_seq: Some(clip_seq) }, "划词翻译", pos, Source::Clipboard);
        }
    }

    fn draw_panel(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        let Some(p) = self.panel.clone() else {
            self.mode = Mode::Idle;
            return;
        };
        let dpi = Dpi::from_ctx(&ctx);
        let screen = text_capture::screen::virtual_screen();
        // w/h 是逻辑像素（配置里按点写的），但 p.anchor 与 screen 是物理的，
        // 所以摆放统一在物理坐标系里算，算完再换成逻辑交给窗口
        let mut w = self.cfg.ui.panel_width;
        let h = self.cfg.ui.panel_max_height;
        // 摆放高度用**实际内容高**（上一帧量到的 view_h），不用最大高度：右上优先时
        // 贴着指针上方的是卡片底边，按最大高度的预留框摆，短词条的底边会离指针老远、
        // 像悬在半空。高度随内容逐帧变、底边始终钉在指针上方，不会跳。
        let inner_h = if p.view_h > 0.0 { p.view_h.clamp(72.0, h) } else { h };
        let size_phys = (dpi.to_physical_size((w, h)).0, dpi.to_physical_len(inner_h).max(1) as u32);
        let (mut x, mut y) = if p.pinned { p.pos } else { place(p.anchor, size_phys, screen, dpi.to_physical_len(14.0)) };
        // 截屏翻译：卡片与框选区同宽、左边缘对齐，看起来才是「这块区域的译文」。
        // 钉住或拖过之后就不再跟框走，用户挪到哪儿就在哪儿。
        // 钉住只是「不自动关闭」，不该改变几何；只有用户亲手拖过才停止跟着框走
        if let (Some(region), false) = (p.source_rect, p.user_moved) {
            let max_w = dpi.to_physical_len(self.cfg.ui.panel_width * 2.5).max(1) as u32;
            let ((nx, ny), wp) = crate::panel::align_to_region(
                region,
                (x, y),
                dpi.to_physical_len(MIN_REGION_PANEL_W).max(1) as u32,
                max_w.min(screen.2),
                screen,
            );
            x = nx;
            y = ny;
            w = dpi.to_logical_len(wp as f32);
        }
        let logical = dpi.to_logical_pos((x, y));
        if (x, y) != p.pos {
            tracing::debug!("面板：锚点物理{:?} → 摆到物理({x},{y}) → 逻辑{logical:?} 尺寸逻辑({w}x{inner_h}) ppp={}", p.anchor, dpi.ppp());
        }
        self.place_window(&ctx, logical, egui::vec2(w, inner_h));
        if self.logged_card_w != Some(w) {
            self.logged_card_w = Some(w);
            tracing::info!(
                "卡片宽度：目标逻辑 {w:.0}，窗口实际 {:?}，egui 画布 {:?}",
                ctx.input(|i| i.viewport().inner_rect.map(|r| r.size())),
                ctx.content_rect().size()
            );
        }

        let mut close = false;
        let mut pin_toggled = false;
        let mut retry = false;
        let mut copy = false;
        let (mut drag_started, mut dragging, mut drag_stopped) = (false, false, false);
        let mut content_h = 0.0;
        let mut content_w = 0.0;
        // 取词场景才认词典格式：普通译文里偶尔也有 # 开头的行（Markdown 标题、井号话题），
        // 不限定场景的话一句正常译文会被切成一张残缺的词典卡
        let entry = (scene_of(p.scene_label) == Scene::Word).then(|| crate::dict::parse(&p.translated)).flatten();
        let base = self.cfg.ui.font_size;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(252, 252, 250, 248))
                    .stroke(egui::Stroke::new(1.0, Color32::from_gray(175)))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(12.0),
            )
            .show(root_ui, |ui| {
                let head = ui.horizontal(|ui| {
                    ui.label(RichText::new(p.scene_label).color(Color32::from_rgb(40, 90, 180)).size(13.0));
                    ui.label(RichText::new(format!("· {}", p.source.label_zh())).color(Color32::from_gray(120)).size(12.0));
                    if p.offline {
                        ui.label(RichText::new("· 离线").color(Color32::from_rgb(150, 110, 20)).size(12.0))
                            .on_hover_text("本地词库的释义，模型的完整词条回来后会替换");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").on_hover_text("关闭").clicked() {
                            close = true;
                        }
                        let pin_label = if p.pinned { "📌" } else { "📍" };
                        if ui.small_button(pin_label).on_hover_text("钉住不自动关闭").clicked() {
                            pin_toggled = true;
                        }
                        let copy_hint = if entry.is_some() { "复制释义" } else { "复制译文" };
                        if ui.small_button("⎘").on_hover_text(copy_hint).clicked() {
                            copy = true;
                        }
                    });
                });
                // 标题栏当拖动手柄。右边留出三个按钮的宽度不参与拖动：
                // 手柄压在按钮上会把点击一起吃掉，表现是「关不掉也钉不住」。
                let mut handle = head.response.rect;
                handle.max.x = (handle.max.x - 112.0).max(handle.min.x);
                let dr = ui.interact(handle, ui.id().with("panel-drag"), egui::Sense::drag());
                if dr.hovered() || dr.dragged() {
                    ui.ctx().set_cursor_icon(if dr.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
                }
                drag_started = dr.drag_started();
                dragging = dr.dragged();
                drag_stopped = dr.drag_stopped();
                ui.separator();
                // **横向必须铺满卡片**：默认 `auto_shrink` 两个方向都开，横向会把内容区
                // 连同滚动条一起缩到「最宽那一行」的宽度——截屏识别出来的多是短行（最宽约 400），
                // 于是卡片明明很宽，文字和滚动条却挤在左边一条，右边一大片空白
                // （2026-09-13 Jack 截图实证）。纵向仍然跟着内容收缩，短译文才不拖一截空白。
                let out = egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height(h - 90.0)
                    .show(ui, |ui| {
                    if let Some(e) = &entry {
                        // 词典卡：词头本身就是原文，不再另起一行灰色原文，省得重复
                        draw_entry(ui, e, &p.original, base, true);
                        if p.is_working() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new("查词中…").color(Color32::GRAY).size(12.0));
                            });
                        }
                        return;
                    }
                    if !p.original.is_empty() {
                        ui.label(RichText::new(&p.original).color(Color32::from_gray(100)).size(base - 1.0));
                        ui.add_space(6.0);
                    }
                    match &p.phase {
                        Phase::Working(msg) => {
                            if p.translated.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new(msg).color(Color32::GRAY).size(12.0));
                                });
                            } else {
                                ui.label(RichText::new(&p.translated).color(Color32::from_gray(60)).size(base));
                            }
                        }
                        Phase::Done => {
                            ui.label(RichText::new(&p.translated).color(Color32::from_gray(20)).size(base));
                        }
                        Phase::Notice(msg) => {
                            ui.label(RichText::new(msg).color(Color32::from_gray(90)).size(13.0));
                        }
                        Phase::Failed(e) => {
                            ui.label(RichText::new(e).color(Color32::from_rgb(185, 45, 45)).size(13.0));
                            if ui.button("重试").clicked() {
                                retry = true;
                            }
                        }
                    }
                });
                // 量内容必须用 ScrollArea 的 content_size，**不能用 ui.min_rect()**：
                // 后者受当前窗口高度限制，而窗口高度又是上一帧量出来的——一旦变矮就自锁，
                // 卡片会永远停在第一帧那点高度，内容被拦腰切掉。
                content_h = head.response.rect.height() + 18.0 + out.content_size.y;
                content_w = out.content_size.x;
            });

        if let Some(p) = self.panel.as_mut() {
            p.pos = (x, y);
            // 12 的内边距上下各一份
            p.view_h = content_h + 24.0;
            if self.logged_content_w.map_or(true, |v: f32| (v - content_w).abs() > 1.0) {
                self.logged_content_w = Some(content_w);
                tracing::info!("卡片内容实际排到 {content_w:.0} 宽（卡片可用宽度约 {:.0}）", w - 24.0);
            }
            if drag_started {
                // 拖动即钉住：特意挪过位置的卡片再自己消失最恼人
                p.drag_from = Some((text_capture::screen::cursor_pos(), p.pos));
                p.pinned = true;
                p.user_moved = true;
            }
            if dragging {
                if let Some(at) = p.drag_to(text_capture::screen::cursor_pos()) {
                    p.pos = at;
                }
            }
            if drag_stopped {
                p.drag_from = None;
            }
            if pin_toggled {
                p.pinned = !p.pinned;
            }
        }
        if copy {
            // 词典卡复制的是整理过的条目，不是带 #标记 的原始回包
            let text = entry.as_ref().map(|e| e.to_plain()).unwrap_or_else(|| p.translated.clone());
            let _ = text_capture::clipboard::set_text(&text);
        }
        if retry {
            let text = p.original.clone();
            if !text.is_empty() {
                self.cur_seq = self.engine.submit(Job::Text { scene: scene_of(p.scene_label), text });
                if let Some(q) = self.panel.as_mut() {
                    q.phase = Phase::Working("翻译中…".into());
                    q.translated.clear();
                }
            }
        }
        if close {
            self.close_panel();
            return;
        }

    }

    /// 截屏框选：整屏半透明遮罩，拖出一个亮框
    fn draw_region_selector(&mut self, root_ui: &mut egui::Ui, start: Option<(i32, i32)>) {
        let ctx = root_ui.ctx().clone();
        let dpi = Dpi::from_ctx(&ctx);
        // 虚拟屏是物理像素，遮罩窗的位置与大小要换成逻辑；
        // 框出来的矩形留在物理坐标系里，因为 BitBlt 抓的是物理像素
        let (sx, sy, sw, sh) = text_capture::screen::virtual_screen();
        self.place_window(&ctx, dpi.to_logical_pos((sx, sy)), dpi.to_logical_size((sw, sh)));
        if self.focus_selector {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.focus_selector = false;
        }
        // **框跟手全靠这两点**（Jack 2026-09-13 报「框与鼠标有偏差」）：
        // 1. 位置取 egui 的指针事件，不再每帧 `GetCursorPos` 轮询——轮询是按帧采样的，
        //    拖快了框就落在鼠标后面；
        // 2. 换算基准用窗口**实际**的左上角，不假设遮罩窗正好落在虚拟屏原点——
        //    差一点点，整条框就整体偏一点。
        let win = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min)).unwrap_or(dpi.to_logical_pos((sx, sy)));
        let ppp = dpi.ppp();
        // 窗口内逻辑坐标 → 虚拟屏物理坐标
        let to_phys = move |p: egui::Pos2| -> (i32, i32) {
            (((win.x + p.x) * ppp).round() as i32, ((win.y + p.y) * ppp).round() as i32)
        };
        // 反向：虚拟屏物理坐标 → 窗口内逻辑坐标（画框用）
        let to_local = move |(x, y): (i32, i32)| -> egui::Pos2 {
            egui::pos2(x as f32 / ppp - win.x, y as f32 / ppp - win.y)
        };
        let mut finish: Option<(i32, i32, u32, u32)> = None;
        let mut cancel = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(0, 0, 0, 90)))
            .show(root_ui, |ui| {
                // 当前点取自指针事件（不是每帧轮询系统鼠标），框才跟得上手。
                // **起点必须自己记**：egui 的 `press_origin` 在松开那一帧就被清空了，
                // 而我们恰好在那一帧算最终矩形——读它的话既画不出框也出不来卡片
                // （2026-09-13 Jack 实测：框没了、译文卡片也没了）。
                let (cur, press, pressed_now, released_now) = ui.input(|i| {
                    (
                        i.pointer.latest_pos().or(i.pointer.interact_pos()),
                        i.pointer.press_origin(),
                        i.pointer.any_pressed(),
                        i.pointer.any_released(),
                    )
                });
                let painter = ui.painter();
                if let (Some(s), Some(c)) = (start, cur) {
                    let (ax, ay) = s;
                    let (mx, my) = to_phys(c);
                    let r = egui::Rect::from_two_pos(to_local(s), c);
                    painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(255, 255, 255, 20));
                    painter.rect_stroke(r, CornerRadius::ZERO, egui::Stroke::new(1.5, Color32::from_rgb(120, 180, 255)), egui::StrokeKind::Inside);
                    painter.text(
                        r.left_top() + egui::vec2(0.0, -18.0),
                        Align2::LEFT_TOP,
                        // 报物理像素：这才是交给识别的那块图的真实尺寸
                        format!("{}×{}", (ax - mx).abs(), (ay - my).abs()),
                        egui::FontId::proportional(13.0),
                        Color32::WHITE,
                    );
                } else {
                    painter.text(
                        egui::pos2(dpi.to_logical_len(sw as f32) / 2.0, 40.0),
                        Align2::CENTER_CENTER,
                        "拖动框选要翻译的区域，Esc 取消",
                        egui::FontId::proportional(16.0),
                        Color32::WHITE,
                    );
                }
                let _resp = ui.allocate_response(ui.available_size(), egui::Sense::click_and_drag());
                // **按下那一帧就记起点**，不等 egui 判定「开始拖动」：那要先走完拖拽阈值，
                // 期间鼠标已经移开一截，框的起点就落在按下点后面
                // （2026-09-13 实测差了 90 物理像素，Jack 报的「框与鼠标有偏差」就是这个）
                if pressed_now && start.is_none() {
                    let from = press.or(cur).map(to_phys);
                    tracing::info!(
                        "框选开始于物理{from:?}（窗口原点逻辑{win:?} ppp={ppp} 指针窗口内{cur:?} 系统鼠标{:?}）",
                        text_capture::screen::cursor_pos()
                    );
                    self.mode = Mode::Selecting { start: from };
                }
                if released_now {
                    if let (Some((ax, ay)), Some(c)) = (start, cur) {
                        let (mx, my) = to_phys(c);
                        tracing::info!(
                            "框选结束于物理({mx},{my})（指针窗口内{c:?} 系统鼠标{:?}）",
                            text_capture::screen::cursor_pos()
                        );
                        let (x0, y0) = (ax.min(mx), ay.min(my));
                        let (w, h) = ((ax - mx).unsigned_abs(), (ay - my).unsigned_abs());
                        if w >= 12 && h >= 12 {
                            finish = Some((x0, y0, w, h));
                        } else {
                            tracing::info!("框选区域太小（{w}x{h}），取消");
                            cancel = true;
                        }
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.mode = Mode::Idle;
        }
        if let Some((x, y, w, h)) = finish {
            // 先把遮罩撤走再抓图：这一刻整屏还盖着半透明黑罩，立刻 BitBlt 拍到的是被压暗的画面，
            // 识别率会掉、甚至一个字都认不出来。改成排队，等窗口挪走几帧再交给引擎。
            tracing::debug!("框选完成，物理矩形({x},{y}) {w}x{h}，等遮罩撤走后再抓图");
            self.pending_region = Some(((x, y, w, h), Instant::now()));
            self.mode = Mode::Idle;
        }
    }

    /// 设置界面画在**主视口**里，不用子视口。
    ///
    /// 子视口在这台 200% 缩放的机器上拿不到一致的坐标系：`show_viewport_immediate` 给出的
    /// 可用区是 920x760「逻辑」，却按 `pixels_per_point = 2` 渲染，而窗口是按 1:1 建的，
    /// 于是只有左上角四分之一可见，底部按钮栏永远排在窗口外面。主视口的换算是对的
    /// （`dpi.rs` 那一套），所以把主窗口临时变成设置窗，关掉再缩回屏幕外当浮窗宿主。
    /// 代价是开着设置时不响应划词与悬停——改设置时本来也不会同时划词。
    fn draw_settings(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        let (win_w, win_h) = window_size(self.settings_tab);

        if self.settings_place_frames > 0 {
            self.settings_place_frames -= 1;
            let d = crate::dpi::Dpi::from_ctx(&ctx);
            let (sx, sy, sw, sh) = text_capture::screen::virtual_screen();
            // 摆在左上角、离两边各留一段（Jack 2026-09-09 定的位置：屏幕居中太靠下）
            const MARGIN_LEFT: i32 = 160;
            const MARGIN_TOP: i32 = 120;
            let at = (
                (sx + MARGIN_LEFT).min(sx + sw as i32 - win_w as i32),
                (sy + MARGIN_TOP).min(sy + sh as i32 - win_h as i32),
            );
            ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
            ctx.send_viewport_cmd(ViewportCommand::Title(window_title(self.settings_tab)));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(win_w, win_h)));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(d.to_logical_pos(at)));
            self.settings_sized_as = Some(self.settings_tab == SettingsTab::Translate);
            self.last_place = None; // 设置窗自己管摆位，别让缓存挡住后续的摆放
            tracing::debug!(
                "设置窗摆位：剩 {} 帧 ppp={} 目标物理{:?} → 逻辑{:?}",
                self.settings_place_frames,
                d.ppp(),
                at,
                d.to_logical_pos(at)
            );
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.settings_want_pos = Some(d.to_logical_pos(at));
        } else if let Some(want) = self.settings_want_pos {
            // **闭环校验**：连发几帧仍会偶发摆歪（实测出现过 (-280,-16)，目标是 (80,60)）。
            // 窗口从「屏幕外的 1x1 浮窗宿主」切成带边框的设置窗时，`Decorations` / `InnerSize` /
            // `OuterPosition` 三条命令的落地顺序不保证，位置可能基于中间态算出来。
            // 盲目多摆几帧治不了偶发，所以读回实际位置比对——不对就再摆一轮。
            let actual = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min));
            if let Some(actual) = actual {
                let off = (actual.x - want.x).abs().max((actual.y - want.y).abs());
                if off > 4.0 && self.settings_place_retries < MAX_PLACE_RETRIES {
                    self.settings_place_retries += 1;
                    self.settings_place_frames = PLACE_FRAMES;
                    tracing::warn!("设置窗摆歪了（实际{actual:?} 期望{want:?}），第 {} 次重摆", self.settings_place_retries);
                } else {
                    // 摆对了（或重试用尽）就别再管，否则用户拖动窗口会被拽回来
                    self.settings_want_pos = None;
                }
            }
        }

        // 翻译页与设置页尺寸不同：换到另一类页时只改大小，不挪位置（用户可能拖过窗口）
        let kind = self.settings_tab == SettingsTab::Translate;
        if self.settings_place_frames == 0 && self.settings_sized_as.is_some_and(|k| k != kind) {
            self.settings_sized_as = Some(kind);
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(win_w, win_h)));
        }

        if self.settings_raise {
            self.settings_raise = false;
            ctx.send_viewport_cmd(ViewportCommand::Title(window_title(self.settings_tab)));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }

        let mut close = false;

        // 存完又改了：「已保存并生效」这句话就过时了，留着会和亮起的保存按钮自相矛盾
        if self.settings_dirty() && self.settings_msg.starts_with("已保存") {
            self.settings_msg.clear();
        }

        let mut do_apply = false;
        let mut do_test = false;
        // 翻译页是独立的主窗口：不要左侧设置导航，整个窗口给输入框和结果
        if self.settings_tab == SettingsTab::Translate {
            close |= self.draw_translate_window(root_ui);
        } else {
            egui::Panel::left("settings-nav").exact_size(104.0).show(root_ui, |ui| {
                ui.add_space(8.0);
                for tab in SettingsTab::ALL {
                    let selected = self.settings_tab == tab;
                    if ui
                        .add_sized([92.0, 26.0], egui::Button::selectable(selected, RichText::new(tab.label()).size(12.0)))
                        .clicked()
                    {
                        self.settings_tab = tab;
                        self.settings_scroll_top = true; // 换页要回到顶部，否则顶着上一页的滚动位置
                        ui.ctx().send_viewport_cmd(ViewportCommand::Title(window_title(tab)));
                        if tab == SettingsTab::Translate {
                            self.manual.want_focus = true;
                        }
                    }
                    ui.add_space(2.0);
                }

                // 操作按钮放这儿而不是窗口底部：Panel::bottom 与 bottom_up 布局在这个
                // 窗口里都排不出来（矩形落在可见区外 / 被 ScrollArea 挤掉），导航栏则稳定可见
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);
                // 有没保存的改动时按钮填成主色，一眼看出「改了还没存」；存完（draft 与 cfg 相等）自动变回来
                let dirty = self.settings_dirty();
                do_apply = ui
                    .add_sized([92.0, 26.0], save_button(dirty))
                    .on_hover_text(if dirty { "有改动还没保存" } else { "没有改动" })
                    .clicked();
                ui.add_space(3.0);
                do_test = ui.add_sized([92.0, 24.0], egui::Button::new(RichText::new("测试翻译").size(12.0))).clicked();
                ui.add_space(3.0);
                if ui.add_sized([92.0, 24.0], egui::Button::new(RichText::new("关闭").size(12.0))).clicked() {
                    close = true;
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("运行状态").size(10.0).color(Color32::from_gray(110)));
                ui.add_space(2.0);
                for line in self.status.split(" · ") {
                    ui.label(RichText::new(line).size(10.0).color(Color32::from_gray(140)));
                }
            });

            egui::CentralPanel::default().show(root_ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(self.settings_tab.label()).size(16.0).strong());
                ui.label(RichText::new(self.settings_tab.hint()).size(10.0).color(Color32::from_gray(140)));
                if !self.settings_msg.is_empty() {
                    let msg = self.settings_msg.clone();
                    let bad = msg.contains("失败") || msg.contains("有误");
                    let color = if bad { Color32::from_rgb(200, 90, 90) } else { Color32::from_rgb(80, 155, 95) };
                    let short: String = msg.chars().take(30).collect();
                    let shown = if short.chars().count() < msg.chars().count() { format!("{short}…") } else { short };
                    ui.add_space(4.0);
                    ui.label(RichText::new(shown).size(11.0).color(color)).on_hover_text(&msg);
                }
                ui.add_space(8.0);
                let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if self.settings_scroll_top {
                    self.settings_scroll_top = false;
                    area = area.vertical_scroll_offset(0.0);
                }
                area.show(ui, |ui| match self.settings_tab {
                    // 翻译页走 draw_translate_window，不会到这里
                    SettingsTab::Translate => {}
                    SettingsTab::Service => self.settings_service(ui),
                    SettingsTab::Trigger => self.settings_trigger(ui),
                    SettingsTab::Glossary => self.settings_glossary(ui),
                    SettingsTab::Server => self.settings_server(ui),
                    SettingsTab::Env => self.settings_env(ui),
                    SettingsTab::About => self.settings_about(ui),
                });
            });
        }

        if do_apply {
            self.apply_settings();
        }
        if do_test {
            let cfg = self.draft.translator_config();
            self.settings_msg = match ai_translate::Translator::new(cfg) {
                Ok(t) => match t.translate(Scene::Selection, "Hello, this is a test.", None, &mut |_| {}) {
                    Ok(zh) => format!("测试通过：{zh}"),
                    Err(e) => format!("测试失败：{e}"),
                },
                Err(e) => format!("配置有误：{e}"),
            };
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            tracing::debug!("设置窗收到关闭请求");
            // **必须撤销这次关闭**：设置窗借的是 ROOT 视口，放任它关掉就是整个进程退出。
            // 用户点标题栏的 ✕ 只是想关设置，不是想退出常驻托盘的应用。
            // 真要退出走托盘菜单的「退出」（`request_quit`）。
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            close = true;
        }
        if close {
            self.settings_open = false;
            self.settings_sized_as = None;
            self.settings_place_frames = 0;
            self.settings_want_pos = None;
            // 变回那个躲在屏幕外的浮窗宿主。**四条命令必须在这一帧全发完**，
            // 不能推到后面的帧：窗口一旦缩到屏幕外，eframe 会判定它 occluded 而
            // 只跑 `App::logic` 不跑 `App::ui`（见 epi_integration 的 update_logic_only），
            // 于是任何「关闭之后的帧」里的逻辑都停摆，标题与边框永远改不回来。
            // 这一帧设置窗还看得见，ui() 一定在跑，是最后的机会。
            ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(ViewportCommand::Title(APP_NAME_ZH.to_string()));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(HIDDEN_SIZE));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(HIDDEN_POS));
            self.last_place = Some((HIDDEN_POS, HIDDEN_SIZE));
            self.place_tries = 0;
            tracing::debug!("设置窗已关闭，已发出恢复浮窗宿主的命令");
        }
    }

    /// 一行设置：左标签、右控件，下面跟一行灰色说明。说明为空就不占位。
    fn row(ui: &mut egui::Ui, label: &str, hint: &str, add: impl FnOnce(&mut egui::Ui)) {
        ui.horizontal(|ui| {
            ui.add_sized([LABEL_W, 22.0], egui::Label::new(RichText::new(label).size(13.0)));
            add(ui);
        });
        if !hint.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(LABEL_W + 8.0);
                ui.label(RichText::new(hint).size(11.0).color(Color32::from_gray(135)));
            });
        }
        ui.add_space(7.0);
    }

    fn section(ui: &mut egui::Ui, title: &str, hint: &str) {
        ui.add_space(1.0);
        ui.separator();
        ui.add_space(7.0);
        ui.label(RichText::new(title).size(13.0).strong());
        if !hint.is_empty() {
            ui.label(RichText::new(hint).size(11.0).color(Color32::from_gray(135)));
        }
        ui.add_space(7.0);
    }

    fn settings_service(&mut self, ui: &mut egui::Ui) {
        Self::row(ui, "翻译方向", "决定所有场景翻成哪种语言；托盘菜单里也能随手切", |ui| {
            let cur = ai_translate::Direction::parse(&self.draft.direction);
            egui::ComboBox::from_id_salt("direction").width(104.0).selected_text(cur.label_zh()).show_ui(ui, |ui| {
                for d in [ai_translate::Direction::ToChinese, ai_translate::Direction::ToEnglish] {
                    ui.selectable_value(&mut self.draft.direction, d.as_str().to_string(), d.label_zh());
                }
            });
        });
        Self::row(ui, "服务商", "选一个就自动填好下面的接口地址与模型；手改地址后这里显示「自定义」", |ui| {
            let cur = crate::presets::label_of(&self.draft.backend.openai_base_url);
            egui::ComboBox::from_id_salt("provider").width(104.0).selected_text(cur).show_ui(ui, |ui| {
                for p in crate::presets::PRESETS {
                    // 选中就填字段；Key 不动——每家的 Key 不一样，替用户清空只会更糟
                    if ui.selectable_label(cur == p.name, p.name).clicked() {
                        self.draft.backend.openai_base_url = p.base_url.to_string();
                        self.draft.backend.openai_model = p.model.to_string();
                    }
                }
            });
        });
        Self::row(ui, "接口地址", "DeepSeek 填 https://api.deepseek.com；通义千问填 dashscope 的兼容地址", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.backend.openai_base_url).desired_width(FIELD_W));
        });
        Self::row(ui, "API Key", "只存在本机配置文件里，不进浏览器插件", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.backend.openai_api_key).password(true).desired_width(FIELD_W));
        });
        Self::row(ui, "模型", "", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.backend.openai_model).desired_width(FIELD_W));
        });

        Self::section(
            ui,
            "每个场景用哪个后端",
            "整页翻译固定走 OpenAI 兼容接口：claude 与 codex 是常驻子进程，每路约 300 MB，撑不住几十并发。",
        );
        for (label, fixed) in [("整页翻译", true), ("划词翻译", false), ("悬停翻译", false), ("截屏翻译", false)] {
            ui.horizontal(|ui| {
                ui.add_sized([LABEL_W, 22.0], egui::Label::new(RichText::new(label).size(13.0)));
                let field = match label {
                    "整页翻译" => &mut self.draft.scenes.full_page,
                    "划词翻译" => &mut self.draft.scenes.selection,
                    "悬停翻译" => &mut self.draft.scenes.hover,
                    _ => &mut self.draft.scenes.screenshot,
                };
                ui.add_enabled_ui(!fixed, |ui| {
                    egui::ComboBox::from_id_salt(label).width(104.0).selected_text(field.clone()).show_ui(ui, |ui| {
                        for b in ["openai", "claude", "codex"] {
                            ui.selectable_value(field, b.to_string(), b);
                        }
                    });
                });
                if fixed {
                    ui.label(RichText::new("固定，不可改").size(11.0).color(Color32::from_gray(135)));
                }
            });
            ui.add_space(7.0);
        }

        Self::section(ui, "性能", "");
        Self::row(ui, "并发上限", "同时向模型发几个请求。插件整页翻译是一段一个请求，这个值决定同时翻几段", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.backend.concurrency).range(1..=64));
        });
        Self::row(ui, "每请求段数", "打包翻译时一次发几段，只影响桌面端的批量场景", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.backend.batch_size).range(1..=50));
        });
        Self::row(ui, "超时", "单个请求等多久算超时", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.backend.timeout_secs).range(5..=300).suffix(" 秒"));
        });
    }

    fn settings_trigger(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("功能分两步：先用三键组合把功能打开（打开＝待命），再用顺手的单键或鼠标动作调用。没打开时怎么按都不响应。").size(11.0).color(Color32::from_gray(135)));
        ui.add_space(10.0);

        ui.label(RichText::new("① 开关（三键，按着别扭但不常按）").size(13.0).strong());
        ui.add_space(7.0);
        Self::row(ui, "悬停取词", "按一下开、再按一下关；当前状态弹卡片告诉你", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.hotkeys.toggle_hover).desired_width(120.0));
        });
        Self::row(ui, "划词图标", "同上", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.hotkeys.toggle_selection).desired_width(120.0));
        });

        Self::section(ui, "② 调用（开着的时候用）", "");
        Self::row(ui, "悬停按住", "开着时按住这个键、鼠标停在文字上就翻鼠标那一行", |ui| {
            let cur = match self.draft.hotkeys.hover_modifier.as_str() {
                "alt" => "Alt",
                "shift" => "Shift",
                "win" => "Win",
                _ => "Ctrl",
            };
            egui::ComboBox::from_id_salt("hovermod").width(104.0).selected_text(cur).show_ui(ui, |ui| {
                for (v, label) in [("ctrl", "Ctrl"), ("alt", "Alt"), ("shift", "Shift"), ("win", "Win")] {
                    ui.selectable_value(&mut self.draft.hotkeys.hover_modifier, v.to_string(), label);
                }
            });
        });
        Self::row(ui, "停留多久", "鼠标停住这么久才翻；太短会一划过去就触发", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.hotkeys.hover_delay_ms).range(80..=3000).suffix(" ms"));
        });
        Self::row(ui, "划词", "开着时选中文字，旁边直接弹「译」按钮，不用按键", |ui| {
            ui.label(RichText::new("选中文字即可").size(11.0).color(Color32::from_gray(135)));
        });

        Self::section(ui, "当前状态", "也可以在这里直接勾，效果与按开关热键一样");
        let hover_label = if self.draft.ui.hover_enabled { "开着" } else { "关着" };
        Self::row(ui, "悬停取词", "", |ui| {
            ui.checkbox(&mut self.draft.ui.hover_enabled, hover_label);
        });
        let icon_label = if self.draft.ui.selection_icon { "开着" } else { "关着" };
        Self::row(ui, "划词图标", "", |ui| {
            ui.checkbox(&mut self.draft.ui.selection_icon, icon_label);
        });
        Self::row(ui, "图标停留", "多久不点就自动消失", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.ui.icon_timeout_ms).range(500..=10000).suffix(" ms"));
        });
        let skip_label = if self.draft.ui.hover_skip_target_lang { "不弹卡片" } else { "照样弹" };
        let dict_label = if self.draft.ui.offline_dict { "先给离线释义" } else { "只等模型" };
        Self::row(ui, "离线词典", "取到单个词时先用本地词库秒出一份释义，模型的完整词条回来再替换", |ui| {
            ui.checkbox(&mut self.draft.ui.offline_dict, dict_label);
        });
        Self::row(ui, "遇目标语言", "悬停取到的已经是目标语言时（英译中时扫到中文）要不要出卡片", |ui| {
            ui.checkbox(&mut self.draft.ui.hover_skip_target_lang, skip_label);
        });

        Self::section(ui, "一次性动作", "按下就直接执行，没有待命状态");
        Self::row(ui, "截屏翻译", "进入框选，拖出一块区域后识别并翻译", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.hotkeys.screenshot).desired_width(120.0));
        });
        Self::row(ui, "面板开关", "显示或收起最近一次的译文面板", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.hotkeys.toggle_panel).desired_width(120.0));
        });

        Self::section(ui, "例外程序", "这些程序里不弹图标、不响应悬停。每行一个可执行文件名，留空表示不排除。");
        let mut text = self.draft.ui.excluded_processes.join("
");
        let r = ui.add(
            egui::TextEdit::multiline(&mut text).desired_width(FIELD_W).desired_rows(3).hint_text("code.exe"),
        );
        if r.changed() {
            self.draft.ui.excluded_processes =
                text.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect();
        }
    }

    fn settings_glossary(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("命中的词会把「原文 → 固定译法」一起发给模型，让专有名词不乱翻。不做硬替换，模型仍按语境调整。")
                .size(11.0)
                .color(Color32::from_gray(135)),
        );
        ui.add_space(12.0);
        let mut remove = None;
        for (i, g) in self.draft.glossary.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut g.src).desired_width(112.0).hint_text("原文"));
                ui.label(RichText::new("→").color(Color32::from_gray(140)));
                ui.add(egui::TextEdit::singleline(&mut g.dst).desired_width(112.0).hint_text("固定译法"));
                if ui.button("删除").clicked() {
                    remove = Some(i);
                }
            });
            ui.add_space(7.0);
        }
        if let Some(i) = remove {
            self.draft.glossary.remove(i);
        }
        if self.draft.glossary.is_empty() {
            ui.label(RichText::new("还没有术语。遇到总被翻错的专有名词时加进来。").size(12.0).color(Color32::from_gray(135)));
            ui.add_space(10.0);
        }
        if ui.add(egui::Button::new("＋ 添加一条").min_size(egui::vec2(96.0, 25.0))).clicked() {
            self.draft.glossary.push(Default::default());
        }
    }

    /// 运行环境自检页。
    ///
    /// 检查结果缓存在 `env_items` 里，**不每帧重查**：查一次要读注册表、枚举 PATH、
    /// 列 OCR 语言包，放在 30 fps 的重绘里会明显卡。进页面时查一次，之后靠「重新检查」。
    fn refresh_env(&mut self) {
        let app_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf));
        let langs = self.capturer.ocr_languages();
        self.env_items = Some(desktop_shell::deps::check_all(app_dir.as_deref(), &langs));
    }

    /// 主窗口（翻译页）的外框：顶栏（名字 + 设置入口）+ 翻译区。返回是否要关窗。
    fn draw_translate_window(&mut self, root_ui: &mut egui::Ui) -> bool {
        let mut close = false;
        let mut to_settings = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(root_ui.visuals().panel_fill).inner_margin(egui::Margin::symmetric(18, 14)))
            .show(root_ui, |ui| {
                // 标题栏已经写着「简译」，这里不再重复一个大标题；设置入口放在操作行里
                to_settings = self.settings_translate(ui);
            });
        if to_settings {
            self.settings_tab = SettingsTab::Service;
            self.settings_scroll_top = true;
            root_ui.ctx().send_viewport_cmd(ViewportCommand::Title(window_title(SettingsTab::Service)));
        }
        // Esc 收起窗口（与浮动卡片一致）。输入框里的 Esc 也算：这里没有需要 Esc 撤销的编辑操作
        if root_ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        close
    }

    /// 翻译区：输入框 + 操作行 + 结果卡片 + 最近查过。
    ///
    /// 英文单词**边输入边查本地词库**（停手 0.25 秒），不等回车、不花模型的钱；
    /// 回车才问模型要完整词条或整句译文（Jack 2026-09-22 定）。
    /// 返回用户是否点了「设置」
    fn settings_translate(&mut self, ui: &mut egui::Ui) -> bool {
        use crate::manual::{Live, Phase as MPhase, View};
        let mut to_settings = false;
        let now = Instant::now();
        let cfg_dir = ai_translate::Direction::parse(&self.cfg.direction);
        let base = self.cfg.ui.font_size;
        let id = egui::Id::new("manual-input");

        // 回车翻译、Shift+回车换行。回车要在输入框处理按键**之前**吃掉，否则先换了行再翻。
        // `consume_key` 会忽略多按的 Shift，所以 Shift 要自己判
        let focused = ui.memory(|m| m.has_focus(id));
        let enter = focused
            && ui.input_mut(|i| !i.modifiers.shift && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let resp = ui.add(
            egui::TextEdit::multiline(&mut self.manual.input)
                .id(id)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(10.0, 8.0))
                .hint_text("输入英文单词查词典，或输入一句话翻译")
                .font(egui::FontId::proportional(base + 3.0)),
        );
        if resp.changed() {
            self.manual.edited(now);
        }
        if self.manual.want_focus {
            self.manual.want_focus = false;
            resp.request_focus();
        }

        // 边输入边查：停手够久就查一次本地词库（界面线程直查，0.2 ms 以内）
        if let Some(q) = self.manual.due_live_query(now) {
            let markers = self.engine.lookup_offline(&q);
            tracing::debug!("主窗口边查 {q:?}：{}", if markers.is_some() { "命中" } else { "没有" });
            self.manual.live = Some(Live { query: q, markers });
        }
        if self.manual.edited_at.is_some() {
            ui.ctx().request_repaint_after(crate::manual::LIVE_DELAY);
        }

        let mut submit = enter || std::mem::take(&mut self.manual.autosubmit);
        let mut clear = false;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let t = self.manual.input.trim();
            let dir_text = if t.is_empty() { "自动识别方向".to_string() } else { format!("{} · 自动", cfg_dir.for_input(t).label_zh()) };
            ui.label(RichText::new(dir_text).size(12.0).color(Color32::from_gray(130)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let go = egui::Button::new(RichText::new("翻译").size(13.0).color(Color32::WHITE))
                    .fill(Color32::from_rgb(64, 120, 220))
                    .min_size(egui::vec2(64.0, 26.0));
                submit |= ui.add_enabled(self.manual.can_submit(), go).on_hover_text("回车").clicked();
                clear = ui.add(egui::Button::new(RichText::new("清空").size(13.0)).min_size(egui::vec2(52.0, 26.0))).clicked();
                to_settings = ui
                    .add(egui::Button::new(RichText::new("⚙").size(15.0)).frame(false))
                    .on_hover_text("设置：翻译服务、快捷键、术语表……")
                    .clicked();
            });
        });
        if clear {
            self.manual.clear();
        }
        if submit && self.manual.can_submit() {
            let text = self.manual.input.trim().to_string();
            let seq = self.engine.submit(Job::Manual { text });
            self.manual.begin(seq, cfg_dir);
            tracing::info!("主窗口翻译：{} 字，{}", self.manual.original.chars().count(), self.manual.dir.label_zh());
        }
        ui.add_space(10.0);

        let mut copy: Option<String> = None;
        let copied = self.manual.copied_at.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        let hint = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).size(12.0).color(Color32::from_gray(140)));
        };
        let view = self.manual.view();
        // 结果卡片占住剩下的高度（留一行给「最近查过」），内容长了在卡片里滚
        let card_max_h = (ui.available_height() - 34.0).max(80.0);
        match view {
            View::Empty => {}
            View::Hint => {
                let t = self.manual.input.trim();
                let is_word = ai_translate::scene::looks_like_word_in(t, ai_translate::Direction::ToChinese);
                if is_word && !self.engine.has_offline_dict() {
                    hint(ui, "本机没装离线词典，回车问模型");
                } else if !is_word {
                    hint(ui, "回车翻译，Shift+回车换行");
                }
            }
            View::LiveMiss => hint(ui, &format!("本地词库里没有「{}」，回车问模型", self.manual.input.trim())),
            View::Live(markers) => {
                let entry = crate::dict::parse(&markers);
                result_card(ui, card_max_h, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("本地词典").size(12.0).color(Color32::from_rgb(150, 180, 255)));
                        ui.label(RichText::new("· 离线").size(12.0).color(Color32::from_rgb(190, 160, 90)));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(if copied { "已复制" } else { "复制" }).clicked() {
                                copy = Some(entry.as_ref().map(|e| e.to_plain()).unwrap_or_else(|| markers.clone()));
                            }
                        });
                    });
                    ui.add_space(4.0);
                    match &entry {
                        Some(e) => draw_entry(ui, e, self.manual.input.trim(), base + 1.0, false),
                        None => {
                            ui.label(RichText::new(&markers).size(base).color(Color32::WHITE));
                        }
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("回车查模型的完整词条（例句、词形）").size(11.0).color(Color32::from_gray(140)));
                });
            }
            View::Result => {
                let entry = self.manual.entry();
                result_card(ui, card_max_h, |ui| {
                    ui.horizontal(|ui| {
                        let what = if self.manual.scene == Scene::Word { "查词" } else { "翻译" };
                        ui.label(RichText::new(format!("{} · {what}", self.manual.dir.label_zh())).size(12.0).color(Color32::from_rgb(150, 180, 255)));
                        if self.manual.offline {
                            ui.label(RichText::new("· 离线").size(12.0).color(Color32::from_rgb(190, 160, 90)))
                                .on_hover_text("本地词库的释义，模型的完整词条回来后会替换");
                        }
                        if !self.manual.result.trim().is_empty() {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(if copied { "已复制" } else { "复制" }).clicked() {
                                    // 词典条目复制整理过的文字，不是带 #标记 的原始回包（与浮动卡片一致）
                                    copy = Some(entry.as_ref().map(|e| e.to_plain()).unwrap_or_else(|| self.manual.result.clone()));
                                }
                            });
                        }
                    });
                    ui.add_space(4.0);
                    if let Some(e) = &entry {
                        draw_entry(ui, e, &self.manual.original, base + 1.0, false);
                    } else if !self.manual.result.trim().is_empty() {
                        ui.add(egui::Label::new(RichText::new(&self.manual.result).size(base + 1.0).color(Color32::WHITE)).selectable(true));
                    }
                    ui.add_space(6.0);
                    match &self.manual.phase {
                        MPhase::Working => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                let doing = if self.manual.scene == Scene::Word { "查词中…" } else { "翻译中…" };
                                ui.label(RichText::new(doing).size(12.0).color(Color32::GRAY));
                            });
                        }
                        MPhase::Done { took_ms } => {
                            ui.label(RichText::new(format!("用时 {took_ms} ms")).size(10.0).color(Color32::from_gray(120)));
                        }
                        MPhase::Notice(msg) => {
                            ui.label(RichText::new(msg).size(11.0).color(Color32::from_gray(150)));
                        }
                        MPhase::Failed(msg) => {
                            ui.label(RichText::new(msg).size(12.0).color(Color32::from_rgb(220, 110, 110)));
                        }
                        MPhase::Idle => {}
                    }
                });
            }
        }
        if let Some(text) = copy {
            match text_capture::clipboard::set_text(&text) {
                Ok(()) => self.manual.copied_at = Some(now),
                Err(e) => tracing::warn!("主窗口复制失败：{e}"),
            }
        }

        // 最近查过：点一下重查（单词走本地词库，句子直接翻）
        if !self.manual.recent.is_empty() {
            ui.add_space(8.0);
            let mut pick: Option<String> = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("最近：").size(11.0).color(Color32::from_gray(130)));
                for r in &self.manual.recent {
                    let shown: String = r.chars().take(16).collect();
                    let shown = if shown.chars().count() < r.chars().count() { format!("{shown}…") } else { shown };
                    if ui.add(egui::Button::new(RichText::new(shown).size(11.0)).frame(false)).on_hover_text(r).clicked() {
                        pick = Some(r.clone());
                    }
                }
            });
            if let Some(r) = pick {
                self.manual.input = r;
                self.manual.want_focus = true;
                if ai_translate::scene::looks_like_word_in(self.manual.input.trim(), ai_translate::Direction::ToChinese) {
                    // 单词：立刻查本地，不用等停手
                    self.manual.edited(now - crate::manual::LIVE_DELAY);
                } else {
                    self.manual.edited(now);
                    self.manual.autosubmit = true;
                }
            }
        }
        to_settings
    }

    /// 「关于」页：版本、各组件装在哪、出问题时该提供哪些信息。
    ///
    /// 每一行都可以「打开」到对应的目录/文件，右上角一个「复制全部」——
    /// 排障时最费口舌的就是让人去找日志和配置在哪。
    fn settings_about(&mut self, ui: &mut egui::Ui) {
        // 词条数要开一次词库才知道，开一次要读几 MB 索引，所以算完就存着
        if self.about_dict.is_none() {
            self.about_dict = Some(match crate::offline::dict_path() {
                Some(p) => offline_dict::Dict::open(&p).ok().map(|d| (p, d.len())),
                None => None,
            });
        }
        let facts = crate::about::facts(
            crate::config::Config::dir(),
            self.about_dict.clone().flatten(),
            self.caption.exe_path(),
            crate::extension::dist_dir(),
            self.server.as_ref().map(|s| s.port),
            self.server.as_ref().is_some_and(|s| s.seen.lock().unwrap().at.is_some()),
        );

        ui.horizontal(|ui| {
            ui.label(RichText::new(APP_NAME_ZH).size(18.0).strong());
            ui.label(RichText::new(crate::about::version_line()).size(12.0).color(Color32::from_gray(150)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("复制全部").on_hover_text("排障时把这些信息一起发过去").clicked() {
                    let text = crate::about::to_plain(&facts);
                    if let Err(e) = text_capture::clipboard::set_text(&text) {
                        self.settings_msg = format!("复制失败：{e}");
                    } else {
                        self.settings_msg = "关于信息已复制".into();
                    }
                }
            });
        });
        ui.add_space(10.0);

        for f in &facts {
            ui.horizontal(|ui| {
                ui.add_sized([LABEL_W, 20.0], egui::Label::new(RichText::new(f.name).size(12.0)));
                ui.label(RichText::new(&f.value).size(12.0).color(Color32::from_gray(190)));
                if let Some(path) = &f.path {
                    let path = path.clone();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("打开").clicked() {
                            // 文件就打开它所在的目录：直接 explorer 一个 .bin 只会弹「用什么打开」
                            let target = if path.is_file() { path.parent().map(|d| d.to_path_buf()) } else { Some(path.clone()) };
                            if let Some(t) = target {
                                if let Err(e) = crate::extension::reveal_dir(&t) {
                                    self.settings_msg = format!("打不开：{e}");
                                }
                            }
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(RichText::new("第三方组件").size(13.0).strong());
        ui.add_space(4.0);
        ui.label(
            RichText::new("离线词典数据来自 ECDICT（MIT）；实时字幕的识别模型来自 whisper.cpp（MIT）。")
                .size(11.0)
                .color(Color32::from_gray(150)),
        );
        ui.add_space(4.0);
        if ui.small_button("查看授权说明").clicked() {
            // 授权说明随安装包投在 exe 旁边；开发时退回仓库里的那份
            let mut doc = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("第三方组件授权.md")));
            if !doc.as_ref().is_some_and(|d| d.is_file()) {
                doc = Some(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/第三方组件授权.md"));
            }
            match doc.filter(|d| d.is_file()) {
                Some(d) => {
                    if let Err(e) = crate::extension::reveal_dir(&d) {
                        self.settings_msg = format!("打不开授权说明：{e}");
                    }
                }
                None => self.settings_msg = "找不到授权说明文件".into(),
            }
        }
    }

    fn settings_env(&mut self, ui: &mut egui::Ui) {
        use desktop_shell::deps::{run_fix, Status};

        if self.env_items.is_none() {
            self.refresh_env();
        }
        let items = self.env_items.clone().unwrap_or_default();
        let missing = items.iter().filter(|i| i.status == Status::Missing).count();
        let optional = items.iter().filter(|i| i.status == Status::Optional).count();

        ui.horizontal(|ui| {
            let (color, text) = if missing > 0 {
                (Color32::from_rgb(215, 95, 95), format!("{missing} 项缺失，对应功能用不了"))
            } else if optional > 0 {
                (Color32::from_rgb(200, 160, 70), format!("必需项齐全，{optional} 项可选未装"))
            } else {
                (Color32::from_rgb(80, 165, 95), "全部齐全".to_string())
            };
            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, color);
            ui.label(RichText::new(text).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("重新检查").clicked() {
                    self.env_items = None;
                }
                if ui.small_button("复制报告").clicked() {
                    let _ = text_capture::clipboard::set_text(&desktop_shell::deps::report(&items));
                    self.settings_msg = "报告已复制".into();
                }
            });
        });
        ui.add_space(8.0);

        let mut do_fix = None;
        for it in &items {
            let (dot, name_color) = match it.status {
                Status::Ok => (Color32::from_rgb(80, 165, 95), Color32::from_gray(215)),
                Status::Optional => (Color32::from_rgb(200, 160, 70), Color32::from_gray(215)),
                Status::Missing => (Color32::from_rgb(215, 95, 95), Color32::from_rgb(245, 175, 175)),
            };
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, dot);
                ui.label(RichText::new(it.name).size(12.5).color(name_color).strong());
                ui.label(RichText::new(it.status.label()).size(10.0).color(Color32::from_gray(135)));
            });
            ui.add(egui::Label::new(RichText::new(&it.detail).size(10.5).color(Color32::from_gray(140))).wrap());
            // 正常的项不铺开影响与办法：全展开会把真正要处理的两三项淹掉
            if it.status != Status::Ok {
                ui.add(egui::Label::new(RichText::new(format!("影响：{}", it.impact)).size(10.5).color(Color32::from_gray(165))).wrap());
                ui.add(egui::Label::new(RichText::new(format!("办法：{}", it.how)).size(10.5).color(Color32::from_gray(140))).wrap());
                if let Some(fix) = &it.fix {
                    ui.add_space(3.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new(fix.label()).size(11.0)).min_size(egui::vec2(96.0, 23.0))).clicked() {
                            do_fix = Some(fix.clone());
                        }
                        // 提前说清会弹 UAC / 会联网，别让提权框和网络请求显得突兀
                        if fix.needs_admin() {
                            ui.label(RichText::new("会弹管理员授权").size(10.0).color(Color32::from_gray(150)));
                        }
                    });
                }
            }
            ui.add_space(9.0);
        }

        if let Some(fix) = do_fix {
            self.settings_msg = match run_fix(&fix) {
                Ok(()) => "已启动安装程序，装完点「重新检查」".into(),
                Err(e) => format!("失败：{e}"),
            };
        }
    }

    fn settings_server(&mut self, ui: &mut egui::Ui) {
        // 插件连没连上：服务端记了最近一次收到扩展请求的时刻
        let seen = self.server.as_ref().map(|s| s.seen.lock().unwrap().clone());
        let (dot, text) = match &seen {
            Some(s) if s.at.is_some() => {
                let secs = s.at.unwrap().elapsed().as_secs();
                let when = if secs < 60 {
                    "刚刚".to_string()
                } else if secs < 3600 {
                    format!("{} 分钟前", secs / 60)
                } else {
                    format!("{} 小时前", secs / 3600)
                };
                (Color32::from_rgb(80, 165, 95), format!("插件已连上（最近一次 {when}）"))
            }
            _ => (Color32::from_gray(150), "还没有插件连过来".to_string()),
        };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, dot);
            ui.label(RichText::new(text).size(12.0));
        });
        ui.add_space(10.0);

        // 三步走：Chrome 不让程序静默装未上架的扩展，只能把用户领到该点的地方
        ui.label(RichText::new("装插件（第一次用）").size(13.0).strong());
        ui.add_space(6.0);
        let dir = crate::extension::dist_dir();
        match &dir {
            Some(d) => {
                // 路径大概率比栏宽长，必须 wrap，不然只看得到开头一截
                ui.add(
                    egui::Label::new(RichText::new(format!("插件目录：{}", d.display())).size(10.0).color(Color32::from_gray(135)))
                        .wrap(),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("① 打开扩展页").min_size(egui::vec2(96.0, 25.0))).clicked() {
                        self.settings_msg = match crate::extension::open_extensions_page() {
                            Ok(()) => "已打开 Chrome 扩展页，记得开右上角「开发者模式」".into(),
                            Err(e) => format!("失败：{e}"),
                        };
                    }
                    if ui.add(egui::Button::new("② 打开插件目录").min_size(egui::vec2(96.0, 25.0))).clicked() {
                        self.settings_msg = match crate::extension::reveal_dir(d) {
                            Ok(()) => "已打开插件目录，把它拖到扩展页，或用「加载已解压的扩展程序」选它".into(),
                            Err(e) => format!("失败：{e}"),
                        };
                    }
                });
                ui.add_space(4.0);
                if ui.add(egui::Button::new("复制目录路径").min_size(egui::vec2(96.0, 24.0))).clicked() {
                    self.settings_msg = match crate::extension::copy_path(d) {
                        Ok(()) => "路径已复制，可直接粘进目录选择框".into(),
                        Err(e) => format!("复制失败：{e}"),
                    };
                }
            }
            None => {
                ui.label(
                    RichText::new("没找到插件目录。安装版的插件在程序目录下的 extension 子目录；开发时先在 extension 目录里跑 npm run build。")
                        .size(11.0)
                        .color(Color32::from_rgb(200, 90, 90)),
                );
            }
        }
        ui.add_space(6.0);
        for line in [
            "打开开发者模式 → 点「加载已解压的扩展程序」→ 选上面那个目录",
            "装过之后升级桌面端，只需在扩展页点插件卡片上的刷新按钮",
        ] {
            ui.add(egui::Label::new(RichText::new(line).size(11.0).color(Color32::from_gray(140))).wrap());
            ui.add_space(2.0);
        }

        Self::section(ui, "本地服务", "插件靠它翻译。只监听 127.0.0.1，且只接受浏览器扩展的请求，网页直接调一律拒绝。");
        Self::row(ui, "启用", "关掉后插件无法工作", |ui| {
            ui.checkbox(&mut self.draft.server.enabled, "");
        });
        Self::row(ui, "端口", "改完要重启应用，插件设置里的端口也要跟着改", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.server.port).range(1024..=65535));
        });
    }

    /// 打开主窗口（翻译页）。与设置共用同一个窗口，原因见 `draw_settings`
    pub fn open_main(&mut self) {
        self.open_window(SettingsTab::Translate);
    }

    /// 打开设置。从翻译页进来的话落到第一个设置页，否则回到上次看的那页
    pub fn open_settings(&mut self) {
        let tab = if self.settings_tab == SettingsTab::Translate { SettingsTab::Service } else { self.settings_tab };
        self.open_window(tab);
    }

    fn open_window(&mut self, tab: SettingsTab) {
        if tab == SettingsTab::Translate {
            self.manual.want_focus = true;
        }
        self.settings_unminimize = true;
        if self.settings_open {
            // 已经开着：只换页、提到前面。**不能重置 draft**，否则设置页里没保存的改动被悄悄丢掉
            if self.settings_tab != tab {
                self.settings_tab = tab;
                self.settings_scroll_top = true;
            }
            self.settings_raise = true;
            return;
        }
        self.settings_tab = tab;
        self.settings_scroll_top = true;
        self.draft = self.cfg.clone();
        self.settings_open = true;
        self.settings_place_retries = 0;
        // 连摆 5 帧（约 165 ms）保证命令落地；只摆一帧会偶发丢失
        self.settings_place_frames = PLACE_FRAMES;
    }

    pub fn request_quit(&mut self) {
        // 先收字幕：它没有自己的托盘和热键，留成孤儿进程的话用户只能去任务管理器杀。
        // 走优雅退出而不是 kill——字幕的 claude 后端有常驻子进程，强杀不回收。
        self.caption.stop();
        if let Some(s) = &self.server {
            s.stop();
        }
        self.quit = true;
    }

}

/// 窗口客户区尺寸（交给 `ViewportCommand::InnerSize`）。设置页 771x691 是 Jack 2026-09-09
/// 用鼠标调出来的顺手值。**单位是逻辑像素**：2026-09-22 实拍，给翻译页 860 宽落地成 1720 物理宽
/// （200% 缩放）——与早先「按物理像素落地」的记录相反，以这次截图为准。
/// 翻译页是查词窗口，窄而竖长，结果卡片在里面滚
fn window_size(tab: SettingsTab) -> (f32, f32) {
    if tab == SettingsTab::Translate {
        (520.0, 600.0)
    } else {
        (771.0, 691.0)
    }
}

/// 结果卡片：与浮动卡片同款深底（`draw_entry` 与正文都是按深底配的白字），内容长了在卡片里滚
fn result_card(ui: &mut egui::Ui, max_h: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .fill(Color32::from_rgba_unmultiplied(28, 28, 32, 246))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(max_h - 28.0).show(ui, add);
        });
}

/// 窗口标题：翻译页是主窗口，其余是设置
fn window_title(tab: SettingsTab) -> String {
    if tab == SettingsTab::Translate {
        APP_NAME_ZH.to_string()
    } else {
        format!("{APP_NAME_ZH} 设置")
    }
}

/// 取根窗口句柄（弹原生菜单要用）
#[cfg(windows)]
fn window_handle_of(frame: &eframe::Frame) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

#[cfg(not(windows))]
fn window_handle_of(_frame: &eframe::Frame) -> isize {
    0
}

/// 画一张词典卡片（版式参考有道取词：词头 → 音标 → 按词性分行的释义 → 词形 → 例句）。
///
/// 每块都可能缺：模型不知道就整行省略，流式过程中后面的行还没到。所以每块都单独判空，
/// 不要写成「有词头就一定有音标」那种连坐。
/// 词典条目版式。`light` = 画在明亮底上（浮动卡片）还是深色底上（主窗口结果卡片）：
/// 同一套版式两种配色，改字色时两处都要看（Jack 2026-09-24 要求浮窗改明亮风格）。
fn draw_entry(ui: &mut egui::Ui, e: &crate::dict::Entry, original: &str, base: f32, light: bool) {
    let c = |dark: u8, lite: u8| {
        if light {
            Color32::from_gray(lite)
        } else {
            Color32::from_gray(dark)
        }
    };
    let accent = if light { Color32::from_rgb(40, 90, 180) } else { Color32::from_rgb(150, 180, 255) };
    // 词头：模型偶尔不回 #词，就拿取到的原文顶上，别让卡片没有标题
    let head = if e.head.is_empty() { original.trim() } else { e.head.as_str() };
    if !head.is_empty() {
        ui.label(RichText::new(head).color(c(255, 25)).size(base + 9.0).strong());
    }
    if !e.phonetic.is_empty() {
        ui.add_space(1.0);
        ui.label(RichText::new(&e.phonetic).color(c(140, 110)).size(base - 3.0));
    }
    if !e.senses.is_empty() {
        ui.add_space(8.0);
        for s in &e.senses {
            ui.horizontal_top(|ui| {
                // 词性列宽固定，多个词性的释义才能左对齐成一栏
                ui.add_sized([34.0, base + 2.0], egui::Label::new(RichText::new(&s.pos).color(accent).size(base - 2.0)));
                ui.add(egui::Label::new(RichText::new(&s.text).color(c(235, 35)).size(base)).wrap());
            });
            ui.add_space(4.0);
        }
    }
    if !e.forms.is_empty() {
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(&e.forms).color(c(130, 100)).size(base - 4.0)).wrap());
    }
    if !e.example.is_empty() {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(&e.example).color(c(205, 70)).size(base - 2.0).italics()).wrap());
        if !e.example_zh.is_empty() {
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(&e.example_zh).color(c(150, 100)).size(base - 3.0)).wrap());
        }
    }
}

/// 有未保存改动时「保存并应用」填成的颜色：与划词小图标同一个蓝，界面上只有一种强调色
const SAVE_DIRTY_FILL: Color32 = Color32::from_rgb(40, 90, 180);

/// 设置窗的「保存并应用」按钮。`dirty` 时填主色、白字，否则就是普通按钮
fn save_button(dirty: bool) -> egui::Button<'static> {
    let text = RichText::new("保存并应用").size(12.0);
    if dirty {
        egui::Button::new(text.color(Color32::WHITE).strong()).fill(SAVE_DIRTY_FILL)
    } else {
        egui::Button::new(text)
    }
}

fn scene_of(label: &str) -> Scene {
    match label {
        "整页翻译" => Scene::FullPage,
        "悬停翻译" => Scene::Hover,
        "截屏翻译" => Scene::Screenshot,
        "取词释义" => Scene::Word,
        _ => Scene::Selection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 加设置页最容易漏的两处：`ALL` 里没挂上（界面里根本看不到），
    /// 或者 label / hint 忘了补（导航栏上是空白一条）
    #[test]
    fn every_settings_tab_is_listed_and_labelled() {
        let mut labels: Vec<&str> = Vec::new();
        for t in SettingsTab::ALL {
            assert!(!t.label().is_empty(), "{t:?} 没有标题");
            assert!(!t.hint().is_empty(), "{t:?} 没有说明");
            labels.push(t.label());
        }
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "两个页签重名，用户分不清");
        assert!(SettingsTab::ALL.contains(&SettingsTab::About), "关于页要挂在导航栏里");
        assert_eq!(SettingsTab::ALL[0], SettingsTab::Translate, "翻译页是主窗口的门面，放第一个");
    }

    #[test]
    fn scene_label_round_trip() {
        assert_eq!(scene_of("悬停翻译"), Scene::Hover);
        assert_eq!(scene_of("截屏翻译"), Scene::Screenshot);
        assert_eq!(scene_of("划词翻译"), Scene::Selection);
        assert_eq!(scene_of("取词释义"), Scene::Word);
        assert_eq!(scene_of("乱写"), Scene::Selection);
        // 重试按钮拿 scene_label 反查场景，标签变了这里必须跟着变
        for sc in Scene::ALL {
            assert_eq!(scene_of(sc.label_zh()), sc);
        }
    }

    /// 画一帧按钮，收集它画出来的矩形填充色
    fn fills_of(dirty: bool) -> Vec<Color32> {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(save_button(dirty));
        });
        // 没有渲染器来消费字体纹理，不清掉的话 epaint 在 Drop 里断言失败
        out.textures_delta.clear();
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) => Some(r.fill),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn save_button_is_highlighted_only_when_dirty() {
        assert!(fills_of(true).contains(&SAVE_DIRTY_FILL), "有改动时按钮要填主色");
        assert!(!fills_of(false).contains(&SAVE_DIRTY_FILL), "没改动时按钮不该是主色");
    }

    #[test]
    fn every_feature_round_trips_and_has_a_menu_item() {
        let mut ids = Vec::new();
        for f in Feature::ALL {
            let mut c = Config::default();
            let was = f.get(&c);
            f.set(&mut c, !was);
            assert_eq!(f.get(&c), !was, "{f:?} 读写的不是同一个字段");
            // 改一项不许牵动别的项
            for g in Feature::ALL.into_iter().filter(|g| *g != f) {
                assert_eq!(g.get(&c), g.get(&Config::default()), "改 {f:?} 牵动了 {g:?}");
            }
            assert_eq!(Feature::from_menu_id(f.menu_id()), Some(f));
            assert!(!f.menu_label(&c).is_empty());
            assert_ne!(f.notice(&c, true), f.notice(&c, false));
            ids.push(f.menu_id());
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Feature::ALL.len());
        assert_eq!(Feature::from_menu_id(crate::tray::MENU_SETTINGS), None);
    }

    #[test]
    fn watch_cfg_clamps_delay() {
        let mut c = Config::default();
        c.hotkeys.hover_delay_ms = 5;
        assert_eq!(watch_cfg(&c).hover_delay, Duration::from_millis(80));
        c.hotkeys.hover_delay_ms = 99_999;
        assert_eq!(watch_cfg(&c).hover_delay, Duration::from_millis(5000));
        c.hotkeys.hover_modifier = "ctrl".into();
        assert_eq!(watch_cfg(&c).hover_modifier, HoverModifier::Ctrl);
    }
}
