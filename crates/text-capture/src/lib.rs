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

    /// 本机装了哪些 OCR 语言包（BCP-47 标签，如 `zh-Hans-CN`、`en-US`）。
    /// 运行环境自检用它判断截屏识别能不能work、英文准不准。
    pub fn ocr_languages(&self) -> Vec<String> {
        #[cfg(windows)]
        {
            ocr::WindowsOcr::available_languages()
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }

    /// 划词：当前选中的文本
    /// `target`：有选区的窗口（弹划词图标时记下的）。点图标会把前台抢走，必须先切回它再复制。
    /// 热键触发时用户的程序本来就在前台，传 None。
    pub fn selection(&self, target: Option<isize>) -> anyhow::Result<Captured> {
        let t0 = std::time::Instant::now();
        let text = clipboard::selected_text_from(target)?;
        Ok(Captured { text: text.trim().to_string(), source: Source::Clipboard, took_ms: t0.elapsed().as_millis() })
    }

    /// 悬停：先问无障碍接口，拿不到就截鼠标周围一块做文字识别。
    /// 识别出来的多行里**只挑鼠标压着的那一行**，不然一悬停就把周围一片都送去翻。
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
        let engine = self.ocr.as_ref().ok_or_else(|| anyhow::anyhow!("本机没有可用的文字识别引擎"))?;
        let img = screen::capture_rect(rx, ry, rw, rh)?;
        let r = engine.recognize(&img)?;
        // 鼠标在这张截图里的位置，用来挑行
        let text = pick_line_at(&r, x - rx, y - ry).unwrap_or_else(|| r.text.clone());
        Ok(Captured { text, source: Source::Ocr, took_ms: t0.elapsed().as_millis() })
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

/// 从识别结果里挑出压着 (x, y) 的那一行；没有正好压住的就取最近的一行（限 1.5 行高内）
pub fn pick_line_at(r: &OcrResult, x: i32, y: i32) -> Option<String> {
    if r.lines.is_empty() {
        return None;
    }
    let hit = r.lines.iter().find(|l| {
        let (lx, ly, lw, lh) = l.rect;
        x >= lx && x <= lx + lw && y >= ly && y <= ly + lh
    });
    if let Some(l) = hit {
        return Some(l.text.trim().to_string());
    }
    // 没压住任何一行：取垂直距离最近的，但别离太远（超过 1.5 行高就认为鼠标不在文字上）
    let mut best: Option<(i32, &OcrLine)> = None;
    for l in &r.lines {
        let (_, ly, _, lh) = l.rect;
        let center = ly + lh / 2;
        let d = (center - y).abs();
        let limit = (lh as f32 * 1.5) as i32;
        if d <= limit && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, l));
        }
    }
    best.map(|(_, l)| l.text.trim().to_string()).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    fn line(text: &str, y: i32) -> OcrLine {
        OcrLine { text: text.into(), rect: (10, y, 500, 24) }
    }

    fn result(lines: Vec<OcrLine>) -> OcrResult {
        let text = lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("
");
        OcrResult { lines, text }
    }

    #[test]
    fn picks_the_line_under_the_cursor() {
        let r = result(vec![line("first", 0), line("second", 30), line("third", 60)]);
        assert_eq!(pick_line_at(&r, 100, 40).as_deref(), Some("second"));
        assert_eq!(pick_line_at(&r, 100, 5).as_deref(), Some("first"));
    }

    #[test]
    fn falls_back_to_nearest_line_within_reach() {
        // 两行的中心分别在 y=12 与 y=52
        let r = result(vec![line("first", 0), line("second", 40)]);
        // 落在两行之间的空隙里，取中心更近的那行
        assert_eq!(pick_line_at(&r, 100, 36).as_deref(), Some("second"), "36 离 52 更近");
        assert_eq!(pick_line_at(&r, 100, 28).as_deref(), Some("first"), "28 离 12 更近");
    }

    #[test]
    fn far_from_any_line_gives_nothing() {
        let r = result(vec![line("only", 0)]);
        assert_eq!(pick_line_at(&r, 100, 500), None, "离所有行都很远就别硬凑");
    }

    #[test]
    fn empty_result_is_none() {
        assert_eq!(pick_line_at(&result(vec![]), 0, 0), None);
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
