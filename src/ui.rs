use crate::config::Config;
use crate::messages::{SegId, Stage, StageState, UiMsg};
use crate::transcript::{Transcript, TranscriptFile};
use crate::translate::TranslatorConfig;
use chrono::{DateTime, Local};
use crossbeam_channel::{Receiver, Sender};
use egui::{Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Id, Layout, RichText, Sense, ViewportBuilder, ViewportCommand, ViewportId};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MARGIN: f32 = 14.0;

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
}

impl Line {
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
    last_cfg_save: Instant,
    last_resize: Instant,
    last_frame: Instant,
    // 设置窗
    tx_cfg: Sender<TranslatorConfig>,
    show_settings: bool,
    draft: Config,
    models: Vec<String>,
    settings_note: String,
}

impl CaptionApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rx: Receiver<UiMsg>,
        cfg: Config,
        paused: Arc<AtomicBool>,
        tx_cfg: Sender<TranslatorConfig>,
    ) -> Self {
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
        let draft = cfg.clone();
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
            last_resize: Instant::now(),
            last_frame: Instant::now(),
            tx_cfg,
            show_settings: false,
            draft,
            models: Vec::new(),
            settings_note: String::new(),
        }
    }

    fn drain(&mut self) {
        let keep = self.cfg.ui.lines.max(1);
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::EnglishPartial { text } => {
                    if text.is_empty() {
                        if self.lines.back().map_or(false, |l| l.partial) {
                            self.lines.pop_back();
                        }
                        continue;
                    }
                    match self.lines.back_mut() {
                        Some(l) if l.partial => {
                            // 新快照通常是旧快照的延长；已显示的部分不回退
                            l.en_shown = l.en_shown.min(text.chars().count() as f32);
                            l.en = text;
                        }
                        _ => {
                            self.lines.push_back(Line { id: 0, at: Local::now(), en: text, zh: None, partial: true, en_shown: 0.0, zh_shown: 0.0 });
                        }
                    }
                }
                UiMsg::English { id, at, text } => {
                    match self.lines.back_mut() {
                        Some(l) if l.partial => {
                            l.id = id;
                            l.at = at;
                            l.en_shown = l.en_shown.min(text.chars().count() as f32);
                            l.en = text.clone();
                            l.partial = false;
                        }
                        _ => {
                            self.lines.push_back(Line { id, at, en: text.clone(), zh: None, partial: false, en_shown: 0.0, zh_shown: 0.0 });
                        }
                    }
                    self.transcript.push_en(id, at, &text);
                }
                UiMsg::ChinesePartial { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.zh_shown = l.zh_shown.min(text.chars().count() as f32);
                        l.zh = Some(text);
                    }
                }
                UiMsg::Chinese { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) {
                        l.zh_shown = l.zh_shown.min(text.chars().count() as f32);
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

    /// 窗口高度随内容伸缩：下边缘固定，向上长；范围 [height, max_height]
    fn fit_height(&mut self, ctx: &egui::Context, inner: egui::Rect, content_h: f32) {
        if self.last_resize.elapsed() < Duration::from_millis(150) {
            return;
        }
        let cur = inner.height();
        let min_h = self.cfg.ui.height as f32;
        let max_h = self.cfg.ui.max_height.max(self.cfg.ui.height) as f32;
        let want = (content_h + MARGIN * 2.0 + 4.0).clamp(min_h, max_h);
        if (want - cur).abs() < 2.0 {
            return;
        }
        let delta = want - cur;
        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(outer.min.x, (outer.min.y - delta).max(0.0))));
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(inner.width(), want)));
        self.last_resize = Instant::now();
    }

    fn maybe_save_window_rect(&mut self, ctx: &egui::Context) {
        if self.last_cfg_save.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_cfg_save = Instant::now();
        let Some(r) = ctx.input(|i| i.viewport().outer_rect) else { return };
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
        self.models = list_models(&self.cfg.asr.models_dir, &self.cfg.asr.model_path);
        self.show_settings = true;
    }

    fn apply_settings(&mut self) {
        let asr_changed = self.draft.asr != self.cfg.asr || self.draft.audio != self.cfg.audio;
        self.cfg = self.draft.clone();
        match self.cfg.save() {
            Ok(()) => {
                let _ = self.tx_cfg.send(TranslatorConfig::from(&self.cfg.translate));
                self.settings_note = if asr_changed {
                    "已保存。翻译后端已切换；识别/音频参数改动需重启应用生效。".into()
                } else {
                    "已保存并应用。".into()
                };
            }
            Err(e) => self.settings_note = format!("保存失败：{e}"),
        }
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        let d = &mut self.draft;
        ui.heading("翻译");
        egui::ComboBox::from_label("后端")
            .selected_text(match d.translate.backend.as_str() {
                "codex" => "Codex CLI（OpenAI 订阅）",
                "openai" => "OpenAI 兼容接口（DeepSeek / 通义千问 / 网关 / Ollama）",
                _ => "Claude Code CLI（Anthropic 订阅）",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.translate.backend, "claude".into(), "Claude Code CLI（Anthropic 订阅）");
                ui.selectable_value(&mut d.translate.backend, "codex".into(), "Codex CLI（OpenAI 订阅）");
                ui.selectable_value(&mut d.translate.backend, "openai".into(), "OpenAI 兼容接口（DeepSeek / 通义千问 / 网关 / Ollama）");
            });
        match d.translate.backend.as_str() {
            "codex" => {
                ui.horizontal(|ui| {
                    ui.label("codex 命令");
                    ui.text_edit_singleline(&mut d.translate.codex_bin);
                });
                ui.horizontal(|ui| {
                    ui.label("模型（留空用默认）");
                    ui.text_edit_singleline(&mut d.translate.codex_model);
                });
                ui.label(RichText::new("每句起一个 codex exec 进程，延迟通常 3 秒以上；适合不在乎速度只想用订阅的场景。").size(12.0).color(Color32::GRAY));
            }
            "openai" => {
                ui.horizontal(|ui| {
                    ui.label("预设：");
                    if ui.small_button("DeepSeek").clicked() {
                        d.translate.openai_base_url = "https://api.deepseek.com".into();
                        d.translate.openai_model = "deepseek-chat".into();
                    }
                    if ui.small_button("通义千问").clicked() {
                        d.translate.openai_base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1".into();
                        d.translate.openai_model = "qwen-turbo".into();
                    }
                    if ui.small_button("自建网关").clicked() {
                        d.translate.openai_base_url = "https://gateway.example.com/v1".into();
                        d.translate.openai_model = "deepseek-v4-flash".into();
                    }
                    if ui.small_button("Ollama 本地").clicked() {
                        d.translate.openai_base_url = "http://127.0.0.1:11434/v1".into();
                        d.translate.openai_model = "qwen2.5:7b".into();
                        d.translate.openai_api_key = "ollama".into();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Base URL");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.openai_base_url).desired_width(360.0));
                });
                ui.horizontal(|ui| {
                    ui.label("API Key");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.openai_api_key).password(true).desired_width(360.0));
                });
                ui.horizontal(|ui| {
                    ui.label("模型");
                    ui.text_edit_singleline(&mut d.translate.openai_model);
                });
                ui.label(RichText::new("请求发到 {Base URL}/chat/completions，流式返回。Key 只存在本机 config.toml。").size(12.0).color(Color32::GRAY));
            }
            _ => {
                ui.horizontal(|ui| {
                    ui.label("claude 路径");
                    ui.add(egui::TextEdit::singleline(&mut d.translate.claude_bin).desired_width(360.0));
                });
                ui.horizontal(|ui| {
                    ui.label("模型");
                    ui.text_edit_singleline(&mut d.translate.claude_model);
                    ui.label(RichText::new("sonnet / haiku / opus").size(12.0).color(Color32::GRAY));
                });
            }
        }
        ui.horizontal(|ui| {
            ui.label("单句超时（秒）");
            ui.add(egui::Slider::new(&mut d.translate.timeout_secs, 3..=60));
        });

        ui.separator();
        ui.heading("识别（改动需重启）");
        let cur_model = d.asr.model_path.clone();
        egui::ComboBox::from_label("模型")
            .selected_text(std::path::Path::new(&cur_model).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(cur_model.clone()))
            .width(360.0)
            .show_ui(ui, |ui| {
                for m in &self.models {
                    let name = std::path::Path::new(m).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(m.clone());
                    ui.selectable_value(&mut d.asr.model_path, m.clone(), name);
                }
            });
        ui.horizontal(|ui| {
            ui.label("模型目录");
            ui.add(egui::TextEdit::singleline(&mut d.asr.models_dir).desired_width(360.0));
        });
        ui.label(RichText::new("large-v3-turbo 最准（每段约 1.5–3 s）；small.en 约 1 s；base.en 约 0.4 s 但更容易听错。").size(12.0).color(Color32::GRAY));
        ui.checkbox(&mut d.asr.flash_attn, "flash attention（本机 Arc 核显实测反而更慢，默认关）");
        ui.horizontal(|ui| {
            ui.label("audio_ctx 下限");
            ui.add(egui::Slider::new(&mut d.asr.min_audio_ctx, 128..=1500));
        });
        ui.horizontal(|ui| {
            ui.label("CPU 线程");
            ui.add(egui::Slider::new(&mut d.asr.threads, 1..=16));
        });

        ui.separator();
        ui.heading("切句（改动需重启）");
        ui.horizontal(|ui| {
            ui.label("能量门限");
            ui.add(egui::Slider::new(&mut d.audio.energy_threshold, 0.001..=0.1).logarithmic(true));
        });
        ui.horizontal(|ui| {
            ui.label("停顿多久算一句结束（ms）");
            ui.add(egui::Slider::new(&mut d.audio.silence_ms, 200..=1500));
        });
        ui.horizontal(|ui| {
            ui.label("逐字上屏：说话中每隔（ms）识别一次，0 关闭");
            ui.add(egui::Slider::new(&mut d.audio.partial_every_ms, 0..=4000));
        });
        ui.horizontal(|ui| {
            ui.label("单段最长（ms）");
            ui.add(egui::Slider::new(&mut d.audio.max_segment_ms, 3000..=15000));
        });

        ui.separator();
        ui.heading("界面（立即生效）");
        ui.horizontal(|ui| {
            ui.label("字号");
            ui.add(egui::Slider::new(&mut d.ui.font_size, 12.0..=60.0));
        });
        ui.horizontal(|ui| {
            ui.label("不透明度");
            ui.add(egui::Slider::new(&mut d.ui.opacity, 0.1..=1.0));
        });
        ui.horizontal(|ui| {
            ui.label("同时显示段数");
            ui.add(egui::Slider::new(&mut d.ui.lines, 1..=8));
        });
        ui.horizontal(|ui| {
            ui.label("打字机速度（字/秒，0 = 整句直出）");
            ui.add(egui::Slider::new(&mut d.ui.typewriter_cps, 0.0..=300.0));
        });
        ui.horizontal(|ui| {
            ui.label("最大高度");
            ui.add(egui::Slider::new(&mut d.ui.max_height, 120..=1200));
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("保存并应用").clicked() {
                self.apply_settings();
            }
            if ui.button("关闭").clicked() {
                self.show_settings = false;
            }
            if !self.settings_note.is_empty() {
                ui.label(RichText::new(&self.settings_note).color(Color32::from_rgb(120, 200, 120)));
            }
        });
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

        // 打字机推进
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.2);
        self.last_frame = now;
        let cps = self.cfg.ui.typewriter_cps;
        let mut animating = false;
        for l in self.lines.iter_mut() {
            animating |= l.advance(dt, cps);
        }

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
        let mut content_h = 0.0f32;
        let mut want_settings = false;
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
                        let dim = if l.partial {
                            170
                        } else {
                            seen_final += 1;
                            if seen_final < n_final { 150 } else { 255 }
                        };
                        let en_text = head(&l.en, l.en_shown);
                        let en_rt = RichText::new(en_text).size(fs * 0.85).color(Color32::from_rgba_unmultiplied(200, 210, 230, dim));
                        ui.add(egui::Label::new(if l.partial { en_rt.italics() } else { en_rt }).wrap());
                        match &l.zh {
                            Some(zh) => {
                                let zh_text = head(zh, l.zh_shown);
                                ui.add(egui::Label::new(RichText::new(zh_text).size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, dim))).wrap());
                            }
                            None if !l.partial => {
                                ui.label(RichText::new("…").size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, 90)));
                            }
                            None => {}
                        }
                        ui.add_space(6.0);
                    }
                });
            content_h = top.response.rect.height() + 4.0 + out.content_size.y;
        });

        if want_settings {
            self.open_settings();
        }
        if self.show_settings {
            let vb = ViewportBuilder::default().with_title("live-caption 设置").with_inner_size([620.0, 760.0]);
            ctx.show_viewport_immediate(ViewportId::from_hash_of("settings"), vb, |ui, _class| {
                if ui.input(|i| i.viewport().close_requested()) {
                    self.show_settings = false;
                }
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.settings_panel(ui));
                });
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

    #[test]
    fn typewriter_advances_and_stops() {
        let mut l = Line { id: 1, at: Local::now(), en: "abcdef".into(), zh: Some("你好".into()), partial: false, en_shown: 0.0, zh_shown: 0.0 };
        assert!(l.advance(0.1, 20.0)); // 2 字
        assert_eq!(head(&l.en, l.en_shown), "ab");
        assert_eq!(head(l.zh.as_ref().unwrap(), l.zh_shown), "你好");
        assert!(l.advance(1.0, 20.0));
        assert!(!l.advance(1.0, 20.0), "显示完就不再推进");
        assert_eq!(head(&l.en, l.en_shown), "abcdef");
        let mut instant = Line { id: 2, at: Local::now(), en: "xyz".into(), zh: None, partial: false, en_shown: 0.0, zh_shown: 0.0 };
        assert!(!instant.advance(0.0, 0.0), "cps=0 整句直出");
        assert_eq!(head(&instant.en, instant.en_shown), "xyz");
    }
}
