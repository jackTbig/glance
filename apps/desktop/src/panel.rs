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
    /// 正在拖动：记下按下那一刻的（鼠标物理坐标，面板左上角物理坐标）。
    /// 拖动中按「当前鼠标 - 起点鼠标」算绝对位移，**不用 egui 的逐帧 drag_delta**：
    /// 那是窗口内的逻辑坐标，而窗口本身正跟着鼠标走，每帧还要取整成物理像素，
    /// 误差攒下来卡片会越拖越落后于鼠标。
    pub drag_from: Option<((i32, i32), (i32, i32))>,
    /// 上一帧量到的内容高度（逻辑像素）。0 = 还没量过，先按配置的最大高度开。
    /// 卡片高度跟着内容走，短词条下面才不会拖一大截空白。
    pub view_h: f32,
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
            drag_from: None,
            view_h: 0.0,
            opened_at: Instant::now(),
            scene_label,
        }
    }

    pub fn is_working(&self) -> bool {
        matches!(self.phase, Phase::Working(_))
    }

    /// 拖动中的新左上角（全物理坐标）：起点面板位置 + 鼠标相对起点的位移。
    /// 每帧都从起点重算而不是累加，所以窗口跟手慢一帧也不会攒出偏差。
    pub fn drag_to(&self, cursor: (i32, i32)) -> Option<(i32, i32)> {
        let (c0, p0) = self.drag_from?;
        Some((p0.0 + cursor.0 - c0.0, p0.1 + cursor.1 - c0.1))
    }

    /// 该不该自动关闭：没钉住、没在拖、已出结果、鼠标离开够久
    pub fn should_auto_close(&self, mouse_outside_for: Option<Duration>, grace: Duration) -> bool {
        if self.pinned || self.drag_from.is_some() || self.is_working() {
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

    /// 面板摆放走的是「物理坐标系里算 → 换成逻辑坐标设窗口」这条链，
    /// 这里把整条链走一遍，确认 200% 下面板落在鼠标旁边而不是两倍远处
    #[test]
    fn panel_placement_chain_stays_next_to_cursor() {
        let d = desktop_shell::dpi::Dpi::new(2.0);
        let screen = (0, 0, 3072u32, 1920u32);
        let cursor = (1000, 600); // 物理
        let size_phys = d.to_physical_size((420.0, 460.0));
        let gap_phys = d.to_physical_len(14.0);
        let placed = place(cursor, size_phys, screen, gap_phys);
        assert_eq!(placed, (1028, 628), "摆放算在物理坐标系里");
        let logical = d.to_logical_pos(placed);
        assert_eq!(logical, egui::pos2(514.0, 314.0));
        // 逻辑坐标乘回缩放比就是窗口真正出现的物理位置，应当紧挨鼠标
        let back = d.to_physical_pos(logical);
        assert!((back.0 - cursor.0).abs() <= 40 && (back.1 - cursor.1).abs() <= 40, "{back:?} 应挨着 {cursor:?}");
    }

    /// 边界收敛：鼠标贴着右下角时，换算完的窗口整体仍在屏幕内
    #[test]
    fn panel_stays_on_screen_near_bottom_right() {
        let d = desktop_shell::dpi::Dpi::new(2.0);
        let screen = (0, 0, 3072u32, 1920u32);
        let size_phys = d.to_physical_size((420.0, 460.0));
        let placed = place((3060, 1910), size_phys, screen, d.to_physical_len(14.0));
        let logical = d.to_logical_pos(placed);
        assert!(logical.x >= 0.0 && logical.y >= 0.0, "{logical:?}");
        // 逻辑屏是 1536x960，面板 420x460 必须整块装得下
        assert!(logical.x + 420.0 <= 1536.0 + 0.5, "{logical:?}");
        assert!(logical.y + 460.0 <= 960.0 + 0.5, "{logical:?}");
    }

    #[test]
    fn dragging_follows_the_cursor_without_drift() {
        let mut p = PanelState::new("取词释义", Source::Uia, (500, 400));
        assert_eq!(p.drag_to((510, 410)), None, "没按下就不该动");
        p.drag_from = Some(((500, 400), (480, 360)));
        // 分多帧走到同一个终点，结果必须和一步到位完全一致（这正是不用逐帧累加的理由）
        for c in [(505, 402), (540, 430), (620, 500)] {
            assert_eq!(p.drag_to(c), Some((480 + c.0 - 500, 360 + c.1 - 400)));
        }
        assert_eq!(p.drag_to((620, 500)), Some((600, 460)));
    }

    /// 用户报过的 bug：悬停取词「显示完成后马上消失」。
    ///
    /// 成因是「鼠标离开面板」的计时从面板弹出就起跑，而悬停时鼠标本来就停在原文上、
    /// 不在面板里；翻译要一两秒，等译文到手宽限期早已耗光，于是 `Done` 的那一帧就满足关闭条件。
    /// 修法是出结果时把计时清零（`ui::drain_events` 里的 `done`），这里钉住那个语义。
    #[test]
    fn result_gets_a_full_grace_period_even_if_mouse_never_entered() {
        let grace = Duration::from_millis(3000);
        let mut p = PanelState::new("悬停翻译", Source::Uia, (0, 0));

        // 翻译中：鼠标一直在外面待了很久也不能关
        assert!(!p.should_auto_close(Some(Duration::from_secs(5)), grace), "翻译中不能关");

        // 出结果的那一刻，调用方会把计时清零 —— 传 None 表示「刚清零，还没开始算」
        p.phase = Phase::Done;
        assert!(!p.should_auto_close(None, grace), "刚出结果就关正是用户报的 bug");

        // 清零之后重新计时，没到宽限期仍不关
        assert!(!p.should_auto_close(Some(grace / 2), grace));
        // 够久了才关
        assert!(p.should_auto_close(Some(grace), grace));
    }

    #[test]
    fn dragging_holds_the_card_open() {
        let mut p = PanelState::new("取词释义", Source::Uia, (0, 0));
        p.phase = Phase::Done;
        let grace = Duration::from_millis(500);
        assert!(p.should_auto_close(Some(Duration::from_secs(2)), grace));
        p.drag_from = Some(((0, 0), (0, 0)));
        assert!(!p.should_auto_close(Some(Duration::from_secs(2)), grace), "拖动中不能自己收掉");
    }

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
