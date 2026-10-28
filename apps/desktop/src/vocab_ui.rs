//! 生词库窗口：左边词表，右边测验。
//!
//! 纯逻辑（存盘、出题、判分）在 `vocab.rs`，这里只管画与交互。
//! 画在**根视口**里（跟设置窗一样）：这台 200% 缩放的机器上子视口只能显示左上角四分之一。

use crate::vocab::{check, Answer, Kind, Question, QuizSession, SharedVocab, VocabSettings, Word};
use egui::{Color32, RichText};

/// 生词库页的状态
#[derive(Default)]
pub struct VocabPage {
    /// 当前这局测验；None = 还没开始
    pub session: Option<QuizSession>,
    /// 拼写题输入框
    pub input: String,
    /// 正在拖的左边第几个词
    pub drag: Option<usize>,
    /// 右边第 i 个槽放的是左边第几个词
    pub slots: Vec<Option<usize>>,
}

impl VocabPage {
    /// 开一局新测验：按设置抽 n 个词、混着出题
    pub fn start(&mut self, store: &SharedVocab) {
        let (words, settings) = {
            let g = store.lock().unwrap();
            let words: Vec<Word> = g.eligible().into_iter().cloned().collect();
            (words, g.settings().clone())
        };
        let refs: Vec<&Word> = words.iter().collect();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let n = settings.quiz_count.clamp(1, 50);
        self.session = Some(QuizSession::new(&refs, n, &settings.kind_list(), seed));
        self.input.clear();
        self.slots.clear();
        self.drag = None;
    }
}

/// 画整个生词库页。`root_ui` 是根视口的 Ui（与设置窗同一个）。
/// 返回 true = 用户点了「设置」，要去设置页。
pub fn draw(page: &mut VocabPage, root_ui: &mut egui::Ui, store: &SharedVocab) -> bool {
    let mut to_settings = false;
    egui::Panel::left("vocab-list").exact_size(240.0).show(root_ui, |ui| {
        draw_list(ui, store);
    });
    egui::CentralPanel::default().show(root_ui, |ui| {
        to_settings = draw_right(page, ui, store);
    });
    to_settings
}

/// 右边：标题 + 设置 + 测验区。返回 true = 要去设置页
fn draw_right(page: &mut VocabPage, ui: &mut egui::Ui, store: &SharedVocab) -> bool {
    let mut to_settings = false;
    let (total, eligible) = {
        let g = store.lock().unwrap();
        (g.len(), g.eligible().len())
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("生词库").size(16.0).strong());
        ui.label(
            RichText::new(format!("{total} 个词 · {eligible} 个可出题"))
                .size(11.0)
                .color(Color32::from_gray(140)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // 这页独占整个窗口，没有左侧导航——**必须留一个回设置页的入口**，
            // 否则用户进来了就出不去了（1.2.0 就这样，Jack 报的）
            if ui
                .add(egui::Button::new(RichText::new("⚙").size(15.0)).frame(false))
                .on_hover_text("设置：翻译服务、快捷键、术语表……")
                .clicked()
            {
                to_settings = true;
            }
            let b = egui::Button::new(RichText::new("开始测验").size(13.0));
            if ui.add_enabled(eligible > 0, b).clicked() {
                page.start(store);
            }
        });
    });
    ui.add_space(4.0);
    draw_settings(ui, store);
    ui.separator();
    draw_quiz(page, ui);
    to_settings
}

/// 设置行：收录开关、推送时刻、每次几个词、题型
fn draw_settings(ui: &mut egui::Ui, store: &SharedVocab) {
    let mut s: VocabSettings = store.lock().unwrap().settings().clone();
    let before = s.clone();
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut s.record, "收录查过的词");
        ui.checkbox(&mut s.push_enabled, "每日推送");
        if s.push_enabled {
            ui.add(egui::DragValue::new(&mut s.push_hour).range(0..=23).speed(0.2));
            ui.label(":");
            ui.add(egui::DragValue::new(&mut s.push_minute).range(0..=59).speed(0.5));
            ui.label(RichText::new("推送测验").size(11.0).color(Color32::from_gray(140)));
        }
        ui.label("每次");
        ui.add(egui::DragValue::new(&mut s.quiz_count).range(1..=50));
        ui.label("个词");
        for k in Kind::all() {
            // kinds 为空表示「全都要」，勾选框要显示实际生效的那几种
            let mut on = s.kind_list().contains(&k);
            if ui.checkbox(&mut on, k.label_zh()).changed() {
                let mut next = s.kind_list();
                if on {
                    next.push(k);
                } else {
                    next.retain(|x| x != &k);
                }
                if next.is_empty() {
                    next.push(k); // 至少留一种，全关了就当没动
                }
                s.kinds = next.iter().map(|x| x.as_str().to_string()).collect();
            }
        }
    });
    if s != before {
        let mut g = store.lock().unwrap();
        g.set_settings(s);
        if let Err(e) = g.save() {
            tracing::warn!("生词库设置写盘失败：{e}");
        }
    }
}

/// 左边词表
fn draw_list(ui: &mut egui::Ui, store: &SharedVocab) {
    let words: Vec<Word> = store.lock().unwrap().words().to_vec();
    if words.is_empty() {
        ui.label(
            RichText::new("还没有收录任何词。\n查一个单词就会自动进来。")
                .size(11.0)
                .color(Color32::from_gray(140)),
        );
        return;
    }
    ui.label(
        RichText::new(format!("共 {} 个", words.len()))
            .size(11.0)
            .color(Color32::from_gray(150)),
    );
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for w in &words {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&w.word).size(13.0).strong());
                if !w.phonetic.trim().is_empty() {
                    ui.label(
                        RichText::new(w.phonetic.trim())
                            .size(10.0)
                            .color(Color32::from_gray(150)),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("从生词库删掉").clicked() {
                        let mut g = store.lock().unwrap();
                        g.remove(&w.word);
                        if let Err(e) = g.save() {
                            tracing::warn!("生词库写盘失败：{e}");
                        }
                    }
                });
            });
            if let Some(m) = w.meaning() {
                ui.label(RichText::new(m).size(11.0).color(Color32::from_gray(90)));
            }
            ui.separator();
        }
    });
}

/// 右边测验区
fn draw_quiz(page: &mut VocabPage, ui: &mut egui::Ui) {
    let Some(mut sess) = page.session.take() else {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(
                RichText::new("点右上角「开始测验」，随机抽几个生词考考自己")
                    .size(13.0)
                    .color(Color32::from_gray(120)),
            );
        });
        return;
    };
    let total = sess.questions.len();
    if total == 0 {
        ui.label(
            RichText::new("生词库里还没有能出题的词（要有中文释义才行）")
                .size(13.0)
                .color(Color32::from_gray(120)),
        );
        page.session = Some(sess);
        return;
    }
    let idx = sess.idx;
    let q = sess.questions[idx].clone();
    let answered = sess.is_answered();

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("第 {} / {} 题", idx + 1, total))
                .size(12.0)
                .color(Color32::from_gray(130)),
        );
        ui.label(
            RichText::new(kind_label(&q))
                .size(12.0)
                .color(Color32::from_rgb(70, 110, 190)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (right, total) = sess.score();
            ui.label(
                RichText::new(format!("对 {right} / {total}"))
                    .size(12.0)
                    .color(Color32::from_gray(130)),
            );
        });
    });
    ui.add_space(8.0);

    match &q {
        Question::Choice { prompt, options, answer, to_english, .. } => {
            ui.label(RichText::new(prompt).size(20.0).strong());
            ui.label(
                RichText::new(if *to_english { "选出对应的英文单词" } else { "选出对应的中文意思" })
                    .size(11.0)
                    .color(Color32::from_gray(140)),
            );
            ui.add_space(6.0);
            draw_options(ui, options, *answer, answered, &mut sess, idx);
        }
        Question::Listen { word, options, answer } => {
            ui.label(RichText::new("听发音，选中文意思").size(16.0).strong());
            ui.add_space(6.0);
            if ui.button("🔊 播放").clicked() {
                crate::tts::speak(word);
            }
            ui.add_space(6.0);
            draw_options(ui, options, *answer, answered, &mut sess, idx);
        }
        Question::Spell { word, meaning, sentence } => {
            ui.label(RichText::new(format!("{meaning}（拼出英文单词）")).size(16.0).strong());
            if !sentence.trim().is_empty() {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(sentence)
                        .size(13.0)
                        .color(Color32::from_gray(80)),
                );
            }
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut page.input)
                    .desired_width(240.0)
                    .hint_text("在这里输入英文单词"),
            );
            let mut submit = false;
            ui.horizontal(|ui| {
                submit = ui.button("提交").clicked();
                if ui.small_button("🔊 听提示").clicked() {
                    crate::tts::speak(word);
                }
            });
            if submit && !answered && !page.input.trim().is_empty() {
                sess.submit(Answer::Text(page.input.clone()));
            }
        }
        Question::Pair { left, right, .. } => {
            draw_pair(page, ui, &mut sess, left, right);
        }
    }

    ui.add_space(10.0);
    // 判分反馈
    if answered {
        let ok = check(&q, sess.current_answer());
        ui.label(
            RichText::new(if ok { "✓ 答对了" } else { "✗ 答错了" })
                .size(13.0)
                .color(if ok {
                    Color32::from_rgb(60, 150, 80)
                } else {
                    Color32::from_rgb(190, 60, 60)
                }),
        );
        let mut next = false;
        ui.horizontal(|ui| {
            next = ui
                .button(if sess.at_last() { "看结果" } else { "下一题" })
                .clicked();
        });
        if next {
            sess.next();
            page.input.clear();
            page.slots.clear();
            page.drag = None;
        }
    }
    page.session = Some(sess);
}

/// 选择题的选项按钮。答完之前点选项即作答，答完后标出正确答案与选错的
///
/// **自己画而不是用 Button**：egui 0.36 的 `add_sized` 是「占这么大地方、把控件居中放进去」，
/// 不是把控件撑满——用 Button 的话按钮只有文字那么宽、还居中在右边，一排选项参差不齐。
fn draw_options(
    ui: &mut egui::Ui,
    options: &[String],
    answer: usize,
    answered: bool,
    sess: &mut QuizSession,
    idx: usize,
) {
    let mut picked: Option<usize> = None;
    for (i, opt) in options.iter().enumerate() {
        let mut col = Color32::from_rgb(250, 250, 248);
        if answered {
            if i == answer {
                col = Color32::from_rgb(215, 240, 215);
            } else if matches!(sess.answers[idx], Answer::Choice(x) if x == i) {
                col = Color32::from_rgb(250, 220, 220);
            }
        }
        // 用 max_rect 而不是 available_width：上面那行设置是折行排的，
        // 折行之后 available_width 只剩最后一行那么宽，选项会被挤成一小条
        let w = ui.max_rect().width();
        if i == 0 {
            tracing::debug!("选项区：max_rect 宽 {w:.0}，available 宽 {:.0}", ui.available_width());
        }
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::click());
        if resp.hovered() && !answered {
            col = Color32::from_rgb(235, 238, 245);
        }
        ui.painter().rect_filled(rect, 4.0, col);
        ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, Color32::from_gray(205)), egui::StrokeKind::Inside);
        let font = egui::FontId { size: 14.0, family: egui::FontFamily::Proportional };
        ui.painter().text(
            rect.left_center() + egui::vec2(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            opt,
            font,
            Color32::from_gray(30),
        );
        if resp.clicked() && !answered {
            picked = Some(i);
        }
        ui.add_space(4.0);
    }
    if let Some(i) = picked {
        sess.submit(Answer::Choice(i));
    }
}

/// 拖曳配对题：左边一列英文词，右边一列中文释义
fn draw_pair(page: &mut VocabPage, ui: &mut egui::Ui, sess: &mut QuizSession, left: &[String], right: &[String]) {
    ui.label(RichText::new("把左边的英文词拖到右边的中文意思上").size(15.0).strong());
    ui.add_space(6.0);
    if page.slots.len() != right.len() {
        page.slots = vec![None; right.len()];
    }

    // 槽位矩形要留到松手时判落点
    let mut slot_rects: Vec<egui::Rect> = Vec::new();
    ui.horizontal_top(|ui| {
        // 左边一列：英文词（可拖）
        //
        // **不用 Label**：Label 的可交互范围只有文字本身（它按字形大小分配矩形），
        // 抓偏一点就拖不动。这里自己分配一块 160x28 的矩形当拖动热区，文字另画。
        ui.vertical(|ui| {
            for (i, w) in left.iter().enumerate() {
                let dragging = page.drag == Some(i);
                let color = if dragging {
                    Color32::from_rgb(40, 80, 160)
                } else {
                    Color32::from_gray(30)
                };
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(160.0, 28.0), egui::Sense::drag());
                let font = egui::FontId { size: 13.0, family: egui::FontFamily::Proportional };
                ui.painter()
                    .text(rect.left_center(), egui::Align2::LEFT_CENTER, w, font, color);
                if resp.drag_started() {
                    tracing::info!("拖曳开始：{w}");
                    page.drag = Some(i);
                }
                ui.add_space(3.0);
            }
        });
        ui.add_space(16.0);
        // 右边一列：中文释义（落点）
        ui.vertical(|ui| {
            for (j, m) in right.iter().enumerate() {
                let placed = page.slots.get(j).copied().flatten().and_then(|i| left.get(i));
                let text = match placed {
                    Some(w) => format!("{m}   ← {w}"),
                    None => m.clone(),
                };
                let resp = ui.add_sized(
                    [280.0, 28.0],
                    egui::Button::new(RichText::new(text).size(13.0)),
                );
                slot_rects.push(resp.rect);
                ui.add_space(3.0);
            }
        });
    });

    // 松手：落在哪个槽就配到哪个槽
    // `interact_pos` 是松手那一刻的鼠标位置（egui 在按下/移动/松开时都会更新它）
    if ui.ctx().input(|i| i.pointer.any_released()) {
        if let Some(i) = page.drag.take() {
            let pos = ui.ctx().input(|i| i.pointer.interact_pos());
            let hit = pos.and_then(|p| slot_rects.iter().position(|r| r.contains(p)));
            tracing::info!(
                "拖曳松手：词 {:?} 位置 {:?} 命中槽 {:?}",
                left.get(i),
                pos.map(|p| (p.x, p.y)),
                hit
            );
            if let Some(j) = hit {
                // 同一个词只能占一个槽
                for s in page.slots.iter_mut() {
                    if *s == Some(i) {
                        *s = None;
                    }
                }
                page.slots[j] = Some(i);
            }
        }
    }

    // 全放满了就判分
    if !sess.is_answered() && page.slots.iter().all(|s| s.is_some()) {
        sess.submit(Answer::Pair(page.slots.clone()));
    }
}

fn kind_label(q: &Question) -> &'static str {
    match q {
        Question::Choice { to_english: true, .. } => "单选（中→英）",
        Question::Choice { .. } => "单选（英→中）",
        Question::Pair { .. } => "拖曳配对",
        Question::Spell { .. } => "拼写",
        Question::Listen { .. } => "听音选义",
    }
}
