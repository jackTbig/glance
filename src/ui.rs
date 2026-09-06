use crate::config::Config;
use crate::messages::{SegId, Stage, StageState, UiMsg};
use crate::transcript::{Transcript, TranscriptFile};
use crate::translate::TranslatorConfig;
use chrono::{DateTime, Local};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::path::PathBuf;
use egui::{Align, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, Id, Layout, ResizeDirection, RichText, Sense, ViewportBuilder, ViewportCommand, ViewportId};
use tray_icon::{MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MARGIN: f32 = 14.0;
/// 待机迷你条尺寸
const MINI_W: f32 = 260.0;
const MINI_H: f32 = 44.0;

struct Line {
    id: SegId,
    #[allow(dead_code)]
    at: DateTime<Local>,
    en: String,
    zh: Option<String>,
    /// 还在说的这句（中间识别结果），会被替换
    partial: bool,
    /// 打字机：已显示的字符数（浮点便于按帧推进）
    en_shown: f32,
    zh_shown: f32,
    /// 中文是说话过程中翻的半句（灰色斜体）
    zh_provisional: bool,
    /// 定稿译文已开始到达：之后晚到的临时中文一律忽略
    zh_final_started: bool,
}

/// 中间英文只增不缩：新快照是延长就用新的；改写了但更长也用新的（已显示部分原地换字，不重打）；
/// 变短或等长的改写一律不采纳（识别器回退的字不该在屏上消失），定稿到了再整句替换。
fn merge_partial_en(shown: &str, new: &str) -> Option<String> {
    if new == shown {
        return None;
    }
    if new.starts_with(shown) || new.chars().count() > shown.chars().count() {
        return Some(new.to_string());
    }
    None
}

fn new_line(id: SegId, at: DateTime<Local>, en: String, partial: bool) -> Line {
    Line { id, at, en, zh: None, partial, en_shown: 0.0, zh_shown: 0.0, zh_provisional: false, zh_final_started: false }
}

/// 中间英文按话轮 id 找行：同 id 的中间行按只增不缩更新；没有就新开一行。
/// 上一句的定稿可能比这一句的中间结果晚到（识别线程让中间结果插队），所以上一句的中间行要留着等它的定稿。
/// 空文本 = 该话轮识别为空（噪声），删掉它的中间行。
fn apply_english_partial(lines: &mut VecDeque<Line>, id: SegId, text: String) {
    if text.is_empty() {
        if let Some(pos) = lines.iter().position(|l| l.partial && l.id == id) {
            lines.remove(pos);
        }
        return;
    }
    if let Some(l) = lines.iter_mut().find(|l| l.partial && l.id == id) {
        // 只增不缩：变短的快照不采纳；采纳的快照已显示部分原地替换不重打，只有新增部分打字推进
        if let Some(new) = merge_partial_en(&l.en, &text) {
            l.en_shown = l.en_shown.min(new.chars().count() as f32);
            l.en = new;
        }
        return;
    }
    lines.push_back(new_line(id, Local::now(), text, true));
}

/// 定稿英文：同 id 的中间行原地转正（已显示部分原地换字不重打）；没有中间行就插在 id 比它大的行前面，保持话轮顺序。
fn apply_english_final(lines: &mut VecDeque<Line>, id: SegId, at: DateTime<Local>, text: String) {
    if let Some(l) = lines.iter_mut().find(|l| l.partial && l.id == id) {
        l.at = at;
        l.en_shown = l.en_shown.min(text.chars().count() as f32);
        l.en = text;
        l.partial = false;
        return;
    }
    let line = new_line(id, at, text, false);
    match lines.iter().position(|l| l.id > id) {
        Some(pos) => lines.insert(pos, line),
        None => lines.push_back(line),
    }
}

impl Line {
    /// 换中文：已显示的部分原地替换、不重打（擦掉重打会打断连续阅读），只有超出已显示长度的新增部分用打字机推进
    fn set_zh(&mut self, text: String, provisional: bool) {
        self.zh_shown = self.zh_shown.min(text.chars().count() as f32);
        self.zh = Some(text);
        self.zh_provisional = provisional;
    }

    /// 中文"只增不缩"：新文本比已显示的短（流式还没追上、临时译文被截短、识别抖动只吐一个词）就不动，
    /// 只在更长或等长改写时才替换（公共前缀不重打）。定稿完成的整句不走这里，直接 set_zh。返回是否替换
    fn set_zh_if_grown(&mut self, text: String, provisional: bool) -> bool {
        if let Some(old) = self.zh.as_deref() {
            if old == text || text.chars().count() < old.chars().count() {
                return false;
            }
        }
        self.set_zh(text, provisional);
        true
    }

    /// 临时中文：定稿流开始后不再采纳；采纳时也遵守"只有变化才动"。返回是否采纳
    fn apply_provisional(&mut self, text: String) -> bool {
        if self.zh_final_started {
            return false;
        }
        self.set_zh_if_grown(text, true)
    }

    fn advance(&mut self, dt: f32, cps: f32) -> bool {
        let en_len = self.en.chars().count() as f32;
        let zh_len = self.zh.as_ref().map_or(0.0, |z| z.chars().count() as f32);
        if cps <= 0.0 {
            self.en_shown = en_len;
            self.zh_shown = zh_len;
            return false;
        }
        let step = cps * dt;
        let mut moving = false;
        if self.en_shown < en_len {
            self.en_shown = (self.en_shown + step).min(en_len);
            moving = true;
        }
        if self.zh_shown < zh_len {
            self.zh_shown = (self.zh_shown + step).min(zh_len);
            moving = true;
        }
        moving
    }
}

struct HistEntry {
    path: PathBuf,
    name: String,
    size: u64,
    modified: DateTime<Local>,
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn head(s: &str, n: f32) -> String {
    s.chars().take(n as usize).collect()
}

pub struct CaptionApp {
    rx: Receiver<UiMsg>,
    cfg: Config,
    lines: VecDeque<Line>,
    status: HashMap<Stage, StageState>,
    paused: Arc<AtomicBool>,
    passthrough: bool,
    transcript: Transcript,
    file: Option<TranscriptFile>,
    _hotkeys: Option<GlobalHotKeyManager>,
    hotkey_toggle_id: u32,
    last_cfg_save: Instant,
    last_resize: Instant,
    last_frame: Instant,
    /// 托盘图标（穿透后鼠标点不到窗口，靠它兜底）
    tray: Option<TrayIcon>,
    /// 正在用鼠标拖边缘调大小
    resizing: bool,
    /// 待机迷你条模式
    mini: bool,
    mini_since: Instant,
    /// 最近一次有字幕活动（中间/最终英文、译文）的时间
    last_activity: Instant,
    // 设置窗
    tx_cfg: Vec<Sender<TranslatorConfig>>,
    show_settings: bool,
    draft: Config,
    models: Vec<String>,
    devices: Vec<String>,
    settings_note: String,
    settings_tab: usize,
    /// 下一帧把设置窗抬到前台（每次打开/再次打开都要）
    settings_raise: bool,
    hist: Vec<HistEntry>,
    hist_sel: Option<usize>,
    hist_preview: String,
    hist_confirm: Option<usize>,
    hist_note: String,
    test_rx: Option<Receiver<Result<String, String>>>,
    test_msg: String,
}

impl CaptionApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rx: Receiver<UiMsg>,
        cfg: Config,
        paused: Arc<AtomicBool>,
        tx_cfg: Vec<Sender<TranslatorConfig>>,
    ) -> Self {
        if let Err(e) = install_fonts(&cc.egui_ctx, &cfg.ui.font_path) {
            tracing::warn!("加载中文字体失败：{e}");
        }
        let parse = |s: &str, fallback: HotKey| -> HotKey {
            match s.parse::<HotKey>() {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("热键 {s:?} 解析失败（{e}），用默认");
                    fallback
                }
            }
        };
        let hk_clickable = parse(&cfg.ui.hotkey_clickable, HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyC));
        let hk_toggle = parse(&cfg.ui.hotkey_passthrough, HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyP));
        let hotkey_toggle_id = hk_toggle.id();
        let hotkeys = match GlobalHotKeyManager::new() {
            Ok(m) => {
                let mut ok = true;
                for (hk, name) in [(hk_clickable, cfg.ui.hotkey_clickable.as_str()), (hk_toggle, cfg.ui.hotkey_passthrough.as_str())] {
                    if let Err(e) = m.register(hk) {
                        tracing::warn!("全局热键 {name} 注册失败（多半被别的程序占了，可在设置里换一个）：{e}");
                        ok = false;
                    }
                }
                if ok {
                    tracing::info!("全局热键已注册：{} 切回可点击，{} 切换穿透", cfg.ui.hotkey_clickable, cfg.ui.hotkey_passthrough);
                }
                Some(m)
            }
            Err(e) => {
                tracing::warn!("全局热键管理器创建失败：{e}");
                None
            }
        };
        let file = match TranscriptFile::create(&Config::resolve(&cfg.paths.transcripts_dir).to_string_lossy()) {
            Ok(f) => Some(f),
            Err(e) => {
                tracing::warn!("建记录文件失败：{e}");
                None
            }
        };
        let draft = cfg.clone();
        let tray = match build_tray() {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!("托盘图标创建失败：{e}");
                None
            }
        };
        let mut app = Self {
            rx,
            cfg,
            lines: VecDeque::new(),
            status: HashMap::new(),
            paused,
            passthrough: false,
            transcript: Transcript::default(),
            file,
            _hotkeys: hotkeys,
            hotkey_toggle_id,
            last_cfg_save: Instant::now(),
            last_resize: Instant::now(),
            last_frame: Instant::now(),
            tray,
            resizing: false,
            mini: false,
            mini_since: Instant::now(),
            last_activity: Instant::now(),
            tx_cfg,
            show_settings: false,
            draft,
            models: Vec::new(),
            devices: Vec::new(),
            settings_note: String::new(),
            settings_tab: 0,
            settings_raise: false,
            hist: Vec::new(),
            hist_sel: None,
            hist_preview: String::new(),
            hist_confirm: None,
            hist_note: String::new(),
            test_rx: None,
            test_msg: String::new(),
        };
        // 调试钩子：LC_OPEN_SETTINGS=1 启动即打开设置窗（截图脚本用）
        if std::env::var_os("LC_OPEN_SETTINGS").is_some() {
            app.open_settings();
        }
        app
    }

    fn drain(&mut self) {
        let keep = self.cfg.ui.lines.max(1);
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::EnglishPartial { id, text } => {
                    self.last_activity = Instant::now();
                    apply_english_partial(&mut self.lines, id, text);
                }
                UiMsg::English { id, at, text } => {
                    self.last_activity = Instant::now();
                    apply_english_final(&mut self.lines, id, at, text.clone());
                    self.transcript.push_en(id, at, &text);
                }
                UiMsg::ChineseProvisional { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.apply_provisional(text);
                    }
                }
                UiMsg::ChinesePartial { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.zh_final_started = true;
                        // 定稿流刚开始只有一两个字：在追上已显示的灰色中文之前不动它，分歧或延长时才替换
                        l.set_zh_if_grown(text, false);
                    }
                }
                UiMsg::Chinese { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.zh_final_started = true;
                        l.set_zh(text.clone(), false);
                    }
                    if let Some(line) = self.transcript.push_zh(id, &text) {
                        if let Some(f) = self.file.as_mut() {
                            f.append(&line);
                        }
                    }
                }
                UiMsg::Status { stage, state } => {
                    self.status.insert(stage, state);
                }
            }
            // 中间结果那一行不计入"最近几段"
            let mut n_final = self.lines.iter().filter(|l| !l.partial).count();
            while n_final > keep {
                if let Some(pos) = self.lines.iter().position(|l| !l.partial) {
                    self.lines.remove(pos);
                }
                n_final -= 1;
            }
        }
    }

    fn status_color_and_text(&self) -> (Color32, String) {
        let mut worst = (Color32::from_rgb(70, 200, 90), String::new());
        for (stage, st) in &self.status {
            let name = match stage {
                Stage::Capture => "采集",
                Stage::Asr => "识别",
                Stage::Translate => "翻译",
            };
            match st {
                StageState::Err(m) => return (Color32::from_rgb(230, 70, 70), format!("{name}：{m}")),
                StageState::Warn(m) => worst = (Color32::from_rgb(240, 190, 60), format!("{name}：{m}")),
                StageState::Ok => {}
            }
        }
        worst
    }

    /// 收起成待机迷你条 / 展开成完整字幕条
    fn set_mini(&mut self, on: bool) {
        if self.mini == on {
            return;
        }
        self.mini = on;
        self.mini_since = Instant::now();
        if !on {
            self.last_activity = Instant::now();
        }
        // 让 fit_height 立刻执行
        self.last_resize = Instant::now() - Duration::from_secs(1);
        tracing::info!("mini={on}");
    }

    /// 窗口高度随内容伸缩：下边缘固定，向上长；范围 [height, max_height]
    fn fit_height(&mut self, ctx: &egui::Context, inner: egui::Rect, content_h: f32) {
        if self.resizing || self.last_resize.elapsed() < Duration::from_millis(150) {
            return;
        }
        let cur = inner.height();
        let (want_w, want) = if self.mini {
            (MINI_W, MINI_H)
        } else {
            let min_h = self.cfg.ui.height as f32;
            let max_h = self.cfg.ui.max_height.max(self.cfg.ui.height) as f32;
            (self.cfg.ui.width as f32, (content_h + MARGIN * 2.0 + 4.0).clamp(min_h, max_h))
        };
        if (want - cur).abs() < 2.0 && (want_w - inner.width()).abs() < 2.0 {
            return;
        }
        let delta = want - cur;
        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(outer.min.x, (outer.min.y - delta).max(0.0))));
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(want_w, want)));
        self.last_resize = Instant::now();
    }

    fn maybe_save_window_rect(&mut self, ctx: &egui::Context) {
        if self.last_cfg_save.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_cfg_save = Instant::now();
        // 迷你条期间、以及切换后两秒内窗口还没变回来时，都不能把尺寸记进配置
        if self.mini || self.mini_since.elapsed() < Duration::from_secs(2) {
            return;
        }
        let Some(r) = ctx.input(|i| i.viewport().outer_rect) else { return };
        if (r.width() - MINI_W).abs() < 3.0 {
            return;
        }
        let (x, y, w) = (r.min.x as i32, r.min.y as i32, r.width() as u32);
        if (x, y, w) != (self.cfg.ui.x, self.cfg.ui.y, self.cfg.ui.width) {
            self.cfg.ui.x = x;
            self.cfg.ui.y = y;
            self.cfg.ui.width = w;
            if let Err(e) = self.cfg.save() {
                tracing::warn!("保存配置失败：{e}");
            }
        }
    }

    fn open_settings(&mut self) {
        self.draft = self.cfg.clone();
        self.settings_note.clear();
        self.test_msg.clear();
        self.models = list_models(&self.cfg.asr.models_dir, &self.cfg.asr.model_path);
        self.devices = crate::audio::capture::list_render_devices();
        self.scan_history();
        // 调试钩子：LC_SETTINGS_TAB=5 之类，让截图脚本直接落到指定页
        if let Some(t) = std::env::var("LC_SETTINGS_TAB").ok().and_then(|v| v.parse::<usize>().ok()) {
            self.settings_tab = t.min(5);
        }
        self.show_settings = true;
        self.settings_raise = true;
    }

    /// "打开记录"：进设置窗的记录页，选中当前会话文件并预览（外部程序打开在页内另有按钮）
    fn open_records(&mut self) {
        if let Some(f) = self.file.as_mut() {
            f.ensure_created();
        }
        self.open_settings();
        self.settings_tab = 5;
        if let Some(cur) = self.file.as_ref().map(|f| f.path.clone()) {
            if let Some(i) = self.hist.iter().position(|h| same_file(&h.path, &cur)) {
                self.load_preview(i);
            }
        }
    }

    /// 草稿里有没有必须重启才生效的改动（识别 / 切句 / 热键）
    fn needs_restart(&self) -> bool {
        self.draft.asr != self.cfg.asr
            || self.draft.audio != self.cfg.audio
            || self.draft.ui.hotkey_clickable != self.cfg.ui.hotkey_clickable
            || self.draft.ui.hotkey_passthrough != self.cfg.ui.hotkey_passthrough
    }

    fn apply_settings(&mut self) -> bool {
        let restart = self.needs_restart();
        self.cfg = self.draft.clone();
        match self.cfg.save() {
            Ok(()) => {
                for tx in &self.tx_cfg {
                    let _ = tx.send(TranslatorConfig::from(&self.cfg.translate));
                }
                self.settings_note = if restart {
                    "已保存。识别 / 切句 / 热键的改动要重启后生效。".into()
                } else {
                    "已保存并应用。".into()
                };
            }
            Err(e) => self.settings_note = format!("保存失败：{e}"),
        }
        restart
    }

    /// 重启：起一个延迟 2 秒的启动器再退出自己（等本进程释放热键与托盘）
    fn restart_app(&self, ctx: &egui::Context) {
        if let Ok(exe) = std::env::current_exe() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let cmd = format!("ping -n 3 127.0.0.1 >nul & start \"\" /D \"{}\" \"{}\"", cwd.display(), exe.display());
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("cmd").args(["/C", &cmd]).creation_flags(0x0800_0000).spawn();
        }
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    }

    fn start_translate_test(&mut self) {
        let cfg = TranslatorConfig::from(&self.draft.translate);
        let (tx, rx) = bounded(1);
        self.test_rx = Some(rx);
        self.test_msg = "测试中…（Claude 首句含冷启动，约 10 秒）".into();
        std::thread::spawn(move || {
            let r = crate::translate::test_translate(&cfg, "Good morning everyone, let's get started with today's meeting.")
                .map(|(zh, d)| format!("{zh}　（{:.1} 秒）", d.as_secs_f32()))
                .map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
    }

    fn settings_window(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(rx) = &self.test_rx {
            if let Ok(r) = rx.try_recv() {
                self.test_msg = match r {
                    Ok(s) => format!("✓ {s}"),
                    Err(e) => format!("✗ {e}"),
                };
                self.test_rx = None;
            }
        }
        ui.style_mut().spacing.slider_width = 260.0;
        ui.style_mut().spacing.item_spacing = egui::vec2(10.0, 8.0);

        egui::Panel::left("settings_tabs").resizable(false).exact_size(150.0).show(ui, |ui| {
            ui.add_space(10.0);
            let tabs = [("翻译", false), ("识别", true), ("切句", true), ("界面", false), ("热键", true), ("记录", false)];
            for (i, (name, restart)) in tabs.iter().enumerate() {
                let text = if *restart { format!("{name}   ·重启") } else { name.to_string() };
                let sel = self.settings_tab == i;
                if ui.add_sized([130.0, 34.0], egui::Button::selectable(sel, RichText::new(text).size(15.0))).clicked() {
                    self.settings_tab = i;
                }
            }
            ui.add_space(14.0);
            ui.label(RichText::new("·重启 = 改动要重启后生效").size(11.0).color(Color32::GRAY));
        });

        egui::Panel::bottom("settings_bar").show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let restart = self.needs_restart();
                if ui.add_sized([120.0, 32.0], egui::Button::new(RichText::new("保存并应用").strong())).clicked() {
                    self.apply_settings();
                }
                if restart && ui.add_sized([120.0, 32.0], egui::Button::new("保存并重启")).clicked() {
                    self.apply_settings();
                    self.restart_app(ctx);
                }
                if ui.add_sized([80.0, 32.0], egui::Button::new("关闭")).clicked() {
                    self.show_settings = false;
                }
                ui.add_space(8.0);
                ui.label(RichText::new(&self.settings_note).color(Color32::from_rgb(70, 150, 70)));
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(6.0);
                match self.settings_tab {
                    0 => self.tab_translate(ui),
                    1 => self.tab_asr(ui),
                    2 => self.tab_audio(ui),
                    3 => self.tab_ui(ui),
                    4 => self.tab_hotkeys(ui),
                    _ => self.tab_history(ui),
                }
            });
        });
    }

    fn transcripts_dir(&self) -> PathBuf {
        Config::resolve(&self.cfg.paths.transcripts_dir)
    }

    fn scan_history(&mut self) {
        let dir = self.transcripts_dir();
        let mut v: Vec<HistEntry> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().map_or(false, |x| x == "md"))
                    .filter_map(|p| {
                        let m = std::fs::metadata(&p).ok()?;
                        let modified: DateTime<Local> = m.modified().ok()?.into();
                        Some(HistEntry { name: p.file_name()?.to_string_lossy().into_owned(), path: p, size: m.len(), modified })
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by(|a, b| b.name.cmp(&a.name));
        self.hist = v;
        self.hist_confirm = None;
        if let Some(i) = self.hist_sel {
            if i >= self.hist.len() {
                self.hist_sel = None;
                self.hist_preview.clear();
            }
        }
    }

    fn load_preview(&mut self, i: usize) {
        self.hist_sel = Some(i);
        let p = &self.hist[i].path;
        self.hist_preview = match std::fs::read(p) {
            Ok(bytes) => {
                let cut = bytes.len().min(200_000);
                let mut s = String::from_utf8_lossy(&bytes[..cut]).into_owned();
                if bytes.len() > cut {
                    s.push_str("\n…（文件较大，只显示前 200 KB）");
                }
                s
            }
            Err(e) => format!("读取失败：{e}"),
        };
    }

    fn is_current_file(&self, p: &std::path::Path) -> bool {
        self.file.as_ref().map_or(false, |f| same_file(&f.path, p))
    }

    fn tab_history(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "历史记录", "每次启动一个文件，逐句写入；当前会话正在写的文件不能删。");
        let dir = self.transcripts_dir();
        ui.horizontal(|ui| {
            if ui.button("刷新").clicked() {
                self.scan_history();
            }
            if ui.button("打开文件夹").clicked() {
                let _ = std::fs::create_dir_all(&dir);
                open_path(&dir);
            }
            let others = self.hist.iter().filter(|h| !self.is_current_file(&h.path)).count();
            if others > 0 {
                if self.hist_confirm == Some(usize::MAX) {
                    if ui.button(RichText::new(format!("确认清空 {others} 个文件？")).color(Color32::from_rgb(200, 80, 80))).clicked() {
                        let mut n = 0;
                        for h in &self.hist {
                            if !self.is_current_file(&h.path) && std::fs::remove_file(&h.path).is_ok() {
                                n += 1;
                            }
                        }
                        self.hist_note = format!("已删除 {n} 个文件");
                        self.hist_sel = None;
                        self.hist_preview.clear();
                        self.scan_history();
                    }
                    if ui.button("取消").clicked() {
                        self.hist_confirm = None;
                    }
                } else if ui.button("清空历史…").clicked() {
                    self.hist_confirm = Some(usize::MAX);
                }
            }
            ui.label(RichText::new(&self.hist_note).size(12.0).color(Color32::GRAY));
        });
        ui.label(RichText::new(dir.to_string_lossy()).size(11.0).color(Color32::GRAY));
        ui.add_space(6.0);
        if self.hist.is_empty() {
            ui.label("还没有记录文件。");
            return;
        }
        let total_h = ui.available_height().max(200.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(300.0);
                egui::ScrollArea::vertical().id_salt("hist_list").max_height(total_h).show(ui, |ui| {
                    let mut click: Option<usize> = None;
                    let mut delete: Option<usize> = None;
                    let mut confirm: Option<usize> = None;
                    for (i, h) in self.hist.iter().enumerate() {
                        let current = self.is_current_file(&h.path);
                        let sel = self.hist_sel == Some(i);
                        let title = format!("{}{}", h.name.trim_end_matches(".md"), if current { "  ·当前" } else { "" });
                        let sub = format!("{}  {:.1} KB", h.modified.format("%m-%d %H:%M"), h.size as f32 / 1024.0);
                        ui.horizontal(|ui| {
                            let r = ui.add_sized([210.0, 40.0], egui::Button::selectable(sel, RichText::new(format!("{title}\n{sub}")).size(12.5)));
                            if r.clicked() {
                                click = Some(i);
                            }
                            if !current {
                                if self.hist_confirm == Some(i) {
                                    if ui.button(RichText::new("确认").color(Color32::from_rgb(200, 80, 80))).clicked() {
                                        delete = Some(i);
                                    }
                                } else if ui.button("删除").clicked() {
                                    confirm = Some(i);
                                }
                            }
                        });
                    }
                    if let Some(i) = delete {
                        let name = self.hist[i].name.clone();
                        self.hist_note = match std::fs::remove_file(&self.hist[i].path) {
                            Ok(()) => format!("已删除 {name}"),
                            Err(e) => format!("删除失败：{e}"),
                        };
                        self.hist_sel = None;
                        self.hist_preview.clear();
                        self.scan_history();
                    } else if let Some(i) = confirm {
                        self.load_preview(i);
                        self.hist_confirm = Some(i);
                    } else if let Some(i) = click {
                        self.load_preview(i);
                        self.hist_confirm = None;
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                match self.hist_sel {
                    Some(i) if i < self.hist.len() => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&self.hist[i].name).strong());
                            if ui.button("用外部程序打开").clicked() {
                                open_path(&self.hist[i].path);
                            }
                        });
                        egui::ScrollArea::vertical().id_salt("hist_preview").max_height(total_h - 30.0).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&self.hist_preview).size(13.0)).wrap());
                        });
                    }
                    _ => {
                        ui.label(RichText::new("在左侧选一个文件查看内容").color(Color32::GRAY));
                    }
                }
            });
        });
    }

    fn section(ui: &mut egui::Ui, title: &str, sub: &str) {
        ui.label(RichText::new(title).size(19.0).strong());
        if !sub.is_empty() {
            ui.label(RichText::new(sub).size(12.0).color(Color32::GRAY));
        }
        ui.add_space(8.0);
    }

    fn grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
        egui::Grid::new(id).num_columns(2).spacing([18.0, 12.0]).min_col_width(120.0).show(ui, add);
    }

    fn tab_translate(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "翻译后端", "字幕的中文由哪个服务翻译；点“保存并应用”立即切换，不用重启。");
        let d = &mut self.draft;
        let backend_name = |b: &str| match b {
            "codex" => "Codex CLI（OpenAI 订阅）",
            "openai" => "OpenAI 兼容接口（DeepSeek / 通义千问 / 网关 / Ollama）",
            _ => "Claude Code CLI（Anthropic 订阅）",
        };
        Self::grid(ui, "g_tr", |ui| {
            ui.label("后端");
            egui::ComboBox::from_id_salt("backend")
                .width(420.0)
                .selected_text(backend_name(&d.translate.backend))
                .show_ui(ui, |ui| {
                    for b in ["claude", "codex", "openai"] {
                        ui.selectable_value(&mut d.translate.backend, b.to_string(), backend_name(b));
                    }
                });
            ui.end_row();
            match d.translate.backend.as_str() {
                "codex" => {
                    ui.label("codex 命令");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.codex_bin).desired_width(420.0));
                    ui.end_row();
                    ui.label("模型");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.codex_model).desired_width(220.0).hint_text("留空用 codex 默认"));
                    ui.end_row();
                }
                "openai" => {
                    ui.label("预设");
                    ui.horizontal(|ui| {
                        if ui.button("DeepSeek").clicked() {
                            d.translate.openai_base_url = "https://api.deepseek.com".into();
                            d.translate.openai_model = "deepseek-chat".into();
                        }
                        if ui.button("通义千问").clicked() {
                            d.translate.openai_base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1".into();
                            d.translate.openai_model = "qwen-turbo".into();
                        }
                        if ui.button("自建网关").clicked() {
                            d.translate.openai_base_url = "https://gateway.example.com/v1".into();
                            d.translate.openai_model = "deepseek-v4-flash".into();
                        }
                        if ui.button("Ollama 本地").clicked() {
                            d.translate.openai_base_url = "http://127.0.0.1:11434/v1".into();
                            d.translate.openai_model = "qwen2.5:7b".into();
                            d.translate.openai_api_key = "ollama".into();
                        }
                    });
                    ui.end_row();
                    ui.label("Base URL");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.openai_base_url).desired_width(420.0));
                    ui.end_row();
                    ui.label("API Key");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.openai_api_key).password(true).desired_width(420.0));
                    ui.end_row();
                    ui.label("模型");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.openai_model).desired_width(220.0));
                    ui.end_row();
                }
                _ => {
                    ui.label("claude 路径");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.claude_bin).desired_width(420.0).hint_text("写 claude 即按 PATH 自动找"));
                    ui.end_row();
                    ui.label("模型");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.claude_model).desired_width(220.0).hint_text("sonnet / haiku / opus"));
                    ui.end_row();
                }
            }
            ui.label("单句超时");
            ui.add(egui::Slider::new(&mut d.translate.timeout_secs, 3..=60).suffix(" 秒"));
            ui.end_row();
            ui.label("临时翻译");
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.translate.partial_translate, "说话过程中先出灰色临时中文");
                ui.label(RichText::new("英文每更新一次翻一次，一次只飞一个；定稿后整句重翻").small().weak());
            });
            ui.end_row();
            ui.label("上下文句数");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut d.translate.context_sentences).range(0..=6));
                ui.label(RichText::new("翻译时带最近几句英中对照，0 不带").small().weak());
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        let hint = match d.translate.backend.as_str() {
            "codex" => "每句起一个 codex exec 进程，延迟通常 3 秒以上；适合只想用订阅、不在乎速度的场景。",
            "openai" => "请求发到 {Base URL}/chat/completions，流式返回。Key 只存在本机 config.toml。走公网服务意味着会议文本会出本机。",
            _ => "常驻 claude 子进程，每句约 1.5 秒；首句冷启动约 10 秒，已在启动时预热。",
        };
        ui.label(RichText::new(hint).size(12.0).color(Color32::GRAY));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let busy = self.test_rx.is_some();
            if ui.add_enabled(!busy, egui::Button::new("测试翻译")).clicked() {
                self.start_translate_test();
            }
            ui.label(RichText::new(&self.test_msg).size(13.0));
        });
    }

    fn tab_asr(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "语音识别（本机 GPU）", "改动要重启后生效；底部有“保存并重启”。");
        let models = self.models.clone();
        let d = &mut self.draft;
        let pretty = |p: &str| -> String {
            let name = std::path::Path::new(p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(p.to_string());
            match std::fs::metadata(p) {
                Ok(m) => format!("{name}  ({} MB)", m.len() / 1_000_000),
                Err(_) => format!("{name}  (找不到文件)"),
            }
        };
        Self::grid(ui, "g_asr", |ui| {
            ui.label("模型");
            egui::ComboBox::from_id_salt("asr_model")
                .width(420.0)
                .selected_text(pretty(&d.asr.model_path))
                .show_ui(ui, |ui| {
                    for m in &models {
                        ui.selectable_value(&mut d.asr.model_path, m.clone(), pretty(m));
                    }
                });
            ui.end_row();
            ui.label("中间识别模型");
            let cur_p = d.asr.partial_model_path.clone();
            egui::ComboBox::from_id_salt("asr_partial_model")
                .width(420.0)
                .selected_text(if cur_p.trim().is_empty() { "同主模型".to_string() } else { pretty(&cur_p) })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.asr.partial_model_path, String::new(), "同主模型");
                    for m in &models {
                        ui.selectable_value(&mut d.asr.partial_model_path, m.clone(), pretty(m));
                    }
                });
            ui.end_row();
            ui.label("模型目录");
            ui.add(egui::TextEdit::singleline(&mut d.asr.models_dir).desired_width(420.0));
            ui.end_row();
            ui.label("flash attention");
            ui.checkbox(&mut d.asr.flash_attn, "开启（本机 Arc 核显实测反而慢 2.5 倍，默认关）");
            ui.end_row();
            ui.label("audio_ctx 下限");
            ui.add(egui::Slider::new(&mut d.asr.min_audio_ctx, 128..=1500));
            ui.end_row();
            ui.label("CPU 线程");
            ui.add(egui::Slider::new(&mut d.asr.threads, 1..=16));
            ui.end_row();
        });
        ui.add_space(6.0);
        if ui.button("重新扫描模型目录").clicked() {
            self.models = list_models(&self.draft.asr.models_dir, &self.draft.asr.model_path);
        }
        ui.add_space(6.0);
        ui.label(RichText::new("主模型出最终句：large-v3-turbo 最准（约 0.35 秒一句）。中间识别模型只管说话过程中的临时英文，越小越快：small.en 约 0.2 秒，最终句仍由主模型定稿。").size(12.0).color(Color32::GRAY));
    }

    fn tab_audio(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "采集、切句与逐字上屏", "改动要重启后生效。");
        let devices = self.devices.clone();
        let d = &mut self.draft;
        Self::grid(ui, "g_audio", |ui| {
            ui.label("采集哪个播放设备");
            egui::ComboBox::from_id_salt("audio_device")
                .width(420.0)
                .selected_text(if d.audio.device.trim().is_empty() { "跟随系统默认播放设备（推荐）".to_string() } else { d.audio.device.clone() })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.audio.device, String::new(), "跟随系统默认播放设备（推荐）");
                    for name in &devices {
                        ui.selectable_value(&mut d.audio.device, name.clone(), name);
                    }
                });
            ui.end_row();
            ui.label("能量门限");
            ui.add(egui::Slider::new(&mut d.audio.energy_threshold, 0.001..=0.1).logarithmic(true));
            ui.end_row();
            ui.label("停顿多久算一句");
            ui.add(egui::Slider::new(&mut d.audio.silence_ms, 200..=1500).suffix(" ms"));
            ui.end_row();
            ui.label("逐字上屏间隔");
            ui.add(egui::Slider::new(&mut d.audio.partial_every_ms, 0..=4000).suffix(" ms"));
            ui.end_row();
            ui.label("单段最长");
            ui.add(egui::Slider::new(&mut d.audio.max_segment_ms, 3000..=15000).suffix(" ms"));
            ui.end_row();
            ui.label("单段最短");
            ui.add(egui::Slider::new(&mut d.audio.min_segment_ms, 300..=3000).suffix(" ms"));
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(RichText::new("视频/会议声音走哪个设备就采哪个：选“跟随默认”时系统默认播放设备切换后会自动跟过去；声音走 HDMI 显示器或蓝牙耳机而默认设备不是它时，在这里指定。能量门限越小越灵敏；逐字上屏间隔 0 = 关闭中间结果。").size(12.0).color(Color32::GRAY));
    }

    fn tab_ui(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "字幕窗外观", "立即生效。");
        let d = &mut self.draft;
        Self::grid(ui, "g_ui", |ui| {
            ui.label("字号");
            ui.add(egui::Slider::new(&mut d.ui.font_size, 12.0..=60.0).fixed_decimals(0));
            ui.end_row();
            ui.label("不透明度");
            ui.add(egui::Slider::new(&mut d.ui.opacity, 0.1..=1.0).fixed_decimals(2));
            ui.end_row();
            ui.label("同时显示段数");
            ui.add(egui::Slider::new(&mut d.ui.lines, 1..=8));
            ui.end_row();
            ui.label("打字机速度");
            ui.add(egui::Slider::new(&mut d.ui.typewriter_cps, 0.0..=300.0).fixed_decimals(0).suffix(" 字/秒"));
            ui.end_row();
            ui.label("最小高度");
            ui.add(egui::Slider::new(&mut d.ui.height, 60..=600).suffix(" px"));
            ui.end_row();
            ui.label("最大高度");
            ui.add(egui::Slider::new(&mut d.ui.max_height, 120..=1200).suffix(" px"));
            ui.end_row();
            ui.label("自动收起");
            ui.add(egui::Slider::new(&mut d.ui.auto_mini_secs, 0..=300).suffix(" 秒无语音").text("（0 = 不自动收起）"));
            ui.end_row();
            ui.label("自动展开");
            ui.checkbox(&mut d.ui.auto_restore, "有新语音时自动从迷你条展开");
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(RichText::new("打字机速度 0 = 整句直出。窗口位置拖动后自动记住；拖边缘可改宽高。").size(12.0).color(Color32::GRAY));
    }

    fn tab_hotkeys(&mut self, ui: &mut egui::Ui) {
        Self::section(ui, "全局热键", "改动要重启后生效。被别的程序占用时换一个组合。");
        let d = &mut self.draft;
        let valid = |s: &str| -> (bool, String) {
            match s.parse::<HotKey>() {
                Ok(_) => (true, "✓".into()),
                Err(e) => (false, format!("✗ {e}")),
            }
        };
        Self::grid(ui, "g_hk", |ui| {
            for (label, field) in [("切回可点击", &mut d.ui.hotkey_clickable), ("切换穿透", &mut d.ui.hotkey_passthrough)] {
                ui.label(label);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(field).desired_width(220.0));
                    let (ok, msg) = valid(field);
                    ui.label(RichText::new(msg).color(if ok { Color32::from_rgb(70, 150, 70) } else { Color32::from_rgb(200, 80, 80) }));
                });
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("写法：Ctrl+Alt+C、Ctrl+Shift+F9、Alt+Shift+Space。托盘图标的菜单不依赖热键，穿透后也能用。").size(12.0).color(Color32::GRAY));
    }
}

/// 32×32 的图标：深色圆角底 + 两条白色字幕线，程序内生成，不依赖外部文件
fn tray_icon_image() -> tray_icon::Icon {
    let (w, h) = (32u32, 32u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // 圆角矩形（半径 7）
            let (cx, cy) = (fx.clamp(7.0, 25.0), fy.clamp(7.0, 25.0));
            let inside = (fx - cx).powi(2) + (fy - cy).powi(2) <= 49.0;
            if !inside {
                continue;
            }
            let bar1 = (9..=13).contains(&y) && (6..=25).contains(&x);
            let bar2 = (18..=22).contains(&y) && (6..=19).contains(&x);
            let (r, g, b) = if bar1 || bar2 { (255, 255, 255) } else { (30, 34, 48) };
            rgba[i] = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = 255;
        }
    }
    tray_icon::Icon::from_rgba(rgba, w, h).expect("icon")
}

/// 托盘菜单项 id（TrackPopupMenu 直接返回）
const MENU_CLICKABLE: u32 = 1;
const MENU_PASSTHROUGH: u32 = 2;
const MENU_PAUSE: u32 = 3;
const MENU_SETTINGS: u32 = 4;
const MENU_TRANSCRIPT: u32 = 5;
const MENU_MINI: u32 = 6;
const MENU_QUIT: u32 = 9;

/// 自己弹 Windows 原生托盘菜单：`TPM_RETURNCMD` 让 TrackPopupMenu 同步返回所选项，
/// 不经过 muda 那条 "WM_COMMAND → 窗口子类 → 全局通道" 的转发（实测在 winit 消息循环里时有时无）。
/// 必须在 UI 线程调用；调用期间会阻塞到菜单关闭。
fn show_native_tray_menu(hwnd: isize, x: i32, y: i32, paused: bool, passthrough: bool, mini: bool) -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, PostMessageW, SetForegroundWindow, TrackPopupMenu, MF_CHECKED,
        MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
    };
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let items: [(u32, String, bool); 8] = [
        (MENU_CLICKABLE, "切回可点击".into(), !passthrough),
        (MENU_PASSTHROUGH, "鼠标穿透".into(), passthrough),
        (0, String::new(), false),
        (MENU_PAUSE, (if paused { "继续识别" } else { "暂停识别" }).into(), paused),
        (MENU_MINI, (if mini { "展开字幕条" } else { "收起为迷你条" }).into(), mini),
        (MENU_SETTINGS, "设置…".into(), false),
        (MENU_TRANSCRIPT, "打开记录文件".into(), false),
        (MENU_QUIT, "退出 live-caption".into(), false),
    ];
    unsafe {
        let hmenu = CreatePopupMenu();
        if hmenu.is_null() {
            return None;
        }
        // 文本缓冲要活到 TrackPopupMenu 之后（AppendMenuW 会拷贝，但保险起见一起持有）
        let mut bufs: Vec<Vec<u16>> = Vec::new();
        for (id, text, checked) in items.iter() {
            if *id == 0 {
                AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
            } else {
                let w = wide(text);
                let flags = MF_STRING | if *checked { MF_CHECKED } else { 0 };
                AppendMenuW(hmenu, flags, *id as usize, w.as_ptr());
                bufs.push(w);
            }
        }
        let h = hwnd as *mut core::ffi::c_void;
        // 托盘菜单惯例：先把自己设为前台，否则点菜单外面不会自动收起、键盘也到不了菜单。
        // 进程没有前台权限时 SetForegroundWindow 会被拒，按惯例先把输入队列挂到当前前台线程上再设。
        {
            use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
            use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
            let fg = GetForegroundWindow();
            let fg_tid = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) };
            let my_tid = GetCurrentThreadId();
            let attached = fg_tid != 0 && fg_tid != my_tid && AttachThreadInput(fg_tid, my_tid, 1) != 0;
            SetForegroundWindow(h);
            if attached {
                AttachThreadInput(fg_tid, my_tid, 0);
            }
        }
        let cmd = TrackPopupMenu(
            hmenu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RIGHTBUTTON,
            x,
            y,
            0,
            h,
            std::ptr::null(),
        );
        PostMessageW(h, WM_NULL, 0, 0);
        DestroyMenu(hmenu);
        drop(bufs);
        if cmd > 0 {
            Some(cmd as u32)
        } else {
            None
        }
    }
}

/// 主窗口 HWND（拿来做 TrackPopupMenu 的宿主）
fn main_hwnd(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// 托盘只放图标；菜单由 `show_native_tray_menu` 自己弹
fn build_tray() -> anyhow::Result<TrayIcon> {
    TrayIconBuilder::new()
        .with_tooltip("live-caption 实时字幕：点击打开菜单")
        .with_icon(tray_icon_image())
        .with_menu_on_left_click(false)
        .with_menu_on_right_click(false)
        .build()
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// 指针落在窗口边缘 8 px 内时给出拉伸方向
fn edge_direction(rect: egui::Rect, p: egui::Pos2) -> Option<ResizeDirection> {
    const E: f32 = 8.0;
    let left = p.x - rect.min.x < E;
    let right = rect.max.x - p.x < E;
    let top = p.y - rect.min.y < E;
    let bottom = rect.max.y - p.y < E;
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, _, _, _) => ResizeDirection::West,
        (_, true, _, _) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (_, _, _, true) => ResizeDirection::South,
        _ => return None,
    })
}

fn cursor_for(d: ResizeDirection) -> CursorIcon {
    match d {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
    }
}

/// 用系统关联程序打开记录文件（ShellExecute，无控制台闪屏）；没有关联程序（SE_ERR_NOASSOC=31）就退回记事本
fn open_path(path: &std::path::Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // canonicalize 会带 \\?\ 长路径前缀，有的编辑器不认，去掉
    let shown = abs.to_string_lossy().trim_start_matches(r"\\?\").to_string();
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (op, file) = (wide("open"), wide(&shown));
    // 文件没有关联程序时 ShellExecute 会弹"选择打开方式"（用户看到的就是一闪而过的窗口），直接改用记事本
    let is_file = abs.is_file();
    if is_file && !has_file_association(&abs) {
        tracing::info!("open {shown}: 无关联程序，用记事本");
        let _ = std::process::Command::new("notepad.exe").arg(&shown).spawn();
        return;
    }
    let rc = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) } as isize;
    tracing::info!("open {shown} rc={rc}");
    if rc <= 32 && is_file {
        let _ = std::process::Command::new("notepad.exe").arg(&shown).spawn();
    }
}

/// 按 Shell 的实际"打开方式"解析出可执行程序（含用户级 UserChoice），解析不出才算无关联
fn has_file_association(path: &std::path::Path) -> bool {
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};
    let Some(ext) = path.extension().map(|e| e.to_string_lossy().into_owned()) else { return false };
    let key: Vec<u16> = format!(".{ext}").encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 1024];
    let mut len: u32 = buf.len() as u32;
    let hr = unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, key.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len) };
    if hr == 0 && len > 1 {
        let exe = String::from_utf16_lossy(&buf[..(len as usize - 1)]);
        tracing::info!(".{ext} 的打开程序：{exe}");
        true
    } else {
        tracing::info!(".{ext} 无关联程序（hr=0x{hr:X}）");
        false
    }
}

fn list_models(dir: &str, current: &str) -> Vec<String> {
    let mut v = Vec::new();
    if !current.is_empty() {
        v.push(current.to_string());
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut found: Vec<String> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |x| x == "bin"))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        found.sort();
        for f in found {
            if !v.contains(&f) {
                v.push(f);
            }
        }
    }
    v
}

fn install_fonts(ctx: &egui::Context, path: &str) -> anyhow::Result<()> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let fallbacks = ["msyh.ttc", "msyhl.ttc", "simhei.ttf", "simsun.ttc", "Deng.ttf"];
    let mut candidates = vec![path.to_string()];
    candidates.extend(fallbacks.iter().map(|f| format!(r"{windir}\Fonts\{f}")));
    let mut bytes = None;
    for c in &candidates {
        if let Ok(b) = std::fs::read(c) {
            if c != path {
                tracing::warn!("字体 {path} 不可用，改用 {c}");
            }
            bytes = Some(b);
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| anyhow::anyhow!("没有找到任何中文字体：{candidates:?}"))?;
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "cjk".into());
    ctx.set_fonts(fonts);
    Ok(())
}

impl eframe::App for CaptionApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain();

        // 打字机推进
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.2);
        self.last_frame = now;
        let cps = self.cfg.ui.typewriter_cps;
        let mut animating = false;
        for l in self.lines.iter_mut() {
            animating |= l.advance(dt, cps);
        }

        // 全局热键：Ctrl+Alt+C 切回可点击；Ctrl+Alt+P 切换穿透
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            let want = if ev.id == self.hotkey_toggle_id { !self.passthrough } else { false };
            tracing::info!("hotkey id={} -> passthrough={want}", ev.id);
            self.passthrough = want;
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(want));
        }
        // 托盘：任意键松开 = 弹原生菜单（同步拿回所选项）
        let mut want_settings = false;
        let mut want_records = false;
        if self.tray.is_some() {
            let mut click: Option<(i32, i32)> = None;
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click { button_state: MouseButtonState::Up, position, .. } = ev {
                    click = Some((position.x as i32, position.y as i32));
                }
            }
            if let Some((x, y)) = click {
                tracing::info!("tray click at ({x}, {y})");
                let paused_now = self.paused.load(Ordering::Relaxed);
                let mini_now = self.mini;
                let chosen = main_hwnd(frame).and_then(|h| show_native_tray_menu(h, x, y, paused_now, self.passthrough, mini_now));
                if let Some(cmd) = chosen {
                    tracing::info!("tray menu: {cmd}");
                    match cmd {
                        MENU_CLICKABLE => {
                            self.passthrough = false;
                            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(false));
                        }
                        MENU_PASSTHROUGH => {
                            self.passthrough = true;
                            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));
                        }
                        MENU_PAUSE => self.paused.store(!paused_now, Ordering::Relaxed),
                        MENU_SETTINGS => want_settings = true,
                        MENU_TRANSCRIPT => {
                            want_records = true;
                        }
                        MENU_MINI => self.set_mini(!mini_now),
                        MENU_QUIT => ctx.send_viewport_cmd(ViewportCommand::Close),
                        _ => {}
                    }
                }
            }
        }

        let rect = ui.max_rect();
        let alpha = (self.cfg.ui.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        ui.painter().rect_filled(rect, CornerRadius::same(14), Color32::from_rgba_unmultiplied(12, 12, 16, alpha));

        // 边缘拉伸：指针在边缘 8 px 内按下即交给系统调大小；松手后把新高度记为最小高度
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let edge = pointer.and_then(|p| edge_direction(rect, p));
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        let any_down = ctx.input(|i| i.pointer.any_down());
        if let Some(d) = edge {
            ctx.set_cursor_icon(cursor_for(d));
            if pressed {
                self.resizing = true;
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
            }
        }
        if self.resizing && !any_down {
            self.resizing = false;
            let h = rect.height().round() as u32;
            self.cfg.ui.height = h.max(60);
            if self.cfg.ui.max_height < self.cfg.ui.height {
                self.cfg.ui.max_height = self.cfg.ui.height;
            }
            self.last_resize = Instant::now();
        }

        // 拖动整窗（边缘以外）；迷你条上单击 = 展开
        let drag = ui.interact(rect, Id::new("window-drag"), Sense::click_and_drag());
        if drag.drag_started() && edge.is_none() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        let hovered = ui.rect_contains_pointer(rect);

        // 自动收起 / 自动展开
        if self.mini && self.cfg.ui.auto_restore && self.last_activity > self.mini_since {
            self.set_mini(false);
        }
        let idle_limit = self.cfg.ui.auto_mini_secs;
        if !self.mini && idle_limit > 0 && !hovered && !self.show_settings && self.last_activity.elapsed() > Duration::from_secs(idle_limit as u64) {
            self.set_mini(true);
        }
        if self.mini {
            if drag.clicked() {
                self.set_mini(false);
            }
            let (color, _) = self.status_color_and_text();
            let paused = self.paused.load(Ordering::Relaxed);
            ui.painter().circle_filled(rect.left_center() + egui::vec2(16.0, 0.0), 5.0, color);
            let text = if paused { "已暂停 · 点击展开" } else { "待机中 · 有语音自动展开，点击也可" };
            ui.painter().text(rect.left_center() + egui::vec2(30.0, 0.0), Align2::LEFT_CENTER, text, egui::FontId::proportional(13.0), Color32::from_gray(210));
            self.fit_height(&ctx, rect, 0.0);
            ctx.request_repaint_after(Duration::from_millis(200));
            return;
        }

        let mut content_h = 0.0f32;
        egui::Frame::NONE.inner_margin(MARGIN).show(ui, |ui| {
            ui.set_width(rect.width() - MARGIN * 2.0);
            let top = ui.horizontal(|ui| {
                let (color, text) = self.status_color_and_text();
                let dot = ui.allocate_response(egui::vec2(12.0, 12.0), Sense::hover());
                ui.painter().circle_filled(dot.rect.center(), 5.0, color);
                if !text.is_empty() {
                    ui.label(RichText::new(text).size(12.0).color(Color32::from_gray(200)));
                }
                if hovered {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("✕").on_hover_text("退出").clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                        if ui.small_button("设置").clicked() {
                            want_settings = true;
                        }
                        if ui.small_button("收起").on_hover_text("缩成待机迷你条；有语音自动展开").clicked() {
                            self.set_mini(true);
                        }
                        if ui.small_button("穿透").on_hover_text("鼠标穿透，Ctrl+Alt+C 切回").clicked() {
                            self.passthrough = true;
                            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));
                        }
                        if ui.small_button("记录").on_hover_text("打开记录文件").clicked() {
                            want_records = true;
                        }
                        if ui.small_button("透-").clicked() {
                            self.cfg.ui.opacity = (self.cfg.ui.opacity - 0.1).max(0.1);
                        }
                        if ui.small_button("透+").clicked() {
                            self.cfg.ui.opacity = (self.cfg.ui.opacity + 0.1).min(1.0);
                        }
                        if ui.small_button("A-").clicked() {
                            self.cfg.ui.font_size = (self.cfg.ui.font_size - 2.0).max(12.0);
                        }
                        if ui.small_button("A+").clicked() {
                            self.cfg.ui.font_size = (self.cfg.ui.font_size + 2.0).min(60.0);
                        }
                        if ui.small_button("行-").on_hover_text("少显示一段").clicked() {
                            self.cfg.ui.lines = self.cfg.ui.lines.saturating_sub(1).max(1);
                        }
                        if ui.small_button("行+").on_hover_text("多显示一段").clicked() {
                            self.cfg.ui.lines = (self.cfg.ui.lines + 1).min(8);
                        }
                        let paused = self.paused.load(Ordering::Relaxed);
                        if ui.small_button(if paused { "▶ 继续" } else { "⏸ 暂停" }).clicked() {
                            self.paused.store(!paused, Ordering::Relaxed);
                        }
                    });
                }
            });
            ui.add_space(4.0);
            let fs = self.cfg.ui.font_size;
            let n_final = self.lines.iter().filter(|l| !l.partial).count();
            let out = egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut seen_final = 0;
                    for l in self.lines.iter() {
                        // 正在打的那一行高亮（淡黄底带 + 亮色英文）；定稿行不高亮，最新一句全亮、更早的变暗
                        let dim = if l.partial {
                            255
                        } else {
                            seen_final += 1;
                            if seen_final < n_final { 150 } else { 255 }
                        };
                        let render = |ui: &mut egui::Ui| {
                            let en_text = head(&l.en, l.en_shown);
                            let en_color = if l.partial {
                                Color32::from_rgba_unmultiplied(255, 236, 160, 255)
                            } else {
                                Color32::from_rgba_unmultiplied(200, 210, 230, dim)
                            };
                            ui.add(egui::Label::new(RichText::new(en_text).size(fs * 0.85).color(en_color)).wrap());
                            match &l.zh {
                                Some(zh) => {
                                    let zh_text = head(zh, l.zh_shown);
                                    let rt = if l.zh_provisional {
                                        RichText::new(zh_text).size(fs).italics().color(Color32::from_rgba_unmultiplied(240, 240, 240, 225))
                                    } else {
                                        RichText::new(zh_text).size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, dim))
                                    };
                                    ui.add(egui::Label::new(rt).wrap());
                                }
                                None if !l.partial => {
                                    ui.label(RichText::new("…").size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, 90)));
                                }
                                None => {}
                            }
                        };
                        if l.partial {
                            egui::Frame::new()
                                .fill(Color32::from_rgba_unmultiplied(255, 225, 140, 30))
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    render(ui);
                                });
                        } else {
                            render(ui);
                        }
                        ui.add_space(6.0);
                    }
                });
            content_h = top.response.rect.height() + 4.0 + out.content_size.y;
        });

        if want_settings {
            self.open_settings();
        }
        if want_records {
            self.open_records();
        }
        if self.show_settings {
            // 与字幕条一样置顶，否则会被别的窗口压住，第二次"打开"看不见
            let vb = ViewportBuilder::default()
                .with_title("live-caption 设置")
                .with_always_on_top()
                .with_inner_size([780.0, 560.0])
                .with_min_inner_size([640.0, 460.0]);
            ctx.show_viewport_immediate(ViewportId::from_hash_of("settings"), vb, |ui, _class| {
                if ui.input(|i| i.viewport().close_requested()) {
                    self.show_settings = false;
                }
                if self.settings_raise {
                    self.settings_raise = false;
                    ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
                    ui.ctx().send_viewport_cmd(ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
                let c = ui.ctx().clone();
                self.settings_window(ui, &c);
            });
        }

        self.fit_height(&ctx, rect, content_h);
        self.maybe_save_window_rect(&ctx);
        ctx.request_repaint_after(if animating { Duration::from_millis(16) } else { Duration::from_millis(100) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: SegId) -> Line {
        Line { id, at: Local::now(), en: "x".into(), zh: None, partial: true, en_shown: 0.0, zh_shown: 0.0, zh_provisional: false, zh_final_started: false }
    }

    #[test]
    fn set_zh_swaps_shown_part_in_place_and_types_only_the_rest() {
        let mut l = line(1);
        l.set_zh("我们现在开始会议".into(), true);
        l.zh_shown = 6.0;
        l.set_zh("我们现在开始今天的会议".into(), true);
        assert_eq!(l.zh_shown, 6.0, "已显示的 6 字不重打，后面接着打");
        l.set_zh("大家来开始".into(), false);
        assert_eq!(l.zh_shown, 5.0, "改写了也不擦掉重打：已显示部分原地换字，只截到新长度");
        assert!(!l.zh_provisional);
    }

    #[test]
    fn provisional_is_ignored_once_final_stream_started() {
        let mut l = line(3);
        assert!(l.apply_provisional("半句".into()));
        assert!(l.zh_provisional);
        l.set_zh("定稿".into(), false);
        l.zh_final_started = true;
        assert!(!l.apply_provisional("晚到的半句".into()));
        assert_eq!(l.zh.as_deref(), Some("定稿"));
    }

    #[test]
    fn zh_only_moves_when_grown_or_diverged() {
        let mut l = line(4);
        assert!(l.apply_provisional("我们接着聊臭名昭著的B.I.G.".into()));
        l.zh_shown = 8.0;
        // 定稿流刚开始，比已显示的短：不动，哪怕开头已经不同
        assert!(!l.set_zh_if_grown("我们".into(), false));
        assert!(!l.set_zh_if_grown("我们接着聊大O".into(), false));
        assert_eq!(l.zh.as_deref(), Some("我们接着聊臭名昭著的B.I.G."));
        assert!(l.zh_provisional, "追上之前仍是灰色临时中文");
        // 追过已显示长度且有分歧：已显示的 8 字原地换字不重打，后面接着打
        assert!(l.set_zh_if_grown("我们接着聊大O表示法，也就是复杂度".into(), false));
        assert_eq!(l.zh.as_deref(), Some("我们接着聊大O表示法，也就是复杂度"));
        assert_eq!(l.zh_shown, 8.0);
        assert!(!l.zh_provisional);
        // 延长：直接接着打
        assert!(l.set_zh_if_grown("我们接着聊大O表示法，也就是复杂度的写法".into(), false));
        assert_eq!(l.zh_shown, 8.0);
        // 定稿完成的整句比显示的短也照样替换（走 set_zh）
        l.set_zh("我们接着聊大O。".into(), false);
        assert_eq!(l.zh.as_deref(), Some("我们接着聊大O。"));
        // 临时译文被模型截短、或识别抖动只吐一个词译成一个字：不采纳
        let mut p = line(5);
        assert!(p.apply_provisional("并确认GPU服务器的预算".into()));
        assert!(!p.apply_provisional("并确认".into()));
        assert!(!p.apply_provisional("这".into()));
        assert_eq!(p.zh.as_deref(), Some("并确认GPU服务器的预算"));
    }

    fn ens(lines: &VecDeque<Line>) -> Vec<(SegId, bool, String)> {
        lines.iter().map(|l| (l.id, l.partial, l.en.clone())).collect()
    }

    #[test]
    fn english_lines_follow_utterance_ids_even_when_next_partial_arrives_before_final() {
        let mut lines = VecDeque::new();
        apply_english_partial(&mut lines, 1, "Good".into());
        apply_english_partial(&mut lines, 1, "Good morning".into());
        assert_eq!(ens(&lines), vec![(1, true, "Good morning".into())]);
        // 下一句的中间结果先到（识别线程插队）：另开一行，上一句的中间行留着
        apply_english_partial(&mut lines, 2, "Today we".into());
        assert_eq!(ens(&lines), vec![(1, true, "Good morning".into()), (2, true, "Today we".into())]);
        // 上一句定稿晚到：原地转正，不影响第二句
        apply_english_final(&mut lines, 1, Local::now(), "Good morning everyone!".into());
        assert_eq!(ens(&lines), vec![(1, false, "Good morning everyone!".into()), (2, true, "Today we".into())]);
        apply_english_partial(&mut lines, 2, "Today we will review".into());
        apply_english_final(&mut lines, 2, Local::now(), "Today we will review the roadmap.".into());
        assert_eq!(ens(&lines), vec![(1, false, "Good morning everyone!".into()), (2, false, "Today we will review the roadmap.".into())]);
    }

    #[test]
    fn final_without_partial_keeps_utterance_order() {
        let mut lines = VecDeque::new();
        apply_english_partial(&mut lines, 5, "next sentence".into());
        // 第 4 句没有中间结果、定稿晚到：插到第 5 句前面
        apply_english_final(&mut lines, 4, Local::now(), "Previous sentence.".into());
        assert_eq!(ens(&lines), vec![(4, false, "Previous sentence.".into()), (5, true, "next sentence".into())]);
    }

    #[test]
    fn empty_partial_removes_only_that_utterance() {
        let mut lines = VecDeque::new();
        apply_english_partial(&mut lines, 1, "noise maybe".into());
        apply_english_partial(&mut lines, 2, "real speech".into());
        apply_english_partial(&mut lines, 1, String::new());
        assert_eq!(ens(&lines), vec![(2, true, "real speech".into())]);
        apply_english_partial(&mut lines, 9, String::new());
        assert_eq!(lines.len(), 1, "没有这个话轮的行就什么都不做");
    }

    #[test]
    fn partial_english_never_shrinks() {
        assert_eq!(merge_partial_en("Please like and subscribe.", "Please"), None, "缩短不采纳");
        assert_eq!(merge_partial_en("Please", "Please"), None, "没变不动");
        assert_eq!(merge_partial_en("Please", "Please like").as_deref(), Some("Please like"), "延长采纳");
        assert_eq!(merge_partial_en("notorious B.I.G.", "notorious B-I-G-O notation").as_deref(), Some("notorious B-I-G-O notation"), "更长的改写采纳");
        assert_eq!(merge_partial_en("Many, many approaches", "Many approaches"), None, "更短的改写不采纳");
        assert_eq!(merge_partial_en("abc", "xyz"), None, "等长改写不采纳");
    }

    #[test]
    fn typewriter_advances_and_stops() {
        let mut l = Line { id: 1, at: Local::now(), en: "abcdef".into(), zh: Some("你好".into()), partial: false, en_shown: 0.0, zh_shown: 0.0, zh_provisional: false, zh_final_started: false };
        assert!(l.advance(0.1, 20.0)); // 2 字
        assert_eq!(head(&l.en, l.en_shown), "ab");
        assert_eq!(head(l.zh.as_ref().unwrap(), l.zh_shown), "你好");
        assert!(l.advance(1.0, 20.0));
        assert!(!l.advance(1.0, 20.0), "显示完就不再推进");
        assert_eq!(head(&l.en, l.en_shown), "abcdef");
        let mut instant = Line { id: 2, at: Local::now(), en: "xyz".into(), zh: None, partial: false, en_shown: 0.0, zh_shown: 0.0, zh_provisional: false, zh_final_started: false };
        assert!(!instant.advance(0.0, 0.0), "cps=0 整句直出");
        assert_eq!(head(&instant.en, instant.en_shown), "xyz");
    }
}
