//! 主窗口「翻译」页：手动输入单词或句子来翻。
//!
//! 这里只放状态与事件处理（纯逻辑、有测试），画界面在 `ui::App::settings_translate`。
//! 事件处理与浮动卡片那套（`ui::App::drain_events`）口径一致：离线释义先顶着、
//! 模型回来再替换，模型失败时离线释义留着不盖红字。

use crate::engine::Event;
use ai_translate::{Direction, Scene};
use std::time::{Duration, Instant};

/// 停止输入多久之后查本地词库。太短会每敲一个字母查一次、结果闪来闪去
pub const LIVE_DELAY: Duration = Duration::from_millis(250);
/// 「最近查过」最多留几条（只在内存里，不落盘——项目不做翻译历史）
pub const RECENT_MAX: usize = 8;

/// 边输入边查的结果
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// 查的是哪个词（输入框再变就作废）
    pub query: String,
    /// 行标记格式的词条；None = 本地词库里没有
    pub markers: Option<String>,
}

/// 结果区此刻该画什么
#[derive(Debug, Clone, PartialEq)]
pub enum View {
    /// 输入框空：只给「最近查过」
    Empty,
    /// 本地词库查到了，还没问模型
    Live(String),
    /// 是个词，但本地词库里没有：提示回车问模型
    LiveMiss,
    /// 句子（或本机没有词库）：提示回车翻译
    Hint,
    /// 上一次提交的结果（进行中或已完成），且输入框没改过
    Result,
}

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
    /// 输入框上次变化的时刻；过了 `LIVE_DELAY` 就查一次本地词库
    pub edited_at: Option<Instant>,
    pub live: Option<Live>,
    /// 最近提交过的原文，新的在前
    pub recent: Vec<String>,
    /// 「复制」按钮刚点过（按钮文字变成「已复制」一会儿）
    pub copied_at: Option<Instant>,
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
            edited_at: None,
            live: None,
            recent: Vec::new(),
            copied_at: None,
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
        self.recent.retain(|r| r != &self.original);
        self.recent.insert(0, self.original.clone());
        self.recent.truncate(RECENT_MAX);
        self.edited_at = None;
        self.dir = cfg_dir.for_input(&self.original);
        self.scene = Scene::Selection;
        self.result.clear();
        self.offline = false;
        self.phase = Phase::Working;
    }

    /// 清空输入与结果。「最近查过」留着——清空是为了查下一个，不是抹掉记录
    pub fn clear(&mut self) {
        let recent = std::mem::take(&mut self.recent);
        *self = ManualState { want_focus: true, recent, ..ManualState::default() };
    }

    /// 输入框变了：记下时刻，等停手再查；之前的边查结果作废
    pub fn edited(&mut self, now: Instant) {
        self.edited_at = Some(now);
        self.live = None;
    }

    /// 现在该不该查本地词库了；该查就返回要查的词（只认英文单词/短语——词库是英汉的）
    pub fn due_live_query(&mut self, now: Instant) -> Option<String> {
        let at = self.edited_at?;
        if now.duration_since(at) < LIVE_DELAY {
            return None;
        }
        self.edited_at = None;
        let q = self.input.trim();
        ai_translate::scene::looks_like_word_in(q, Direction::ToChinese).then(|| q.to_string())
    }

    /// 结果区该画什么
    pub fn view(&self) -> View {
        let q = self.input.trim();
        if q.is_empty() {
            return View::Empty;
        }
        // 输入框还是提交时那段：显示提交的结果（模型的完整词条优先于边查的本地释义）
        if self.phase != Phase::Idle && q == self.original {
            return View::Result;
        }
        match &self.live {
            Some(l) if l.query == q => match &l.markers {
                Some(m) => View::Live(m.clone()),
                None => View::LiveMiss,
            },
            // 句子，或者还在等停手：先别闪「没查到」，上一次提交的结果也别留着误导
            _ => View::Hint,
        }
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
            Event::Skipped { .. } | Event::Shot { .. } | Event::Line { .. } | Event::OverlayFailed { .. } => {}
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
    fn live_lookup_waits_for_the_typing_to_pause_and_only_takes_english_words() {
        let t0 = Instant::now();
        let mut m = ManualState { input: "resilient".into(), ..Default::default() };
        m.edited(t0);
        assert_eq!(m.due_live_query(t0 + Duration::from_millis(100)), None, "还在打字，别查");
        assert_eq!(m.due_live_query(t0 + LIVE_DELAY).as_deref(), Some("resilient"));
        assert_eq!(m.due_live_query(t0 + LIVE_DELAY * 2), None, "查过一次就不再查，等下次输入");
        let mut zh = ManualState { input: "今天天气很好".into(), ..Default::default() };
        zh.edited(t0);
        assert_eq!(zh.due_live_query(t0 + LIVE_DELAY), None, "词库是英汉的，中文与句子不查");
    }

    #[test]
    fn view_prefers_live_entry_until_submitted_then_the_result() {
        let mut m = ManualState::default();
        assert_eq!(m.view(), View::Empty);
        m.input = "run".into();
        m.live = Some(Live { query: "run".into(), markers: Some("#词 run".into()) });
        assert_eq!(m.view(), View::Live("#词 run".into()));
        m.begin(3, Direction::ToChinese);
        assert_eq!(m.view(), View::Result, "提交后显示模型那一路（离线释义也会从那一路先到）");
        m.input = "runner".into();
        m.edited(Instant::now());
        assert_eq!(m.view(), View::Hint, "改了输入，旧结果不能留着冒充新词的释义");
        m.live = Some(Live { query: "runner".into(), markers: None });
        assert_eq!(m.view(), View::LiveMiss);
        m.input = "It is fine.".into();
        assert_eq!(m.view(), View::Hint);
    }

    #[test]
    fn recent_keeps_newest_first_without_duplicates_and_survives_clear() {
        let mut m = ManualState::default();
        for w in ["a", "b", "a"] {
            m.input = w.into();
            m.begin(1, Direction::ToChinese);
        }
        assert_eq!(m.recent, vec!["a".to_string(), "b".to_string()]);
        m.clear();
        assert_eq!(m.recent.len(), 2, "清空只清输入与结果");
        for i in 0..20 {
            m.input = format!("w{i}");
            m.begin(1, Direction::ToChinese);
        }
        assert_eq!(m.recent.len(), RECENT_MAX);
    }

    #[test]
    fn sentence_result_is_not_parsed_as_dictionary() {
        let mut m = working("It is a fine day.");
        m.on_event(Event::Done { seq: 7, text: "#今天 天气很好".into(), took_ms: 1 });
        assert!(m.entry().is_none(), "句子场景里 # 开头的译文不能被当成词典条目");
    }
}
