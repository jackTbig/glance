//! 截屏：抓虚拟屏上任意矩形（支持多显示器与负坐标）

/// 虚拟屏范围（所有显示器合并后的外接矩形）：x, y, w, h
pub fn virtual_screen() -> (i32, i32, u32, u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        };
        unsafe {
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let w = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32;
            let h = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32;
            (x, y, w, h)
        }
    }
    #[cfg(not(windows))]
    {
        (0, 0, 1920, 1080)
    }
}

pub fn cursor_pos() -> (i32, i32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut p);
        }
        (p.x, p.y)
    }
    #[cfg(not(windows))]
    {
        (0, 0)
    }
}

/// 光标形状粗判：用来分辨「鼠标停在文字上」还是「停在别的东西上」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    /// I 形：停在可选中的文字上
    IBeam,
    /// 箭头：不在文字上（拖文件、拖滑块、拖图片都是这个）
    Arrow,
    /// 其它（手型、拖动图标、自绘光标……）认不准
    Other,
}

/// 当前光标的形状。取不到就按「认不准」处理。
///
/// 判据是光标手柄：系统 I 形/箭头都是共享对象，手柄全局唯一。
/// 拿不到形状信息时返回 Other——**宁可认不准，也不要把划词判没了**。
pub fn cursor_shape() -> CursorShape {
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicPtr, Ordering};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetCursorInfo, LoadCursorW, CURSORINFO, IDC_ARROW, IDC_IBEAM,
        };

        static IBEAM: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
        static ARROW: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
        unsafe {
            let mut ibeam = IBEAM.load(Ordering::Relaxed);
            if ibeam.is_null() {
                ibeam = LoadCursorW(std::ptr::null_mut(), IDC_IBEAM);
                IBEAM.store(ibeam, Ordering::Relaxed);
            }
            let mut arrow = ARROW.load(Ordering::Relaxed);
            if arrow.is_null() {
                arrow = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
                ARROW.store(arrow, Ordering::Relaxed);
            }
            if ibeam.is_null() || arrow.is_null() {
                return CursorShape::Other;
            }
            let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
            if GetCursorInfo(&mut info) == 0 {
                return CursorShape::Other;
            }
            // 光标被藏起来了（有些程序拖拽时藏光标）：这时候形状不可信
            if info.flags & windows_sys::Win32::UI::WindowsAndMessaging::CURSOR_SUPPRESSED != 0 {
                return CursorShape::Other;
            }
            if info.hCursor == ibeam {
                CursorShape::IBeam
            } else if info.hCursor == arrow {
                CursorShape::Arrow
            } else {
                CursorShape::Other
            }
        }
    }
    #[cfg(not(windows))]
    {
        CursorShape::Other
    }
}

/// 以 (x, y) 为中心的 w×h 矩形，收敛到虚拟屏范围内
pub fn rect_around(x: i32, y: i32, w: u32, h: u32) -> (i32, i32, u32, u32) {
    let (vx, vy, vw, vh) = virtual_screen();
    let w = w.min(vw);
    let h = h.min(vh);
    let mut left = x - (w / 2) as i32;
    let mut top = y - (h / 2) as i32;
    left = left.max(vx).min(vx + vw as i32 - w as i32);
    top = top.max(vy).min(vy + vh as i32 - h as i32);
    (left, top, w, h)
}

/// 抓屏幕上一块矩形
#[cfg(windows)]
pub fn capture_rect(x: i32, y: i32, w: u32, h: u32) -> anyhow::Result<image::RgbaImage> {
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, SRCCOPY,
    };

    anyhow::ensure!(w > 0 && h > 0, "截屏尺寸为 0");
    anyhow::ensure!(w <= 20000 && h <= 20000, "截屏尺寸过大");

    struct ScreenDc(HDC);
    impl Drop for ScreenDc {
        fn drop(&mut self) {
            unsafe { ReleaseDC(std::ptr::null_mut(), self.0) };
        }
    }
    struct MemDc(HDC);
    impl Drop for MemDc {
        fn drop(&mut self) {
            unsafe { DeleteDC(self.0) };
        }
    }
    struct Bmp(HBITMAP);
    impl Drop for Bmp {
        fn drop(&mut self) {
            unsafe { DeleteObject(self.0 as _) };
        }
    }

    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        anyhow::ensure!(!screen.is_null(), "GetDC 失败");
        let screen = ScreenDc(screen);
        let mem = CreateCompatibleDC(screen.0);
        anyhow::ensure!(!mem.is_null(), "CreateCompatibleDC 失败");
        let mem = MemDc(mem);
        let bmp = CreateCompatibleBitmap(screen.0, w as i32, h as i32);
        anyhow::ensure!(!bmp.is_null(), "CreateCompatibleBitmap 失败");
        let bmp = Bmp(bmp);
        let old = SelectObject(mem.0, bmp.0 as _);
        let ok = BitBlt(mem.0, 0, 0, w as i32, h as i32, screen.0, x, y, SRCCOPY);
        SelectObject(mem.0, old);
        anyhow::ensure!(ok != 0, "BitBlt 失败");

        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            // 负高度 = 自上而下，省得再翻转
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let got = GetDIBits(mem.0, bmp.0, 0, h, buf.as_mut_ptr() as _, &mut info, DIB_RGB_COLORS);
        anyhow::ensure!(got != 0, "GetDIBits 失败");
        // BGRA → RGBA，alpha 强制不透明（BitBlt 出来的 alpha 通道是垃圾）
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        image::RgbaImage::from_raw(w, h, buf).ok_or_else(|| anyhow::anyhow!("像素缓冲长度不符"))
    }
}

#[cfg(not(windows))]
pub fn capture_rect(_x: i32, _y: i32, _w: u32, _h: u32) -> anyhow::Result<image::RgbaImage> {
    anyhow::bail!("当前平台不支持截屏")
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn virtual_screen_is_sane() {
        let (_, _, w, h) = virtual_screen();
        assert!(w >= 640 && h >= 480, "{w}x{h}");
    }

    #[test]
    fn rect_around_clamps_to_screen() {
        let (vx, vy, vw, vh) = virtual_screen();
        let (x, y, w, h) = rect_around(vx - 5000, vy - 5000, 600, 200);
        assert_eq!((x, y), (vx, vy), "左上越界应收敛到屏幕原点");
        let (x2, y2, _, _) = rect_around(vx + vw as i32 + 5000, vy + vh as i32 + 5000, 600, 200);
        assert_eq!(x2, vx + vw as i32 - w as i32);
        assert_eq!(y2, vy + vh as i32 - h as i32);
    }

    #[test]
    fn rect_around_shrinks_oversized_box() {
        let (_, _, vw, _) = virtual_screen();
        let (_, _, w, _) = rect_around(0, 0, vw + 10_000, 100);
        assert_eq!(w, vw);
    }

    #[test]
    fn capture_small_rect_returns_image() {
        let img = capture_rect(0, 0, 8, 8).unwrap();
        assert_eq!((img.width(), img.height()), (8, 8));
        assert!(img.pixels().all(|p| p.0[3] == 255), "alpha 应全不透明");
    }

    #[test]
    fn capture_zero_size_is_error() {
        assert!(capture_rect(0, 0, 0, 10).is_err());
    }
}
