//! 译文面板的状态与摆放计算（与 egui 无关的部分，可单测）
use text_capture::Source;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// 取词中 / 翻译中
    Working(String),
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct PanelState {
    /// 原文
    pub original: String,
    /// 译文（流式过程中逐步变长）
    pub translated: String,
    pub source: Source,
    pub phase: Phase,
    /// 面板左上角（虚拟屏坐标）
    pub pos: (i32, i32),
    /// 钉住后不自动关闭，可拖动
    pub pinned: bool,
    pub opened_at: Instant,
    pub scene_label: &'static str,
}

impl PanelState {
    pub fn new(scene_label: &'static str, source: Source, pos: (i32, i32)) -> PanelState {
        PanelState {
            original: String::new(),
            translated: String::new(),
            source,
            phase: Phase::Working("正在取词…".into()),
            pos,
            pinned: false,
            opened_at: Instant::now(),
            scene_label,
        }
    }

    pub fn is_working(&self) -> bool {
        matches!(self.phase, Phase::Working(_))
    }

    /// 该不该自动关闭：没钉住、已出结果、鼠标离开够久
    pub fn should_auto_close(&self, mouse_outside_for: Option<Duration>, grace: Duration) -> bool {
        if self.pinned || self.is_working() {
            return false;
        }
        matches!(mouse_outside_for, Some(d) if d >= grace)
    }
}

/// 把面板摆在触发点旁边，且不出屏幕。优先放右下，放不下依次尝试左下、右上、左上。
pub fn place(
    anchor: (i32, i32),
    size: (u32, u32),
    screen: (i32, i32, u32, u32),
    gap: i32,
) -> (i32, i32) {
    let (ax, ay) = anchor;
    let (w, h) = (size.0 as i32, size.1 as i32);
    let (sx, sy, sw, sh) = (screen.0, screen.1, screen.2 as i32, screen.3 as i32);
    let (right, bottom) = (sx + sw, sy + sh);

    let candidates = [
        (ax + gap, ay + gap),         // 右下
        (ax - gap - w, ay + gap),     // 左下
        (ax + gap, ay - gap - h),     // 右上
        (ax - gap - w, ay - gap - h), // 左上
    ];
    for (x, y) in candidates {
        if x >= sx && y >= sy && x + w <= right && y + h <= bottom {
            return (x, y);
        }
    }
    // 四个方位都放不下：硬收敛到屏幕内
    let x = (ax + gap).min(right - w).max(sx);
    let y = (ay + gap).min(bottom - h).max(sy);
    (x, y)
}

/// 小图标位置：选区结束点的右下角
pub fn place_icon(anchor: (i32, i32), size: u32, screen: (i32, i32, u32, u32)) -> (i32, i32) {
    place(anchor, (size, size), screen, 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32, u32, u32) = (0, 0, 1920, 1080);

    #[test]
    fn places_bottom_right_when_there_is_room() {
        assert_eq!(place((100, 100), (400, 300), SCREEN, 12), (112, 112));
    }

    #[test]
    fn flips_left_when_right_edge_is_close() {
        let (x, _) = place((1900, 100), (400, 300), SCREEN, 12);
        assert_eq!(x, 1900 - 12 - 400);
    }

    #[test]
    fn flips_up_when_bottom_edge_is_close() {
        let (_, y) = place((100, 1070), (400, 300), SCREEN, 12);
        assert_eq!(y, 1070 - 12 - 300);
    }

    #[test]
    fn clamps_when_nothing_fits() {
        let (x, y) = place((10, 10), (1900, 1070), SCREEN, 12);
        assert!(x >= 0 && y >= 0);
        assert!(x + 1900 <= 1920 && y + 1070 <= 1080);
    }

    #[test]
    fn works_on_secondary_monitor_with_negative_origin() {
        let screen = (-1920, -200, 1920, 1080);
        let (x, y) = place((-1900, -180), (400, 300), screen, 12);
        assert!(x >= -1920 && y >= -200, "({x},{y})");
    }

    #[test]
    fn auto_close_respects_pin_and_working() {
        let mut p = PanelState::new("划词翻译", Source::Clipboard, (0, 0));
        assert!(!p.should_auto_close(Some(Duration::from_secs(5)), Duration::from_millis(500)), "翻译中不关");
        p.phase = Phase::Done;
        assert!(p.should_auto_close(Some(Duration::from_millis(600)), Duration::from_millis(500)));
        assert!(!p.should_auto_close(Some(Duration::from_millis(100)), Duration::from_millis(500)));
        assert!(!p.should_auto_close(None, Duration::from_millis(500)), "鼠标在面板里不关");
        p.pinned = true;
        assert!(!p.should_auto_close(Some(Duration::from_secs(60)), Duration::from_millis(500)));
    }
}
