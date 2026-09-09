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
    Service,
    Trigger,
    Glossary,
    Server,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 4] = [SettingsTab::Service, SettingsTab::Trigger, SettingsTab::Glossary, SettingsTab::Server];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::Service => "翻译服务",
            SettingsTab::Trigger => "触发方式",
            SettingsTab::Glossary => "术语表",
            SettingsTab::Server => "浏览器插件",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SettingsTab::Service => "接口地址、API Key、各场景用哪个后端",
            SettingsTab::Trigger => "快捷键、悬停、划词图标，以及不想被打扰的程序",
            SettingsTab::Glossary => "让专有名词按你定的译法翻",
            SettingsTab::Server => "安装或更新 Chrome 插件，以及它依赖的本地服务",
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

/// 界面里正在做什么
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Idle,
    /// 选区旁的小图标
    Icon { pos: (i32, i32), since: Instant },
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
    /// 上次记进日志的小图标位置。界面 30 fps 重画，不去重的话一秒刷 30 行
    logged_icon_at: Option<(i32, i32)>,
    /// 当前有效的任务序号，用来丢弃过期事件
    cur_seq: u64,
    mouse_left_panel_at: Option<Instant>,
    /// 状态提示卡片什么时候自己收掉
    notice_until: Option<Instant>,
    settings_open: bool,
    settings_tab: SettingsTab,
    /// 换页后把滚动条拉回顶部
    settings_scroll_top: bool,
    /// 设置窗这次打开是否已经摆过位置
    settings_sized: bool,
    draft: Config,
    settings_msg: String,
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
            logged_icon_at: None,
            cur_seq: 0,
            mouse_left_panel_at: None,
            notice_until: None,
            // 调试钩子：设 TT_OPEN_SETTINGS=1 启动即开设置窗（截图与自测用）
            settings_open: std::env::var("TT_OPEN_SETTINGS").is_ok_and(|v| v != "0"),
            // TT_SETTINGS_TAB=0..3 指定启动时停在哪一页（截图与自测用）
            settings_tab: std::env::var("TT_SETTINGS_TAB")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .and_then(|i| SettingsTab::ALL.get(i).copied())
                .unwrap_or(SettingsTab::Service),
            settings_scroll_top: false,
            settings_sized: false,
            settings_msg: String::new(),
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
                    app.start(Job::Text { scene: Scene::Word, text: w.trim().into() }, Scene::Word.label_zh(), (600, 400), Source::Clipboard);
                    // 钉住，不然鼠标没停在卡片上，出结果后一会儿就自己收了，截不到
                    if let Some(p) = app.panel.as_mut() {
                        p.pinned = true;
                    }
                }
            }
            app
        })
    }

    fn start(&mut self, job: Job, scene_label: &'static str, anchor: (i32, i32), source: Source) {
        tracing::debug!("{scene_label} 开始，锚点物理({},{}) job={job:?}", anchor.0, anchor.1);
        self.cur_seq = self.engine.submit(job);
        self.panel = Some(PanelState::new(scene_label, source, anchor));
        self.mode = Mode::Panel;
        self.mouse_left_panel_at = None;
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
            match t {
                Trigger::SelectionEnded { x, y } => {
                    if !self.cfg.ui.selection_icon {
                        tracing::debug!("划词结束于({x},{y})，但设置里关了小图标");
                    } else if matches!(self.mode, Mode::Selecting { .. }) {
                        tracing::debug!("划词结束于({x},{y})，但正在框选，忽略");
                    } else if self.excluded_at(x, y).is_none() {
                        tracing::debug!("划词结束于物理({x},{y})，弹小图标");
                        self.mode = Mode::Icon { pos: (x, y), since: Instant::now() };
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
                        self.start(Job::Hover { x, y }, "悬停翻译", (x, y), Source::Uia);
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
                HotkeyAction::ToggleHover => {
                    self.cfg.ui.hover_enabled = !self.cfg.ui.hover_enabled;
                    self.draft.ui.hover_enabled = self.cfg.ui.hover_enabled;
                    self.watcher.set_config(watch_cfg(&self.cfg));
                    let on = self.cfg.ui.hover_enabled;
                    let key = HoverModifier::parse(&self.cfg.hotkeys.hover_modifier).label_zh();
                    tracing::info!("悬停取词{}", if on { "已开启" } else { "已关闭" });
                    self.notice(
                        "悬停取词",
                        if on {
                            format!("已开启：按住 {key} 把鼠标停在文字上就翻")
                        } else {
                            "已关闭：按住修饰键也不再响应".into()
                        },
                        (x, y),
                    );
                    let _ = self.cfg.save();
                }
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
                HotkeyAction::ToggleSelection => {
                    self.cfg.ui.selection_icon = !self.cfg.ui.selection_icon;
                    self.draft.ui.selection_icon = self.cfg.ui.selection_icon;
                    let on = self.cfg.ui.selection_icon;
                    tracing::info!("划词小图标{}", if on { "已开启" } else { "已关闭" });
                    self.notice(
                        "划词翻译",
                        if on { "已开启：选中文字后点旁边的「译」".into() } else { "已关闭：选中文字不再弹图标".to_string() },
                        (x, y),
                    );
                    let _ = self.cfg.save();
                }
                HotkeyAction::Screenshot => {
                    self.mode = Mode::Selecting { start: None };
                    self.panel = None;
                    self.pending_region = None;
                    self.focus_selector = true;
                }
                HotkeyAction::TogglePanel => {
                    if self.panel.is_some() {
                        self.close_panel();
                    } else {
                        self.start(Job::Selection, "划词翻译", (x, y), Source::Clipboard);
                    }
                }
            }
        }
    }

    fn drain_events(&mut self) {
        while let Ok(e) = self.events.try_recv() {
            let seq = match &e {
                Event::Captured { seq, .. } | Event::Delta { seq, .. } | Event::Done { seq, .. } | Event::Failed { seq, .. } | Event::Nothing { seq, .. } => *seq,
            };
            if seq != self.cur_seq {
                continue; // 过期
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
                Event::Delta { text, .. } => {
                    p.translated = text;
                }
                Event::Done { text, took_ms, .. } => {
                    p.translated = text;
                    p.phase = Phase::Done;
                    tracing::info!("{} 完成 {took_ms}ms", p.scene_label);
                }
                Event::Failed { error, .. } => p.phase = Phase::Failed(error),
                Event::Nothing { reason, .. } => p.phase = Phase::Failed(reason),
            }
        }
    }

    /// 托盘点击 → 自己弹原生菜单（不挂 muda，见 tray.rs 顶部说明）
    fn drain_tray(&mut self, _ctx: &egui::Context, frame: &mut eframe::Frame) {
        use crate::tray::{
            MENU_CAPTION_CLICKABLE, MENU_CAPTION_MINI, MENU_CAPTION_PAUSE, MENU_CAPTION_SETTINGS, MENU_CAPTION_TOGGLE,
            MENU_PAUSE, MENU_QUIT, MENU_SETTINGS,
        };
        use desktop_shell::caption_ipc::Command;
        let mut clicked_at = None;
        while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
            if let tray_icon::TrayIconEvent::Click { position, button, button_state, .. } = ev {
                let _ = button;
                if button_state == tray_icon::MouseButtonState::Up {
                    clicked_at = Some((position.x as i32, position.y as i32));
                }
            }
        }
        let Some((x, y)) = clicked_at else { return };
        let hwnd = window_handle_of(frame);
        let mut items = vec![
            (MENU_SETTINGS, "设置…".to_string(), false),
            (MENU_PAUSE, "暂停取词".to_string(), self.paused),
        ];
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
            Some(MENU_SETTINGS) => self.open_settings(),
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

    /// 弹一张只报状态的卡片（没有重试按钮），1.6 秒后自己收
    fn notice(&mut self, title: &'static str, msg: String, at: (i32, i32)) {
        let mut st = PanelState::new(title, Source::Uia, at);
        st.phase = Phase::Notice(msg);
        self.panel = Some(st);
        self.mode = Mode::Panel;
        self.cur_seq = 0;
        self.notice_until = Some(Instant::now() + Duration::from_millis(1600));
    }

    fn close_panel(&mut self) {
        self.logged_icon_at = None;
        self.panel = None;
        self.mode = Mode::Idle;
        self.mouse_left_panel_at = None;
    }

    fn apply_settings(&mut self) {
        match ai_translate::Translator::new(self.draft.translator_config()) {
            Ok(t) => {
                *self.translator.lock().unwrap() = t;
                self.cfg = self.draft.clone();
                self.watcher.set_config(watch_cfg(&self.cfg));
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.quit {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            return;
        }
        self.drain_hotkeys();
        self.drain_triggers();
        self.drain_events();
        self.drain_tray(&ctx, _frame);
        // 回收退出的字幕子进程，并按退出码决定要不要重新拉起（改识别参数会请求重启）。
        // 用 try_wait 不用 wait：后者会把 UI 线程堵死。
        self.caption.reap();

        // 框选完等遮罩真的从屏幕上撤走再抓图（见 draw_region_selector 末尾）
        if let Some((rect, at)) = self.pending_region {
            if at.elapsed() >= MASK_CLEAR_DELAY {
                self.pending_region = None;
                let (x, y, w, h) = rect;
                self.start(Job::Region { x, y, w, h }, "截屏翻译", (x, y + h as i32), Source::Ocr);
            }
        }

        // 状态提示卡片到点自己收
        if let Some(t) = self.notice_until {
            if Instant::now() >= t {
                self.notice_until = None;
                self.close_panel();
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
                Mode::Icon { pos, .. } => self.draw_icon(ui, pos),
                Mode::Panel => self.draw_panel(ui),
                Mode::Selecting { start } => self.draw_region_selector(ui, start),
            }
        }
        // 30 fps 足够，别空转烧 CPU
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

impl App {
    /// 空闲时把主窗口缩到看不见的一格（egui 的根视口不能真的隐藏）
    fn draw_hidden(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(1.0, 1.0)));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(-32000.0, -32000.0)));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |_| {});
    }

    fn draw_icon(&mut self, ui: &mut egui::Ui, pos: (i32, i32)) {
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
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(logical));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(ICON, ICON)));
        let mut clicked = false;
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let r = ui.allocate_response(egui::vec2(ICON, ICON), egui::Sense::click());
            let painter = ui.painter();
            let bg = if r.hovered() { Color32::from_rgb(60, 120, 220) } else { Color32::from_rgb(40, 90, 180) };
            painter.rect_filled(r.rect, CornerRadius::same(8), bg);
            painter.text(r.rect.center(), Align2::CENTER_CENTER, "译", egui::FontId::proportional(16.0), Color32::WHITE);
            if r.clicked() {
                clicked = true;
            }
        });
        if clicked {
            self.start(Job::Selection, "划词翻译", pos, Source::Clipboard);
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
        let w = self.cfg.ui.panel_width;
        let h = self.cfg.ui.panel_max_height;
        let size_phys = dpi.to_physical_size((w, h));
        let (x, y) = if p.pinned { p.pos } else { place(p.anchor, size_phys, screen, dpi.to_physical_len(14.0)) };
        let logical = dpi.to_logical_pos((x, y));
        if (x, y) != p.pos {
            tracing::debug!("面板：锚点物理{:?} → 摆到物理({x},{y}) → 逻辑{logical:?} 尺寸逻辑({w}x{h}) ppp={}", p.anchor, dpi.ppp());
        }
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(logical));
        // 高度跟着上一帧量到的内容走，短词条不拖一大截空白；摆放仍按最大高度算，
        // 免得卡片一变矮就往下跳一次（place 会在放不下时翻到锚点上方）
        let inner_h = if p.view_h > 0.0 { p.view_h.clamp(72.0, h) } else { h };
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(w, inner_h)));

        let mut close = false;
        let mut pin_toggled = false;
        let mut retry = false;
        let mut copy = false;
        let (mut drag_started, mut dragging, mut drag_stopped) = (false, false, false);
        let mut content_h = 0.0;
        // 取词场景才认词典格式：普通译文里偶尔也有 # 开头的行（Markdown 标题、井号话题），
        // 不限定场景的话一句正常译文会被切成一张残缺的词典卡
        let entry = (scene_of(p.scene_label) == Scene::Word).then(|| crate::dict::parse(&p.translated)).flatten();
        let base = self.cfg.ui.font_size;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(28, 28, 32, 246)).corner_radius(CornerRadius::same(10)).inner_margin(12.0))
            .show(root_ui, |ui| {
                let head = ui.horizontal(|ui| {
                    ui.label(RichText::new(p.scene_label).color(Color32::from_rgb(150, 180, 255)).size(13.0));
                    ui.label(RichText::new(format!("· {}", p.source.label_zh())).color(Color32::GRAY).size(12.0));
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
                let out = egui::ScrollArea::vertical().max_height(h - 90.0).show(ui, |ui| {
                    if let Some(e) = &entry {
                        // 词典卡：词头本身就是原文，不再另起一行灰色原文，省得重复
                        draw_entry(ui, e, &p.original, base);
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
                        ui.label(RichText::new(&p.original).color(Color32::from_gray(150)).size(base - 1.0));
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
                                ui.label(RichText::new(&p.translated).color(Color32::from_gray(230)).size(base));
                            }
                        }
                        Phase::Done => {
                            ui.label(RichText::new(&p.translated).color(Color32::WHITE).size(base));
                        }
                        Phase::Notice(msg) => {
                            ui.label(RichText::new(msg).color(Color32::from_gray(170)).size(13.0));
                        }
                        Phase::Failed(e) => {
                            ui.label(RichText::new(e).color(Color32::from_rgb(240, 130, 130)).size(13.0));
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
            });

        if let Some(p) = self.panel.as_mut() {
            p.pos = (x, y);
            // 12 的内边距上下各一份
            p.view_h = content_h + 24.0;
            if drag_started {
                // 拖动即钉住：特意挪过位置的卡片再自己消失最恼人
                p.drag_from = Some((text_capture::screen::cursor_pos(), p.pos));
                p.pinned = true;
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

        // 鼠标离开面板一段时间后自动关（没钉住时）。鼠标是物理坐标，面板矩形也用物理的。
        let (mx, my) = text_capture::screen::cursor_pos();
        let inside = mx >= x && my >= y && mx <= x + size_phys.0 as i32 && my <= y + size_phys.1 as i32;
        if inside {
            self.mouse_left_panel_at = None;
        } else if self.mouse_left_panel_at.is_none() {
            self.mouse_left_panel_at = Some(Instant::now());
        }
        let outside_for = self.mouse_left_panel_at.map(|t| t.elapsed());
        if let Some(p) = &self.panel {
            if p.should_auto_close(outside_for, Duration::from_millis(1200)) {
                self.close_panel();
            }
        }
    }

    /// 截屏框选：整屏半透明遮罩，拖出一个亮框
    fn draw_region_selector(&mut self, root_ui: &mut egui::Ui, start: Option<(i32, i32)>) {
        let ctx = root_ui.ctx().clone();
        let dpi = Dpi::from_ctx(&ctx);
        // 虚拟屏是物理像素，遮罩窗的位置与大小要换成逻辑；
        // 框出来的矩形留在物理坐标系里，因为 BitBlt 抓的是物理像素
        let (sx, sy, sw, sh) = text_capture::screen::virtual_screen();
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(dpi.to_logical_pos((sx, sy))));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(dpi.to_logical_size((sw, sh))));
        if self.focus_selector {
            ctx.send_viewport_cmd(ViewportCommand::Focus);
            self.focus_selector = false;
        }
        let (mx, my) = text_capture::screen::cursor_pos();
        let mut finish: Option<(i32, i32, u32, u32)> = None;
        let mut cancel = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(0, 0, 0, 90)))
            .show(root_ui, |ui| {
                let painter = ui.painter();
                if let Some((ax, ay)) = start {
                    let r = egui::Rect::from_two_pos(
                        dpi.point_in_window((ax, ay), (sx, sy)),
                        dpi.point_in_window((mx, my), (sx, sy)),
                    );
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
                let resp = ui.allocate_response(ui.available_size(), egui::Sense::click_and_drag());
                if resp.drag_started() {
                    self.mode = Mode::Selecting { start: Some((mx, my)) };
                }
                if resp.drag_stopped() {
                    if let Some((ax, ay)) = start {
                        let (x0, y0) = (ax.min(mx), ay.min(my));
                        let (w, h) = ((ax - mx).unsigned_abs(), (ay - my).unsigned_abs());
                        if w >= 12 && h >= 12 {
                            finish = Some((x0, y0, w, h));
                        } else {
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
        // Jack 2026-09-09 用鼠标调出来的顺手尺寸（客户区物理像素）。
        // `ViewportCommand::InnerSize` 在这里按**物理**像素落地，实测给 386 得到的就是 386 物理宽。
        const W: f32 = 771.0;
        const H: f32 = 691.0;

        if !self.settings_sized {
            self.settings_sized = true;
            let d = crate::dpi::Dpi::from_ctx(&ctx);
            let (sx, sy, sw, sh) = text_capture::screen::virtual_screen();
            // 摆在左上角、离两边各留一段（Jack 2026-09-09 定的位置：屏幕居中太靠下）
            const MARGIN_LEFT: i32 = 160;
            const MARGIN_TOP: i32 = 120;
            let at = (
                (sx + MARGIN_LEFT).min(sx + sw as i32 - W as i32),
                (sy + MARGIN_TOP).min(sy + sh as i32 - H as i32),
            );
            ctx.send_viewport_cmd(ViewportCommand::Decorations(true));
            ctx.send_viewport_cmd(ViewportCommand::Title(format!("{APP_NAME_ZH} 设置")));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(W, H)));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(d.to_logical_pos(at)));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }

        let mut close = false;

        let mut do_apply = false;
        let mut do_test = false;
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
                }
                ui.add_space(2.0);
            }

            // 操作按钮放这儿而不是窗口底部：Panel::bottom 与 bottom_up 布局在这个
            // 窗口里都排不出来（矩形落在可见区外 / 被 ScrollArea 挤掉），导航栏则稳定可见
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);
            do_apply = ui.add_sized([92.0, 26.0], egui::Button::new(RichText::new("保存并应用").size(12.0))).clicked();
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
                SettingsTab::Service => self.settings_service(ui),
                SettingsTab::Trigger => self.settings_trigger(ui),
                SettingsTab::Glossary => self.settings_glossary(ui),
                SettingsTab::Server => self.settings_server(ui),
            });
        });

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
            close = true;
        }
        if close {
            self.settings_open = false;
            self.settings_sized = false;
            // 变回那个躲在屏幕外的浮窗宿主
            ctx.send_viewport_cmd(ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(ViewportCommand::Title(APP_NAME_ZH.to_string()));
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

    pub fn open_settings(&mut self) {
        self.draft = self.cfg.clone();
        self.settings_open = true;
        self.settings_sized = false;
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
fn draw_entry(ui: &mut egui::Ui, e: &crate::dict::Entry, original: &str, base: f32) {
    // 词头：模型偶尔不回 #词，就拿取到的原文顶上，别让卡片没有标题
    let head = if e.head.is_empty() { original.trim() } else { e.head.as_str() };
    if !head.is_empty() {
        ui.label(RichText::new(head).color(Color32::WHITE).size(base + 9.0).strong());
    }
    if !e.phonetic.is_empty() {
        ui.add_space(1.0);
        ui.label(RichText::new(&e.phonetic).color(Color32::from_gray(140)).size(base - 3.0));
    }
    if !e.senses.is_empty() {
        ui.add_space(8.0);
        for s in &e.senses {
            ui.horizontal_top(|ui| {
                // 词性列宽固定，多个词性的释义才能左对齐成一栏
                ui.add_sized(
                    [34.0, base + 2.0],
                    egui::Label::new(RichText::new(&s.pos).color(Color32::from_rgb(150, 180, 255)).size(base - 2.0)),
                );
                ui.add(egui::Label::new(RichText::new(&s.text).color(Color32::from_gray(235)).size(base)).wrap());
            });
            ui.add_space(4.0);
        }
    }
    if !e.forms.is_empty() {
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(&e.forms).color(Color32::from_gray(130)).size(base - 4.0)).wrap());
    }
    if !e.example.is_empty() {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(&e.example).color(Color32::from_gray(205)).size(base - 2.0).italics()).wrap());
        if !e.example_zh.is_empty() {
            ui.add_space(2.0);
            ui.add(egui::Label::new(RichText::new(&e.example_zh).color(Color32::from_gray(150)).size(base - 3.0)).wrap());
        }
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
