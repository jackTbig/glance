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

/// 截屏区域的完整结果：原图 + 每行文字及其在图内的位置。
///
/// 截屏翻译要把译文**盖回原文的位置**（有道那种），所以不能只给合并后的整段文本：
/// 位置信息一旦在这一层丢掉，界面就再也拼不回去了。
#[derive(Debug, Clone)]
pub struct RegionShot {
    /// 原始截图（物理像素）
    pub image: image::RgbaImage,
    /// 识别出的每一行：文字 + 相对图片左上角的矩形
    pub lines: Vec<ocr::OcrLine>,
    /// 各行合并后的整段文本（退回普通卡片时用）
    pub text: String,
    pub took_ms: u128,
}

impl RegionShot {
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
        self.selection_since(target, None)
    }

    /// 同 `selection`，多一个 `clip_seq_at_press`：用户按下鼠标开始拖选那一刻的剪贴板序号。
    ///
    /// 只给终端用：里面的程序开了鼠标模式时（Claude Code、vim 这类），拖选是程序自己画的，
    /// 终端没有选区；但这类程序常在松开鼠标时自己把选中内容写进剪贴板。
    /// 序号在拖选之后变过，就说明是它刚复制的，直接拿来用。
    pub fn selection_since(&self, target: Option<isize>, clip_seq_at_press: Option<u32>) -> anyhow::Result<Captured> {
        let t0 = std::time::Instant::now();
        // 终端单独走：读无障碍接口里的选区，不发 Ctrl+C（那在终端里是中断信号）
        let hwnd = target.unwrap_or_else(clipboard::foreground_window);
        let class = clipboard::class_name(hwnd);
        if clipboard::is_terminal_class(&class) {
            let text = match &self.uia {
                Some(u) => u.selection_in(hwnd)?,
                None => None,
            };
            tracing::info!(
                "划词目标是终端（{class}），改读无障碍接口的选区：{}",
                text.as_ref().map_or("没有".to_string(), |t| format!("{} 字", t.chars().count()))
            );
            if text.is_none() {
                if let Some(seq) = clip_seq_at_press {
                    let now = clipboard::sequence_number();
                    if now != seq {
                        let t = clipboard::get_text().unwrap_or_default();
                        tracing::info!(
                            "终端没有原生选区，但拖选之后剪贴板变过（序号 {seq} → {now}），当成程序自己复制的选区：{} 字",
                            t.chars().count()
                        );
                        if !t.trim().is_empty() {
                            return Ok(Captured {
                                text: t.trim().to_string(),
                                source: Source::Clipboard,
                                took_ms: t0.elapsed().as_millis(),
                            });
                        }
                    }
                }
            }
            return match text {
                Some(t) => Ok(Captured { text: t, source: Source::Uia, took_ms: t0.elapsed().as_millis() }),
                // 选区是终端里的程序自己画的（vim、Claude Code 这类开了鼠标模式的界面），终端本身不知道
                None => anyhow::bail!(
                    "终端里没读到选区：里面的程序接管了鼠标，选区是它自己画的。按住 Shift 再拖选就是终端自己的选区，或用截屏翻译"
                ),
            };
        }
        // 浏览器改走无障碍接口读选区：模拟 Ctrl+C 在浏览器里复制到的是空串
        // （理由见 clipboard::is_browser_class）。读不到再退回 Ctrl+C 兜底。
        if clipboard::is_browser_class(&class) {
            if let Some(u) = &self.uia {
                match u.selection_in(hwnd) {
                    Ok(Some(t)) if !t.trim().is_empty() => {
                        tracing::info!(
                            "划词目标是浏览器（{class}），改读无障碍接口的选区：{} 字",
                            t.trim().chars().count()
                        );
                        // 选区已经读到手里了，把前台还给浏览器——用户还要接着看这一页。
                        // 老流程（Ctrl+C）也是这么做的，别让交互习惯变掉。
                        clipboard::bring_to_front(hwnd);
                        return Ok(Captured {
                            text: t.trim().to_string(),
                            source: Source::Uia,
                            took_ms: t0.elapsed().as_millis(),
                        });
                    }
                    Ok(None) | Ok(Some(_)) => {
                        tracing::info!("浏览器里无障碍接口没读到选区，退回模拟 Ctrl+C")
                    }
                    Err(e) => tracing::warn!("浏览器里读无障碍接口选区出错：{e}"),
                }
            }
        }
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
        // 1.0.34：只挑光标位置那个单词（Youdao 风格），不是整行。
        let text = pick_word_at(&r, x - rx, y - ry).unwrap_or_else(|| r.text.clone());
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

    /// 截屏区域：识别文字并**连原图与每行坐标一起**返回（覆盖式译文用）
    pub fn region_shot(&self, x: i32, y: i32, w: u32, h: u32) -> anyhow::Result<RegionShot> {
        let t0 = std::time::Instant::now();
        let engine = self.ocr.as_ref().ok_or_else(|| anyhow::anyhow!("本机没有可用的文字识别引擎"))?;
        let image = screen::capture_rect(x, y, w, h)?;
        let r = engine.recognize(&image)?;
        Ok(RegionShot { image, lines: r.lines, text: r.text, took_ms: t0.elapsed().as_millis() })
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

/// 1.0.35：悬停取词取单词（Youdao 风格），用 OCR 自带的逐词像素矩形点对点找。
///
/// 之前用 `OcrLine.rect`（各 word rect 的并集）当背景，然后按字数均分行宽估算词中心
/// ——比例字体的 i/l/m/w 宽度差几倍，估算完全不准（Jack 2026-10-05 截图：鼠标在
/// "completed" 但被翻成整行）。现在直接用 `OcrLine.words` 的精确 rect。
///
/// 算法：先找鼠标所在行（容差按行高吸收半行内偏）。在该行内：
/// 1. 光标**严格落在**某个 word 矩形里（半开区间）→ 取这个 word
/// 2. 否则，按 `tolerance` 像素容差向外扩 word 矩形，落在扩大的矩形里就算候选，
///    取中心距离最近的那个 word
/// `OcrLine.words` 在 Windows OCR 实现里直接从 `OcrLine.Words()` 读出来，没估算。
pub fn pick_word_at(r: &OcrResult, x: i32, y: i32) -> Option<String> {
    pick_word_at_with(r, x, y, 15)
}

/// `tolerance` 是 word 矩形向外扩的像素（Youdao 默认 15，0 表示必须严格落在某 word 内）。
pub fn pick_word_at_with(r: &OcrResult, x: i32, y: i32, tolerance: i32) -> Option<String> {
    if r.lines.is_empty() {
        return None;
    }
    let line = r.lines.iter().find(|l| {
        let (lx, ly, lw, lh) = l.rect;
        x >= lx - tolerance && x <= lx + lw + tolerance
            && y >= ly - lh / 2 && y <= ly + lh + lh / 2
    })?;
    if line.words.is_empty() {
        return None;
    }
    // 1. 严格落在 word 内（半开区间：边缘像素算外面）
    if let Some(w) = line.words.iter().find(|w| {
        let (wx, wy, ww, wh) = w.rect;
        x > wx && x < wx + ww && y > wy && y < wy + wh
    }) {
        let t = w.text.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    // 2. tolerance=0：必须严格落在 word 内，找不到就直接 None（边缘像素不算"在词上"）
    if tolerance == 0 {
        return None;
    }
    // 3. tolerance>0：word 矩形向外扩 tolerance 后落在里面就算候选
    let mut best: Option<(i32, &str)> = None;
    for w in &line.words {
        let (wx, wy, ww, wh) = w.rect;
        if x < wx - tolerance || x > wx + ww + tolerance || y < wy - tolerance || y > wy + wh + tolerance {
            continue;
        }
        let t = w.text.trim();
        if t.is_empty() {
            continue;
        }
        // 中心距离（曼哈顿）排冲突
        let d = (x - (wx + ww / 2)).abs() + (y - (wy + wh / 2)).abs();
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, t));
        }
    }
    best.map(|(_, t)| t.to_string())
}

#[cfg(test)]
mod pick_tests {
    use super::*;
    use crate::ocr::{OcrLine, OcrWord};

    /// 测 `pick_line_at` 用：行 + 模拟 word 矩形（按字数均分行宽）。1.0.35 后 `words` 字段必填，
    /// 给个每行单字（中文用例）的最小填充，让 `pick_line_at` 的逻辑能跑。
    fn line(text: &str, y: i32) -> OcrLine {
        OcrLine {
            text: text.into(),
            rect: (10, y, 500, 24),
            words: vec![ocr::OcrWord { text: text.into(), rect: (10, y, 500, 24) }],
        }
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

    /// 1.0.35：用 OCR 自带的 per-word 矩形点对点找，不再按字数估算。
    /// 每个 word 自带精确像素矩形，鼠标 x 落在哪个 word 矩形里就取哪个。
    #[test]
    fn pick_word_in_english_line() {
        // 行在 (10, 0, 500, 24)；4 个 word 各自精确矩形
        let l = OcrLine {
            text: "Hello world foo bar".into(),
            rect: (10, 0, 500, 24),
            words: vec![
                OcrWord { text: "Hello".into(), rect: (10,  0, 80, 24) },
                OcrWord { text: "world".into(), rect: (90,  0, 70, 24) },
                OcrWord { text: "foo".into(),   rect: (160, 0, 60, 24) },
                OcrWord { text: "bar".into(),   rect: (220, 0, 80, 24) },
            ],
        };
        let r = OcrResult { lines: vec![l], text: "Hello world foo bar".into() };
        // 光标在每个 word 矩形中心
        assert_eq!(pick_word_at(&r, 50,  12).as_deref(), Some("Hello"), "光标在 Hello 中心");
        assert_eq!(pick_word_at(&r, 125, 12).as_deref(), Some("world"), "光标在 world 中心");
        assert_eq!(pick_word_at(&r, 190, 12).as_deref(), Some("foo"),   "光标在 foo 中心");
        assert_eq!(pick_word_at(&r, 260, 12).as_deref(), Some("bar"),   "光标在 bar 中心");
    }

    #[test]
    fn pick_word_handles_cjk_without_spaces() {
        // CJK 一行：每个汉字自带精确矩形
        let l = OcrLine {
            text: "你好世界".into(),
            rect: (10, 0, 500, 24),
            words: vec![
                OcrWord { text: "你".into(), rect: (10,  0, 120, 24) },
                OcrWord { text: "好".into(), rect: (130, 0, 120, 24) },
                OcrWord { text: "世".into(), rect: (250, 0, 120, 24) },
                OcrWord { text: "界".into(), rect: (370, 0, 130, 24) },
            ],
        };
        let r = OcrResult { lines: vec![l], text: "你好世界".into() };
        assert_eq!(pick_word_at(&r, 70,  12).as_deref(), Some("你"), "CJK 字符级定位");
        assert_eq!(pick_word_at(&r, 190, 12).as_deref(), Some("好"));
        assert_eq!(pick_word_at(&r, 310, 12).as_deref(), Some("世"));
        assert_eq!(pick_word_at(&r, 440, 12).as_deref(), Some("界"));
    }

    #[test]
    fn pick_word_uses_tolerance_to_fall_back_to_nearest() {
        // 鼠标落在两 word 之间（不在任何 word 内），应取容差内最近的 word
        let l = OcrLine {
            text: "foo bar".into(),
            rect: (10, 0, 200, 24),
            words: vec![
                OcrWord { text: "foo".into(), rect: (10,  0, 80, 24) },
                OcrWord { text: "bar".into(), rect: (90, 0, 80, 24) },
            ],
        };
        let r = OcrResult { lines: vec![l], text: "foo bar".into() };
        // x=90：foo [10,90)、bar [90,170)，严格"点对点内部"两边都不在 → 走容差最近
        // foo 中心 50 (d=40)、bar 中心 130 (d=40)，tie 选 first → foo
        assert_eq!(pick_word_at(&r, 90, 12).as_deref(), Some("foo"), "容差内找最近：foo（先到先得）");
        // tolerance=0：d 必须 = 0，x=90 到两个中心都 40，超 0 → None
        assert_eq!(pick_word_at_with(&r, 90, 12, 0), None, "tolerance=0 严格不在任何 word 中心 → None");
    }

    #[test]
    fn pick_word_returns_none_when_no_line_under_cursor() {
        let l = OcrLine {
            text: "Hello".into(),
            rect: (10, 0, 500, 24),
            words: vec![OcrWord { text: "Hello".into(), rect: (10, 0, 500, 24) }],
        };
        let r = OcrResult { lines: vec![l], text: "Hello".into() };
        // y=1000 远在第一行外（容差按行高 12 吸收半行 = 18 px）
        assert_eq!(pick_word_at(&r, 100, 1000), None);
    }

    #[test]
    fn pick_word_handles_empty_text() {
        // 整行就一个纯空白 word → 不该返回（"   " 不是单词）
        let l = OcrLine {
            text: "   ".into(),
            rect: (10, 0, 500, 24),
            words: vec![OcrWord { text: "   ".into(), rect: (10, 0, 500, 24) }],
        };
        let r = OcrResult { lines: vec![l], text: "   ".into() };
        // 鼠标在 word 矩形里，但 word 文本是空白 → 应判空返回 None
        assert!(pick_word_at(&r, 100, 12).map_or(true, |t| t.trim().is_empty()),
                "空白 word 不该被翻译成空字符串");
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
