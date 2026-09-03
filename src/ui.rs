use crate::config::Config;
use crate::messages::{SegId, Stage, StageState, UiMsg};
use crate::transcript::{Transcript, TranscriptFile};
use chrono::{DateTime, Local};
use crossbeam_channel::Receiver;
use egui::{Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Id, Layout, RichText, Sense, ViewportCommand};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Line {
    id: SegId,
    #[allow(dead_code)]
    at: DateTime<Local>,
    en: String,
    zh: Option<String>,
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
    last_cfg_save: Instant,
}

impl CaptionApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rx: Receiver<UiMsg>, cfg: Config, paused: Arc<AtomicBool>) -> Self {
        if let Err(e) = install_fonts(&cc.egui_ctx, &cfg.ui.font_path) {
            tracing::warn!("加载中文字体失败：{e}");
        }
        let hotkeys = GlobalHotKeyManager::new()
            .ok()
            .and_then(|m| m.register(HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyC)).ok().map(|_| m));
        if hotkeys.is_none() {
            tracing::warn!("全局热键 Ctrl+Alt+C 注册失败");
        }
        let file = match TranscriptFile::create(&cfg.paths.transcripts_dir) {
            Ok(f) => Some(f),
            Err(e) => {
                tracing::warn!("建记录文件失败：{e}");
                None
            }
        };
        Self {
            rx,
            cfg,
            lines: VecDeque::new(),
            status: HashMap::new(),
            paused,
            passthrough: false,
            transcript: Transcript::default(),
            file,
            _hotkeys: hotkeys,
            last_cfg_save: Instant::now(),
        }
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::English { id, at, text } => {
                    self.transcript.push_en(id, at, &text);
                    self.lines.push_back(Line { id, at, en: text, zh: None });
                    while self.lines.len() > 2 {
                        self.lines.pop_front();
                    }
                }
                UiMsg::Chinese { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.zh = Some(text.clone());
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

    fn maybe_save_window_rect(&mut self, ctx: &egui::Context) {
        if self.last_cfg_save.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_cfg_save = Instant::now();
        let Some(r) = ctx.input(|i| i.viewport().outer_rect) else { return };
        let (x, y, w, h) = (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32);
        if (x, y, w, h) != (self.cfg.ui.x, self.cfg.ui.y, self.cfg.ui.width, self.cfg.ui.height) {
            self.cfg.ui.x = x;
            self.cfg.ui.y = y;
            self.cfg.ui.width = w;
            self.cfg.ui.height = h;
            if let Err(e) = self.cfg.save() {
                tracing::warn!("保存配置失败：{e}");
            }
        }
    }
}

fn install_fonts(ctx: &egui::Context, path: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
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

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain();

        // 全局热键：切回可点击
        if let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state == HotKeyState::Pressed && self.passthrough {
                self.passthrough = false;
                ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(false));
            }
        }

        let rect = ui.max_rect();
        let alpha = (self.cfg.ui.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        ui.painter().rect_filled(rect, CornerRadius::same(14), Color32::from_rgba_unmultiplied(12, 12, 16, alpha));

        // 拖动整窗
        let drag = ui.interact(rect, Id::new("window-drag"), Sense::drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let hovered = ui.rect_contains_pointer(rect);
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            // 顶部：状态点 + 提示 + 悬停工具条
            ui.horizontal(|ui| {
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
                        if ui.small_button("穿透").on_hover_text("鼠标穿透，Ctrl+Alt+C 切回").clicked() {
                            self.passthrough = true;
                            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));
                        }
                        if ui.small_button("记录").on_hover_text("打开记录文件").clicked() {
                            if let Some(f) = &self.file {
                                let _ = std::process::Command::new("cmd")
                                    .args(["/C", "start", "", &f.path.to_string_lossy()])
                                    .spawn();
                            }
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
                        let paused = self.paused.load(Ordering::Relaxed);
                        if ui.small_button(if paused { "▶ 继续" } else { "⏸ 暂停" }).clicked() {
                            self.paused.store(!paused, Ordering::Relaxed);
                        }
                    });
                }
            });
            ui.add_space(4.0);
            let fs = self.cfg.ui.font_size;
            let n = self.lines.len();
            for (i, l) in self.lines.iter().enumerate() {
                let dim = if i + 1 < n { 150 } else { 255 };
                ui.label(RichText::new(&l.en).size(fs * 0.85).color(Color32::from_rgba_unmultiplied(200, 210, 230, dim)));
                match &l.zh {
                    Some(zh) => {
                        ui.label(RichText::new(zh).size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, dim)));
                    }
                    None => {
                        ui.label(RichText::new("…").size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, 90)));
                    }
                }
                ui.add_space(6.0);
            }
        });

        self.maybe_save_window_rect(&ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
