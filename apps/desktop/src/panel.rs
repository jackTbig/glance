//! 译文面板的状态与摆放计算（与 egui 无关的部分，可单测）
use text_capture::Source;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// 取词中 / 翻译中
    Working(String),
    Done,
    Failed(String),
    /// 不是错误，只是告诉用户为什么没翻（比如进程在例外名单里）。没有重试按钮
    Notice(String),
}

#[derive(Debug, Clone)]
pub struct PanelState {
    /// 原文
    pub original: String,
    /// 译文（流式过程中逐步变长）
    pub translated: String,
    pub source: Source,
    pub phase: Phase,
    /// 触发点（虚拟屏物理坐标）。**摆放永远以它为基准算**，
    /// 拿上一帧摆出来的 pos 当锚点会每帧再加一次间距，面板会以每秒几百像素往右下爬走。
    pub anchor: (i32, i32),
    /// 上一帧实际摆到的左上角（虚拟屏物理坐标）；钉住后就固定用它
    pub pos: (i32, i32),
    /// 钉住后不自动关闭，可拖动
    pub pinned: bool,
    pub opened_at: Instant,
    pub scene_label: &'static str,
}

impl PanelState {
    pub fn new(scene_label: &'static str, source: Source, anchor: (i32, i32)) -> PanelState {
        PanelState {
            original: String::new(),
            translated: String::new(),
            source,
            phase: Phase::Working("正在取词…".into()),
            anchor,
            pos: anchor,
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

/// 小图标位置：选区结束点的右下角。
/// `size` 与 `gap` 都是**物理**像素，调用方负责按当前缩放比换算（见 `dpi::Dpi`）。
pub fn place_icon(anchor: (i32, i32), size: u32, screen: (i32, i32, u32, u32), gap: i32) -> (i32, i32) {
    place(anchor, (size, size), screen, gap)
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
    fn icon_sits_just_off_the_release_point() {
        assert_eq!(place_icon((1680, 393), 60, (0, 0, 3072, 1920), 8), (1688, 401));
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

    /// 回归：界面每帧都会把摆放结果记回 `pos`。要是摆放拿 `pos` 当锚点，
    /// 每帧都会再加一次间距，面板会以每秒几百像素往右下爬走、绕屏一圈——
    /// 用户看到的就是「翻译完了但屏幕上什么都没有」。
    #[test]
    fn repeated_placement_does_not_drift() {
        const BIG: (i32, i32, u32, u32) = (0, 0, 3072, 1920);
        let mut p = PanelState::new("悬停翻译", Source::Uia, (1633, 358));
        let mut last: Option<(i32, i32)> = None;
        for frame in 0..40 {
            let at = place(p.anchor, (840, 920), BIG, 28);
            p.pos = at;
            if let Some(prev) = last {
                assert_eq!(prev, at, "第 {frame} 帧面板挪了位");
            }
            last = Some(at);
        }
        assert_eq!(last, Some((1661, 386)));
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
