//! text-capture：Windows 上的三种取词方式
//!
//! - 划词：模拟 Ctrl+C 读剪贴板，所有应用通用
//! - 悬停：无障碍接口（UI Automation）取鼠标下的段落，失败自动截图做文字识别
//! - 截屏：框选区域后做文字识别
//!
//! 文字识别引擎可插拔，默认用 Windows 自带的（见 [`ocr`]）。

pub mod clipboard;
pub mod ocr;
pub mod screen;
pub mod text;
pub mod uia;

pub use ocr::{OcrEngine, OcrLine, OcrResult};
pub use uia::UiaCapture;

/// 这段文本是怎么来的
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Clipboard,
    Uia,
    Ocr,
}

impl Source {
    pub fn label_zh(self) -> &'static str {
        match self {
            Source::Clipboard => "划词",
            Source::Uia => "无障碍接口",
            Source::Ocr => "文字识别",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Captured {
    pub text: String,
    pub source: Source,
    pub took_ms: u128,
}

impl Captured {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// 取词器：持有 UIA 与 OCR 两个可选能力，任一初始化失败只降级不阻断
pub struct Capturer {
    uia: Option<UiaCapture>,
    ocr: Option<Box<dyn OcrEngine>>,
    /// 悬停取词失败时截多大一块做识别
    pub ocr_box: (u32, u32),
}

impl Default for Capturer {
    fn default() -> Self {
        Self::new()
    }
}

impl Capturer {
    pub fn new() -> Capturer {
        let uia = match UiaCapture::new() {
            Ok(u) => Some(u),
            Err(e) => {
                tracing::warn!("无障碍接口不可用，悬停取词将只靠文字识别：{e}");
                None
            }
        };
        let ocr = match ocr::default_engine() {
            Ok(o) => {
                tracing::info!("文字识别引擎：{}", o.name());
                Some(o)
            }
            Err(e) => {
                tracing::warn!("文字识别不可用，截屏翻译将无法使用：{e}");
                None
            }
        };
        Capturer { uia, ocr, ocr_box: (700, 220) }
    }

    pub fn has_uia(&self) -> bool {
        self.uia.is_some()
    }

    pub fn has_ocr(&self) -> bool {
        self.ocr.is_some()
    }

    /// 文字识别引擎当前使用的语言（Windows 引擎才有）
    pub fn ocr_language(&self) -> Option<String> {
        #[cfg(windows)]
        {
            ocr::WindowsOcr::new().ok().map(|e| e.language().to_string())
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 划词：当前选中的文本
    pub fn selection(&self) -> anyhow::Result<Captured> {
        let t0 = std::time::Instant::now();
        let text = clipboard::selected_text()?;
        Ok(Captured { text: text.trim().to_string(), source: Source::Clipboard, took_ms: t0.elapsed().as_millis() })
    }

    /// 悬停：先问无障碍接口，拿不到就截鼠标周围一块做文字识别
    pub fn at_point(&self, x: i32, y: i32) -> anyhow::Result<Captured> {
        let t0 = std::time::Instant::now();
        if let Some(u) = &self.uia {
            if let Ok(Some(t)) = u.text_at(x, y) {
                if text::worth_translating(&t) {
                    return Ok(Captured { text: t, source: Source::Uia, took_ms: t0.elapsed().as_millis() });
                }
            }
        }
        let (w, h) = self.ocr_box;
        let (rx, ry, rw, rh) = screen::rect_around(x, y, w, h);
        let mut c = self.region(rx, ry, rw, rh)?;
        c.took_ms = t0.elapsed().as_millis();
        Ok(c)
    }

    /// 截屏区域：识别其中的文字
    pub fn region(&self, x: i32, y: i32, w: u32, h: u32) -> anyhow::Result<Captured> {
        let t0 = std::time::Instant::now();
        let engine = self.ocr.as_ref().ok_or_else(|| anyhow::anyhow!("本机没有可用的文字识别引擎"))?;
        let img = screen::capture_rect(x, y, w, h)?;
        let r = engine.recognize(&img)?;
        Ok(Captured { text: r.text, source: Source::Ocr, took_ms: t0.elapsed().as_millis() })
    }

    /// 该点所属进程的可执行文件名（进程例外用）
    pub fn process_at(&self, x: i32, y: i32) -> Option<String> {
        self.uia.as_ref().and_then(|u| u.process_at(x, y).ok().flatten())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn capturer_initializes_both_capabilities() {
        let c = Capturer::new();
        assert!(c.has_uia(), "本机应有无障碍接口");
        assert!(c.has_ocr(), "本机应有文字识别");
        assert!(c.ocr_language().is_some());
    }

    #[test]
    fn region_capture_returns_something() {
        let c = Capturer::new();
        let r = c.region(0, 0, 200, 60).unwrap();
        assert_eq!(r.source, Source::Ocr);
    }

    #[test]
    fn at_point_never_errors_on_empty_desktop() {
        let c = Capturer::new();
        let (x, y, _, _) = screen::virtual_screen();
        assert!(c.at_point(x + 2, y + 2).is_ok());
    }
}
