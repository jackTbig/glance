//! egui 界面：译文浮窗、划词小图标、截屏框选、设置窗、托盘
use crate::config::Config;
use crate::dpi::Dpi;
use crate::engine::{Engine, Event, Job};
use crate::hotkey::{parse_hotkey, HoverModifier};
use crate::panel::{place, place_icon, PanelState, Phase};
use crate::watcher::{Trigger, WatchConfig, Watcher};
use ai_translate::Scene;
use crossbeam_channel::Receiver;
use egui::{Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, RichText, ViewportBuilder, ViewportCommand, ViewportId};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use text_capture::{Capturer, Source};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

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
            SettingsTab::Server => "插件靠本地服务工作，这里配端口",
        }
    }
}

/// 设置项左侧标签的固定宽度，让每行的控件左边缘对齐
const LABEL_W: f32 = 92.0;
/// 长输入框的宽度
const FIELD_W: f32 = 288.0;

/// 用户配的热键被别的程序占了就往下试这些。
/// 常见组合（Ctrl+Alt+T/S/X/D/W、Ctrl+Shift+D）在实测机器上都已被占用，别放进来。
const FALLBACK_HOTKEYS: &[&str] = &[
    "Ctrl+Alt+Y", "Ctrl+Alt+G", "Ctrl+Alt+Q",
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
    settings_open: bool,
    settings_tab: SettingsTab,
    draft: Config,
    settings_msg: String,
    status: String,
    /// 服务句柄，重启服务时替换
    server: Option<crate::server::ServerHandle>,
    /// 持有着就在，丢掉图标就没了
    _tray: Option<tray_icon::TrayIcon>,
    paused: bool,
    quit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HotkeyAction {
    Selection,
    Screenshot,
    TogglePanel,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config) -> anyhow::Result<App> {
        install_cjk_font(&cc.egui_ctx);
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
            (&cfg.hotkeys.selection, HotkeyAction::Selection, "划词"),
            (&cfg.hotkeys.screenshot, HotkeyAction::Screenshot, "截屏"),
            (&cfg.hotkeys.toggle_panel, HotkeyAction::TogglePanel, "面板"),
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
            // 调试钩子：设 TT_OPEN_SETTINGS=1 启动即开设置窗（截图与自测用）
            settings_open: std::env::var("TT_OPEN_SETTINGS").is_ok_and(|v| v != "0"),
            // TT_SETTINGS_TAB=0..3 指定启动时停在哪一页（截图与自测用）
            settings_tab: std::env::var("TT_SETTINGS_TAB")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .and_then(|i| SettingsTab::ALL.get(i).copied())
                .unwrap_or(SettingsTab::Service),
            settings_msg: String::new(),
            status,
            server,
            _tray: match crate::tray::build("translate-tool 翻译工具") {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!("{e}");
                    None
                }
            },
            paused: false,
            quit: false,
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
                HotkeyAction::Selection => self.start(Job::Selection, "划词翻译", (x, y), Source::Clipboard),
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
                Event::Captured { text, source, took_ms, .. } => {
                    p.original = text;
                    p.source = source;
                    p.phase = Phase::Working(format!("{} {took_ms}ms · 翻译中…", source.label_zh()));
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
        use crate::tray::{MENU_PAUSE, MENU_QUIT, MENU_SETTINGS};
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
        let items = vec![
            (MENU_SETTINGS, "设置…".to_string(), false),
            (MENU_PAUSE, "暂停取词".to_string(), self.paused),
            (0, String::new(), false),
            (MENU_QUIT, "退出".to_string(), false),
        ];
        match crate::tray::show_menu(hwnd, x, y, &items) {
            Some(MENU_SETTINGS) => self.open_settings(),
            Some(MENU_PAUSE) => {
                self.paused = !self.paused;
                self.watcher.set_paused(self.paused);
                tracing::info!("取词已{}", if self.paused { "暂停" } else { "恢复" });
            }
            Some(MENU_QUIT) => self.request_quit(),
            _ => {}
        }
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
        hover_modifier: HoverModifier::parse(&cfg.hotkeys.hover_modifier),
        hover_delay: Duration::from_millis(cfg.hotkeys.hover_delay_ms.clamp(80, 5000)),
        ..Default::default()
    }
}

fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\msyh.ttf", r"C:\Windows\Fonts\simhei.ttf"];
    for p in candidates {
        if let Ok(bytes) = std::fs::read(p) {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "cjk".into());
            fonts.families.get_mut(&FontFamily::Monospace).unwrap().push("cjk".into());
            ctx.set_fonts(fonts);
            return;
        }
    }
    tracing::warn!("没找到中文字体，界面里的中文会显示成方块");
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

        // 框选完等遮罩真的从屏幕上撤走再抓图（见 draw_region_selector 末尾）
        if let Some((rect, at)) = self.pending_region {
            if at.elapsed() >= MASK_CLEAR_DELAY {
                self.pending_region = None;
                let (x, y, w, h) = rect;
                self.start(Job::Region { x, y, w, h }, "截屏翻译", (x, y + h as i32), Source::Ocr);
            }
        }

        // 小图标超时消失
        if let Mode::Icon { since, .. } = self.mode {
            if since.elapsed() > Duration::from_millis(self.cfg.ui.icon_timeout_ms) {
                self.mode = Mode::Idle;
            }
        }

        match self.mode.clone() {
            Mode::Idle => self.draw_hidden(ui),
            Mode::Icon { pos, .. } => self.draw_icon(ui, pos),
            Mode::Panel => self.draw_panel(ui),
            Mode::Selecting { start } => self.draw_region_selector(ui, start),
        }
        if self.settings_open {
            self.draw_settings(&ctx);
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
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(w, h)));

        let mut close = false;
        let mut pin_toggled = false;
        let mut retry = false;
        let mut copy = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgba_unmultiplied(28, 28, 32, 246)).corner_radius(CornerRadius::same(10)).inner_margin(12.0))
            .show(root_ui, |ui| {
                ui.horizontal(|ui| {
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
                        if ui.small_button("⎘").on_hover_text("复制译文").clicked() {
                            copy = true;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(h - 90.0).show(ui, |ui| {
                    if !p.original.is_empty() {
                        ui.label(RichText::new(&p.original).color(Color32::from_gray(150)).size(self.cfg.ui.font_size - 1.0));
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
                                ui.label(RichText::new(&p.translated).color(Color32::from_gray(230)).size(self.cfg.ui.font_size));
                            }
                        }
                        Phase::Done => {
                            ui.label(RichText::new(&p.translated).color(Color32::WHITE).size(self.cfg.ui.font_size));
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
            });

        if let Some(p) = self.panel.as_mut() {
            p.pos = (x, y);
            if pin_toggled {
                p.pinned = !p.pinned;
            }
        }
        if copy {
            let _ = text_capture::clipboard::set_text(&p.translated);
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

    fn draw_settings(&mut self, ctx: &egui::Context) {
        // ViewportBuilder 的尺寸这里按物理像素落地（实测 200% 屏上给 780 得到的窗口就是 780 物理宽，
        // 只有想要的一半），所以要自己乘缩放比，否则高分屏上内容会被切掉。
        let k = crate::dpi::Dpi::from_ctx(ctx).ppp();
        let vb = ViewportBuilder::default()
            .with_title("translate-tool 设置")
            .with_inner_size([600.0 * k, 440.0 * k])
            .with_min_inner_size([520.0 * k, 360.0 * k]);
        let mut close = false;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("settings"), vb, |vp_ui, _class| {
            let vp_ctx = vp_ui.ctx().clone();

            // 按钮栏固定在底部，不跟着内容滚
            egui::Panel::bottom("settings-actions").show(vp_ui, |ui| {
                ui.add_space(7.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("保存并应用").min_size(egui::vec2(98.0, 27.0))).clicked() {
                        self.apply_settings();
                    }
                    if ui.add(egui::Button::new("测试翻译").min_size(egui::vec2(82.0, 27.0))).clicked() {
                        let cfg = self.draft.translator_config();
                        self.settings_msg = match ai_translate::Translator::new(cfg) {
                            Ok(t) => match t.translate(Scene::Selection, "Hello, this is a test.", None, &mut |_| {}) {
                                Ok(zh) => format!("测试通过：{zh}"),
                                Err(e) => format!("测试失败：{e}"),
                            },
                            Err(e) => format!("配置有误：{e}"),
                        };
                    }
                    if !self.settings_msg.is_empty() {
                        let bad = self.settings_msg.contains("失败") || self.settings_msg.contains("有误");
                        let color = if bad { Color32::from_rgb(200, 90, 90) } else { Color32::from_rgb(80, 155, 95) };
                        ui.add_space(6.0);
                        ui.label(RichText::new(&self.settings_msg).size(12.0).color(color));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("关闭").min_size(egui::vec2(66.0, 27.0))).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(7.0);
            });

            // 左侧分类导航；运行状态挂在导航下方，不占正文
            egui::Panel::left("settings-nav").exact_size(118.0).show(vp_ui, |ui| {
                ui.add_space(8.0);
                for tab in SettingsTab::ALL {
                    let selected = self.settings_tab == tab;
                    if ui
                        .add_sized([104.0, 27.0], egui::Button::selectable(selected, RichText::new(tab.label()).size(13.0)))
                        .clicked()
                    {
                        self.settings_tab = tab;
                    }
                    ui.add_space(2.0);
                }
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("运行状态").size(11.0).color(Color32::from_gray(110)));
                ui.add_space(3.0);
                for line in self.status.split(" · ") {
                    ui.label(RichText::new(line).size(11.0).color(Color32::from_gray(140)));
                }
            });

            egui::CentralPanel::default().show(vp_ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(self.settings_tab.label()).size(17.0).strong());
                ui.label(RichText::new(self.settings_tab.hint()).size(11.0).color(Color32::from_gray(140)));
                ui.add_space(9.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.settings_tab {
                    SettingsTab::Service => self.settings_service(ui),
                    SettingsTab::Trigger => self.settings_trigger(ui),
                    SettingsTab::Glossary => self.settings_glossary(ui),
                    SettingsTab::Server => self.settings_server(ui),
                });
            });

            if vp_ctx.input(|i| i.viewport().close_requested()) {
                close = true;
            }
        });
        if close {
            self.settings_open = false;
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
                    egui::ComboBox::from_id_salt(label).width(150.0).selected_text(field.clone()).show_ui(ui, |ui| {
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
        ui.label(RichText::new("全局快捷键").size(13.0).strong());
        ui.label(
            RichText::new("在任何窗口里都生效。组合被别的程序占用时会自动改用备选，实际用的是哪个看左下角运行状态。")
                .size(11.0)
                .color(Color32::from_gray(135)),
        );
        ui.add_space(9.0);
        Self::row(ui, "划词翻译", "选中文字后按它，立刻翻译选中的内容", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.hotkeys.selection).desired_width(170.0));
        });
        Self::row(ui, "截屏翻译", "按它进入框选，拖出一块区域后识别并翻译", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.hotkeys.screenshot).desired_width(170.0));
        });
        Self::row(ui, "面板开关", "显示或收起最近一次的译文面板", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.draft.hotkeys.toggle_panel).desired_width(170.0));
        });

        Self::section(
            ui,
            "悬停翻译",
            "按住修饰键、把鼠标停在文字上，自动翻译鼠标压着的那一行。浏览器里请用 Chrome 插件，效果更好。",
        );
        Self::row(ui, "修饰键", "选「关闭」就完全不做悬停翻译", |ui| {
            let cur = match self.draft.hotkeys.hover_modifier.as_str() {
                "ctrl" => "Ctrl",
                "shift" => "Shift",
                "win" => "Win",
                "none" => "关闭",
                _ => "Alt",
            };
            egui::ComboBox::from_id_salt("hovermod").width(150.0).selected_text(cur).show_ui(ui, |ui| {
                for (v, label) in [("alt", "Alt"), ("ctrl", "Ctrl"), ("shift", "Shift"), ("win", "Win"), ("none", "关闭")] {
                    ui.selectable_value(&mut self.draft.hotkeys.hover_modifier, v.to_string(), label);
                }
            });
        });
        Self::row(ui, "停留多久触发", "鼠标停住这么久才翻译；太短会一划过去就触发", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.hotkeys.hover_delay_ms).range(80..=3000).suffix(" ms"));
        });

        Self::section(ui, "划词小图标", "");
        Self::row(ui, "选中后弹图标", "关掉后选中文字不再自动冒出「译」按钮，只能用上面的划词快捷键", |ui| {
            ui.checkbox(&mut self.draft.ui.selection_icon, "");
        });
        Self::row(ui, "图标停留", "多久不点就自动消失", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.ui.icon_timeout_ms).range(500..=10000).suffix(" ms"));
        });

        Self::section(
            ui,
            "例外程序",
            "在这些程序里不弹小图标、不响应悬停，快捷键仍然有效。每行一个可执行文件名。留空表示不排除任何程序。",
        );
        let mut text = self.draft.ui.excluded_processes.join("\n");
        let r = ui.add(
            egui::TextEdit::multiline(&mut text)
                .desired_width(FIELD_W)
                .desired_rows(3)
                .hint_text("code.exe\ndevenv.exe"),
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
                ui.add(egui::TextEdit::singleline(&mut g.src).desired_width(210.0).hint_text("原文，如 pod"));
                ui.label(RichText::new("→").color(Color32::from_gray(140)));
                ui.add(egui::TextEdit::singleline(&mut g.dst).desired_width(210.0).hint_text("固定译法，如 容器组"));
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
        if ui.add(egui::Button::new("＋ 添加一条").min_size(egui::vec2(112.0, 27.0))).clicked() {
            self.draft.glossary.push(Default::default());
        }
    }

    fn settings_server(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Chrome 插件靠这个本地服务翻译。只监听 127.0.0.1，且只接受浏览器扩展的请求，网页直接调一律拒绝。")
                .size(11.0)
                .color(Color32::from_gray(135)),
        );
        ui.add_space(12.0);
        Self::row(ui, "启用", "关掉后 Chrome 插件无法工作", |ui| {
            ui.checkbox(&mut self.draft.server.enabled, "");
        });
        Self::row(ui, "端口", "改完要重启应用，插件那边的端口也要跟着改", |ui| {
            ui.add(egui::DragValue::new(&mut self.draft.server.port).range(1024..=65535));
        });

        Self::section(ui, "插件怎么装", "");
        for line in [
            "1. Chrome 地址栏打开 chrome://extensions",
            "2. 打开右上角「开发者模式」",
            "3. 点「加载已解压的扩展程序」，选安装目录下的 extension 文件夹",
        ] {
            ui.label(RichText::new(line).size(12.0).color(Color32::from_gray(150)));
            ui.add_space(3.0);
        }
    }

    pub fn open_settings(&mut self) {
        self.draft = self.cfg.clone();
        self.settings_open = true;
    }

    pub fn request_quit(&mut self) {
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

fn scene_of(label: &str) -> Scene {
    match label {
        "整页翻译" => Scene::FullPage,
        "悬停翻译" => Scene::Hover,
        "截屏翻译" => Scene::Screenshot,
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
        assert_eq!(scene_of("乱写"), Scene::Selection);
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
