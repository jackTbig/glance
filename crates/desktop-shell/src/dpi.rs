//! 物理像素 ↔ 逻辑像素的唯一换算出处。
//!
//! 两套坐标系在这个应用里天天打架，混用一次界面就整体漂移：
//!
//! - **物理像素**：Win32 给的一切。`GetCursorPos`、`GetSystemMetrics(SM_C*VIRTUALSCREEN)`、
//!   `BitBlt` 抓屏的矩形。进程是 PER_MONITOR_AWARE，系统不替它缩放。
//! - **逻辑像素**：egui 的窗口指令收的单位。`ViewportCommand::OuterPosition` /
//!   `InnerSize` 传进去的值，egui-winit 会再乘一次 `pixels_per_point` 才交给窗口；
//!   egui 画布里的「点」也是这个单位。
//!
//! 实测机器：200% 缩放，物理 3072x1920，逻辑 1536x960，`pixels_per_point` = 2.0。
//! 把物理坐标直接喂给 `OuterPosition`，窗口就跑到两倍远的地方。
//!
//! **换算比每帧从 `ctx.pixels_per_point()` 取，不要缓存成常量**：窗口挪到另一台
//! 缩放不同的显示器上时这个值会变。egui-winit 的 `pixels_per_point()` 就是
//! `zoom_factor * 窗口 scale_factor`，与 `Context::pixels_per_point()` 同源，
//! 所以窗口定位与画布绘制共用这一个比例，不需要分两个。

/// 一次换算所用的缩放比。构造出来就地用掉，别存进结构体。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dpi {
    ppp: f32,
}

impl Dpi {
    /// 缩放比不是有限正数、或明显不合理时退回 1.0（宁可不缩放，也不要把窗口扔到天边）
    pub fn new(ppp: f32) -> Dpi {
        let ppp = if ppp.is_finite() && (0.25..=8.0).contains(&ppp) { ppp } else { 1.0 };
        Dpi { ppp }
    }

    /// 每帧现取，别缓存
    pub fn from_ctx(ctx: &egui::Context) -> Dpi {
        Dpi::new(ctx.pixels_per_point())
    }

    pub fn ppp(self) -> f32 {
        self.ppp
    }

    /// 物理坐标 → 窗口定位用的逻辑坐标
    pub fn to_logical_pos(self, (x, y): (i32, i32)) -> egui::Pos2 {
        egui::pos2(x as f32 / self.ppp, y as f32 / self.ppp)
    }

    /// 物理尺寸 → 逻辑尺寸
    pub fn to_logical_size(self, (w, h): (u32, u32)) -> egui::Vec2 {
        egui::vec2(w as f32 / self.ppp, h as f32 / self.ppp)
    }

    /// 单个长度：物理 → 逻辑
    pub fn to_logical_len(self, v: f32) -> f32 {
        v / self.ppp
    }

    /// 单个长度：逻辑 → 物理（四舍五入，至少 1 px）
    pub fn to_physical_len(self, v: f32) -> i32 {
        let p = (v * self.ppp).round() as i32;
        if v > 0.0 { p.max(1) } else { p }
    }

    /// 逻辑尺寸 → 物理尺寸（面板宽高是按逻辑像素配的，摆放计算要在物理坐标系里做）
    pub fn to_physical_size(self, (w, h): (f32, f32)) -> (u32, u32) {
        (self.to_physical_len(w).max(1) as u32, self.to_physical_len(h).max(1) as u32)
    }

    /// 逻辑坐标 → 物理坐标
    pub fn to_physical_pos(self, p: egui::Pos2) -> (i32, i32) {
        ((p.x * self.ppp).round() as i32, (p.y * self.ppp).round() as i32)
    }

    /// 屏幕上的物理点 → 以 `origin`（物理）为左上角的那个窗口内部的 egui 点坐标。
    /// 截屏遮罩画选框时用：遮罩窗覆盖整个虚拟屏，鼠标是物理坐标，画笔要的是窗口内的点。
    pub fn point_in_window(self, (x, y): (i32, i32), (ox, oy): (i32, i32)) -> egui::Pos2 {
        egui::pos2((x - ox) as f32 / self.ppp, (y - oy) as f32 / self.ppp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_at_200_percent() {
        let d = Dpi::new(2.0);
        assert_eq!(d.to_logical_pos((1000, 600)), egui::pos2(500.0, 300.0));
        assert_eq!(d.to_logical_size((3072, 1920)), egui::vec2(1536.0, 960.0));
        assert_eq!(d.to_physical_size((420.0, 460.0)), (840, 920));
        assert_eq!(d.to_physical_len(14.0), 28);
        assert_eq!(d.to_logical_len(28.0), 14.0);
    }

    #[test]
    fn identity_at_100_percent() {
        let d = Dpi::new(1.0);
        assert_eq!(d.to_logical_pos((1000, 600)), egui::pos2(1000.0, 600.0));
        assert_eq!(d.to_physical_size((420.0, 460.0)), (420, 460));
    }

    #[test]
    fn handles_fractional_scale() {
        let d = Dpi::new(1.5);
        assert_eq!(d.to_logical_pos((150, 300)), egui::pos2(100.0, 200.0));
        assert_eq!(d.to_physical_size((100.0, 100.0)), (150, 150));
    }

    #[test]
    fn absurd_scale_falls_back_to_one() {
        for bad in [0.0, -2.0, f32::NAN, f32::INFINITY, 100.0] {
            assert_eq!(Dpi::new(bad).ppp(), 1.0, "{bad} 应退回 1.0");
        }
    }

    #[test]
    fn round_trips_position() {
        let d = Dpi::new(2.0);
        let phys = (1234, 5678);
        assert_eq!(d.to_physical_pos(d.to_logical_pos(phys)), phys);
    }

    #[test]
    fn point_in_window_is_relative_to_origin() {
        let d = Dpi::new(2.0);
        // 遮罩窗左上角在物理 (0,0)：物理 (1000,600) 在窗口内是 (500,300) 点
        assert_eq!(d.point_in_window((1000, 600), (0, 0)), egui::pos2(500.0, 300.0));
        // 副屏原点为负时同样成立
        assert_eq!(d.point_in_window((-1720, -100), (-1920, -200)), egui::pos2(100.0, 50.0));
    }
}
