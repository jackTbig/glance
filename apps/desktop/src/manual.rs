//! 主窗口「翻译」页：手动输入单词或句子来翻。
//!
//! 这里只放状态与事件处理（纯逻辑、有测试），画界面在 `ui::App::settings_translate`。
//! 事件处理与浮动卡片那套（`ui::App::drain_events`）口径一致：离线释义先顶着、
//! 模型回来再替换，模型失败时离线释义留着不盖红字。

use crate::engine::Event;
use ai_translate::{Direction, Scene};

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Idle,
    Working,
    Done { took_ms: u128 },
    /// 离线释义已在屏上、模型没回来：给提示，不当失败
    Notice(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ManualState {
    /// 输入框里的内容
    pub input: String,
    /// 正在等的那次任务的序号；0 = 没有在等
    pub seq: u64,
    /// 这次提交的原文（输入框之后可能被改，结果区按提交时的原文显示）
    pub original: String,
    pub scene: Scene,
    pub dir: Direction,
    /// 译文或词典条目（行标记格式）
    pub result: String,
    /// 结果区显示的是离线词典的释义
    pub offline: bool,
    pub phase: Phase,
    /// 下一帧把键盘焦点给输入框（打开窗口、点清空之后）
    pub want_focus: bool,
    /// 下一帧自动提交一次（只给调试钩子 TT_MAIN_INPUT 用）
    pub autosubmit: bool,
}

impl Default for ManualState {
    fn default() -> Self {
        ManualState {
            input: String::new(),
            seq: 0,
            original: String::new(),
            scene: Scene::Selection,
            dir: Direction::ToChinese,
            result: String::new(),
            offline: false,
            phase: Phase::Idle,
            want_focus: true,
            autosubmit: false,
        }
    }
}

impl ManualState {
    /// 能不能提交：空白不提交，正在翻的同一段不重复提交
    pub fn can_submit(&self) -> bool {
        let t = self.input.trim();
        !t.is_empty() && !(self.phase == Phase::Working && t == self.original)
    }

    /// 提交一次：记下序号与原文，清掉上一次的结果。方向由配置的方向按内容翻转（与引擎同一个判据）
    pub fn begin(&mut self, seq: u64, cfg_dir: Direction) {
        self.seq = seq;
        self.original = self.input.trim().to_string();
        self.dir = cfg_dir.for_input(&self.original);
        self.scene = Scene::Selection;
        self.result.clear();
        self.offline = false;
        self.phase = Phase::Working;
    }

    pub fn clear(&mut self) {
        *self = ManualState { want_focus: true, ..ManualState::default() };
    }

    /// 这个事件是不是给主窗口的
    pub fn owns(&self, seq: u64) -> bool {
        self.seq != 0 && seq == self.seq
    }

    pub fn on_event(&mut self, e: Event) {
        match e {
            Event::Captured { scene, .. } => self.scene = scene,
            Event::Offline { text, .. } => {
                if self.result.trim().is_empty() {
                    self.result = text;
                    self.offline = true;
                }
            }
            Event::Delta { text, .. } => {
                self.result = text;
                self.offline = false;
            }
            Event::Done { text, took_ms, .. } => {
                self.result = text;
                self.offline = false;
                self.phase = Phase::Done { took_ms };
            }
            Event::Failed { error, .. } => {
                self.phase = if self.offline {
                    Phase::Notice(format!("模型没回来，显示的是离线词典的释义（{error}）"))
                } else {
                    Phase::Failed(error)
                };
            }
            Event::Nothing { reason, .. } => self.phase = Phase::Failed(reason),
            // 手动输入不会走截屏覆盖，也不会被「遇目标语言跳过」
            Event::Skipped { .. } | Event::Shot { .. } | Event::Line { .. } => {}
        }
    }

    /// 结果是词典条目时解析出来；普通译文返回 None
    pub fn entry(&self) -> Option<crate::dict::Entry> {
        (self.scene == Scene::Word).then(|| crate::dict::parse(&self.result)).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn working(input: &str) -> ManualState {
        let mut m = ManualState { input: input.into(), ..Default::default() };
        m.begin(7, Direction::ToChinese);
        m
    }

    #[test]
    fn direction_follows_the_input() {
        assert_eq!(working("hello world").dir, Direction::ToChinese);
        assert_eq!(working("你好世界").dir, Direction::ToEnglish);
    }

    #[test]
    fn only_its_own_events_are_taken() {
        let m = working("hi");
        assert!(m.owns(7));
        assert!(!m.owns(8));
        assert!(!ManualState::default().owns(0), "没提交过时序号 0 不能认领任何事件");
    }

    #[test]
    fn blank_or_same_pending_text_is_not_resubmitted() {
        assert!(!ManualState { input: "  \n".into(), ..Default::default() }.can_submit());
        let m = working("hello");
        assert!(!m.can_submit(), "同一段正在翻，别重复提交");
        let mut m2 = m.clone();
        m2.input = "hello there".into();
        assert!(m2.can_submit());
    }

    /// 与浮动卡片同一口径：离线先顶着，模型回来替换；模型失败时离线内容留着
    #[test]
    fn offline_entry_is_replaced_by_model_and_kept_on_failure() {
        let mut m = working("run");
        m.on_event(Event::Captured { seq: 7, text: "run".into(), source: text_capture::Source::Clipboard, scene: Scene::Word, took_ms: 0 });
        m.on_event(Event::Offline { seq: 7, text: "#词 run\n#释 v. 跑".into() });
        assert!(m.offline);
        assert!(m.entry().is_some());
        let mut failed = m.clone();
        failed.on_event(Event::Failed { seq: 7, error: "超时".into() });
        assert!(matches!(failed.phase, Phase::Notice(_)), "离线释义在屏上时模型失败不该变红字");
        assert!(!failed.result.is_empty());
        m.on_event(Event::Done { seq: 7, text: "#词 run\n#释 v. 跑；运行".into(), took_ms: 900 });
        assert!(!m.offline);
        assert_eq!(m.phase, Phase::Done { took_ms: 900 });
    }

    #[test]
    fn sentence_result_is_not_parsed_as_dictionary() {
        let mut m = working("It is a fine day.");
        m.on_event(Event::Done { seq: 7, text: "#今天 天气很好".into(), took_ms: 1 });
        assert!(m.entry().is_none(), "句子场景里 # 开头的译文不能被当成词典条目");
    }
}
