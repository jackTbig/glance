//! 译文面板的状态与摆放计算（与 egui 无关的部分，可单测）
use text_capture::Source;
use std::time::Instant;

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
    /// 用户亲手拖过这张卡片。拖过之后就不再跟着截屏框对齐，用户放哪儿就在哪儿
    pub user_moved: bool,
    /// 当前显示的是离线词典的释义（模型还没回来）。卡片上要标出来，
    /// 否则用户分不清「这就是结果」还是「还在等」
    pub offline: bool,
    /// 这张卡片的内容来自屏幕上的哪一块（虚拟屏物理矩形）。**只有截屏翻译有**。
    /// 有值时卡片与框选区同宽、左边缘对齐（Jack 2026-09-13 要求）。
    pub source_rect: Option<(i32, i32, u32, u32)>,
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
            user_moved: false,
            offline: false,
            source_rect: None,
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

}

/// 物理坐标点是否落在浮窗矩形里。浮窗「点外面才关」的判据（Jack 2026-09-24 定，不再计时自收）。
pub fn contains_phys(rect: (i32, i32, u32, u32), x: i32, y: i32) -> bool {
    x >= rect.0 && y >= rect.1 && x <= rect.0 + rect.2 as i32 && y <= rect.1 + rect.3 as i32
}

/// 点是否落在浮窗矩形外扩 margin 的范围内。贴着窗边的点击多半是瞄它没瞄中
/// （Jack 2026-09-25 实测偏 61 物理像素），这种近失不关窗，直接忽略。
pub fn near_phys(rect: (i32, i32, u32, u32), x: i32, y: i32, margin: i32) -> bool {
    let (rx, ry, rw, rh) = rect;
    let w = (rw as i32 + margin * 2).max(0) as u32;
    let h = (rh as i32 + margin * 2).max(0) as u32;
    contains_phys((rx - margin, ry - margin, w, h), x, y)
}

/// 把面板摆在触发点旁边，且不出屏幕。优先放右下，放不下依次尝试左下、右上、左上。
/// 卡片与截屏框对齐：宽度取框宽（夹在 `min_w`..`max_w` 之间——框太窄的话卡片里一行放不下几个字），
/// 左边缘对齐框的左边缘，超出屏幕右边就收回屏内。纵向位置沿用 `place` 算好的。
///
/// 返回（左上角物理坐标，卡片物理宽度）。
pub fn align_to_region(
    region: (i32, i32, u32, u32),
    placed: (i32, i32),
    min_w: u32,
    max_w: u32,
    screen: (i32, i32, u32, u32),
) -> ((i32, i32), u32) {
    let (rx, _ry, rw, _rh) = region;
    let (sx, _sy, sw, _sh) = screen;
    let w = rw.clamp(min_w.min(max_w), max_w);
    let right = sx + sw as i32;
    let x = rx.min(right - w as i32).max(sx);
    ((x, placed.1), w)
}

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

    // 右上优先（Jack 2026-09-24 定）：卡片弹在鼠标指针的右上角，不挡他正在读的那行。
    // 上方放不下退到同侧下方，右侧放不下退到同高度左侧。
    let candidates = [
        (ax + gap, ay - gap - h),     // 右上
        (ax + gap, ay + gap),         // 右下
        (ax - gap - w, ay - gap - h), // 左上
        (ax - gap - w, ay + gap),     // 左下
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

/// 摆放候选序：下方优先。给锚点不是鼠标的卡片用（非覆盖式截屏卡的锚点是选区左下角，
/// 用「右上优先」会让卡片盖住选区底部，审核 M2，2026-09-25 修）。
pub fn place_below(
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
    let x = (ax + gap).min(right - w).max(sx);
    let y = (ay + gap).min(bottom - h).max(sy);
    (x, y)
}

/// 小图标位置：选区结束点的**右下角**——图标要贴在刚选完的文字尾端下面，
/// 与卡片的「右上优先」相反，所以自己列候选顺序，不跟 `place` 走。
/// `size` 与 `gap` 都是**物理**像素，调用方负责按当前缩放比换算（见 `dpi::Dpi`）。
pub fn place_icon(anchor: (i32, i32), size: u32, screen: (i32, i32, u32, u32), gap: i32) -> (i32, i32) {
    let (ax, ay) = anchor;
    let (w, h) = (size as i32, size as i32);
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
    let x = (ax + gap).min(right - w).max(sx);
    let y = (ay + gap).min(bottom - h).max(sy);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 截屏卡片要与框同宽、左边缘对齐——用户框哪块，译文就压在那块下面同样宽
    #[test]
    fn card_matches_the_screenshot_box() {
        let screen = (0, 0, 3072, 1920);
        let ((x, y), w) = align_to_region((400, 300, 800, 200), (517, 514), 240, 2000, screen);
        assert_eq!((x, w), (400, 800), "左边缘与宽度都跟着框走");
        assert_eq!(y, 514, "纵向位置沿用 place 算好的");
    }

    /// 卡片默认弹在指针右上角（Jack 2026-09-24）；上方或右侧放不下才换方位
    #[test]
    fn card_goes_to_the_upper_right_of_the_cursor() {
        let placed = place((1000, 600), (200, 100), SCREEN, 10);
        assert_eq!(placed, (1010, 490), "右上：x 在指针右、y 在指针上");
        // 贴着屏幕上边：上方放不下，退到右下
        let placed = place((1000, 40), (200, 100), SCREEN, 10);
        assert_eq!(placed, (1010, 50));
        // 贴着屏幕右边：右侧放不下，退到左上
        let placed = place((1900, 600), (200, 100), SCREEN, 10);
        assert_eq!(placed, (1690, 490));
    }

    /// 小图标仍贴选区尾端的右下角，不跟卡片的右上优先走
    #[test]
    fn icon_stays_at_the_lower_right_of_the_selection() {
        assert_eq!(place_icon((1000, 600), 32, SCREEN, 10), (1010, 610));
    }

    #[test]
    fn card_stays_readable_and_on_screen() {
        let screen = (0, 0, 3072, 1920);
        // 框太窄：卡片不跟着缩到读不了
        let ((_, _), w) = align_to_region((100, 100, 60, 40), (0, 0), 240, 2000, screen);
        assert_eq!(w, 240);
        // 框太宽：夹到上限
        let ((_, _), w) = align_to_region((0, 0, 3000, 100), (0, 0), 240, 2000, screen);
        assert_eq!(w, 2000);
        // 靠右的框：卡片收回屏内，不许探出右边
        let ((x, _), w) = align_to_region((2900, 100, 400, 100), (0, 0), 240, 2000, screen);
        assert!(x + w as i32 <= 3072, "卡片右边缘 {} 超出屏幕", x + w as i32);
    }

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
    fn region_card_stays_below_the_selection() {
        // 锚点是选区左下角：卡片必须落在选区下面，不许盖原文
        let placed = place_below((100, 500), (400, 300), SCREEN, 12);
        assert_eq!(placed, (112, 512));
        // 贴屏幕底边才翻到上方
        let placed = place_below((100, 1070), (400, 300), SCREEN, 12);
        assert_eq!(placed, (112, 758));
    }

    #[test]
    fn near_miss_within_margin_does_not_count_as_outside() {
        let r = (100, 200, 300, 400);
        assert!(near_phys(r, 90, 400, 20), "外扩范围内算近失");
        assert!(near_phys(r, 250, 610, 20));
        assert!(!near_phys(r, 79, 400, 20), "超出 margin 才算真外面");
        assert!(!near_phys(r, 250, 621, 20));
    }

    #[test]
    fn outside_click_uses_the_window_rect() {
        let r = (100, 200, 300, 400);
        assert!(contains_phys(r, 100, 200), "左上角算里面");
        assert!(contains_phys(r, 400, 600), "右下边缘算里面");
        assert!(contains_phys(r, 250, 400));
        assert!(!contains_phys(r, 99, 400));
        assert!(!contains_phys(r, 401, 400));
        assert!(!contains_phys(r, 250, 199));
        assert!(!contains_phys(r, 250, 601));
    }
}
