//! 截屏覆盖式译文的纯计算部分（与 egui 无关，可单测）。
//!
//! 有道那种做法：把截下来的原图原样显示，再把译文**盖回每一行原文的位置**，
//! 点一下在原文 / 译文之间切换。要做到「盖得像」，两件事必须算对：
//! 1. 盖住原文用什么底色——直接涂黑会把版面毁掉，要取这一行周围的背景色；
//! 2. 译文用多大字号——中文比英文短、但字宽，字号照搬行高会溢出行外。

/// 一行的覆盖信息（坐标都相对截图左上角，物理像素）
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub rect: (i32, i32, i32, i32),
    pub src: String,
    /// 译文。还没翻到这一行时是 None
    pub dst: Option<String>,
}

impl Line {
    /// 这一行值不值得覆盖：没译文、或译文与原文一样（模型原样退回）就别盖，
    /// 盖上去等于拿一块底色把原文糊掉，还不如让用户直接看原文
    pub fn worth_covering(&self) -> bool {
        match &self.dst {
            None => false,
            Some(d) => {
                let d = d.trim();
                !d.is_empty() && d != self.src.trim()
            }
        }
    }
}

/// 取一行的背景色：在行矩形**外侧一圈**采样，取出现次数最多的颜色。
///
/// 为什么采外侧：行矩形里面几乎全是字的颜色，采出来是字色，盖上去就成了一块黑条。
/// 为什么取众数而不是平均：平均会把深色文字和浅色背景混成灰，越糊越显眼。
pub fn background_of(
    px: impl Fn(i32, i32) -> Option<[u8; 4]>,
    rect: (i32, i32, i32, i32),
    pad: i32,
) -> [u8; 4] {
    let (x, y, w, h) = rect;
    // (量化后的桶, 桶里第一个真实颜色, 票数)。
    // **归票用量化、返回用真实颜色**：只用量化色的话纯白 255 会被截成 224，
    // 盖上去整条发灰，在白底文档上一眼就看得出来（2026-09-14 实拍发现）。
    let mut votes: Vec<([u8; 4], [u8; 4], u32)> = Vec::new();
    let mut sample = |p: Option<[u8; 4]>| {
        let Some(c) = p else { return };
        let q = [c[0] & 0xE0, c[1] & 0xE0, c[2] & 0xE0, 255];
        match votes.iter_mut().find(|(v, _, _)| *v == q) {
            Some((_, _, n)) => *n += 1,
            None => votes.push((q, [c[0], c[1], c[2], 255], 1)),
        }
    };
    let step = (w / 12).max(1);
    for i in 0..=(w / step) {
        let sx = x + i * step;
        sample(px(sx, y - pad));
        sample(px(sx, y + h + pad));
    }
    let vstep = (h / 4).max(1);
    for i in 0..=(h / vstep) {
        let sy = y + i * vstep;
        sample(px(x - pad, sy));
        sample(px(x + w + pad, sy));
    }
    votes.into_iter().max_by_key(|(_, _, n)| *n).map(|(_, real, _)| real).unwrap_or([255, 255, 255, 255])
}

/// 译文字号：先按行高给一个，再按「测出来的宽度」收缩，直到塞得进这一行。
///
/// `measure` 给定字号返回该字号下整行文字的宽度（像素）。收缩有下限：
/// 再小就看不清了，宁可让它略微超出，也不要糊成一团。
pub fn fit_font_size(rect_h: f32, rect_w: f32, measure: impl Fn(f32) -> f32) -> f32 {
    const MIN: f32 = 9.0;
    let mut size = (rect_h * 0.78).clamp(MIN, 96.0);
    for _ in 0..24 {
        if measure(size) <= rect_w || size <= MIN {
            break;
        }
        size *= 0.92;
    }
    size.max(MIN)
}

/// 盖住原文的矩形：比识别框稍微放大一点。
///
/// 识别框常常紧贴字形，原样盖会在边缘留下一圈原文的残影（尤其是有描边或阴影的字）。
pub fn cover_rect(rect: (i32, i32, i32, i32), grow: i32, bounds: (i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = rect;
    let (bw, bh) = bounds;
    let x0 = (x - grow).max(0);
    let y0 = (y - grow).max(0);
    let x1 = (x + w + grow).min(bw);
    let y1 = (y + h + grow).min(bh);
    (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

/// 一条画在屏幕上的覆盖指令。界面只管照着画，**排版算在这里**，好让测试盯住。
#[derive(Debug, Clone, PartialEq)]
pub struct DrawOp {
    /// 盖住原文的矩形（窗口内逻辑坐标：左, 上, 宽, 高）
    pub rect: (f32, f32, f32, f32),
    pub bg: [u8; 4],
    pub text: String,
    pub font_size: f32,
    /// 底色浅就写深色字，反之写浅色字——固定颜色在深色界面上会看不见
    pub dark_text: bool,
}

/// 排出这一屏要盖哪些字。
///
/// `img` 是截图的像素尺寸，`full` 是窗口在屏幕上的逻辑尺寸：两者比例即缩放系数
/// （200% 缩放下窗口逻辑尺寸正好是截图的一半，行坐标必须跟着缩，否则译文会整体偏移）。
/// `measure` 给定文字与字号返回宽度。
pub fn plan(
    lines: &[Line],
    bg: &[[u8; 4]],
    img: (i32, i32),
    full: (f32, f32),
    measure: impl Fn(&str, f32) -> f32,
) -> Vec<DrawOp> {
    let sx = full.0 / (img.0.max(1) as f32);
    let sy = full.1 / (img.1.max(1) as f32);
    let mut ops = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !l.worth_covering() {
            continue;
        }
        let (cx, cy, cw, ch) = cover_rect(l.rect, 2, (img.0, img.1));
        let rect = (cx as f32 * sx, cy as f32 * sy, cw as f32 * sx, ch as f32 * sy);
        let text = l.dst.clone().unwrap_or_default();
        let font_size = fit_font_size(rect.3, rect.2, |size| measure(&text, size));
        let c = bg.get(i).copied().unwrap_or([255, 255, 255, 255]);
        let lum = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
        ops.push(DrawOp { rect, bg: c, text, font_size, dark_text: lum > 140.0 });
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(rect: (i32, i32, i32, i32), src: &str, dst: Option<&str>) -> Line {
        Line { rect, src: src.into(), dst: dst.map(str::to_string) }
    }

    /// 200% 缩放：窗口逻辑尺寸是截图像素的一半，覆盖框必须跟着缩一半
    #[test]
    fn plan_scales_line_rects_to_the_window() {
        let lines = vec![l((100, 40, 200, 30), "hello", Some("你好"))];
        let ops = plan(&lines, &[[255, 255, 255, 255]], (1000, 200), (500.0, 100.0), |_, s| s * 2.0);
        assert_eq!(ops.len(), 1);
        // cover_rect 先外扩 2 像素：(98,38,204,34) → 逻辑减半
        assert_eq!(ops[0].rect, (49.0, 19.0, 102.0, 17.0));
    }

    #[test]
    fn plan_skips_lines_without_a_real_translation() {
        let lines = vec![
            l((0, 0, 50, 20), "a", None),
            l((0, 30, 50, 20), "b", Some("b")),
            l((0, 60, 50, 20), "c", Some("丙")),
        ];
        let bg = vec![[255, 255, 255, 255]; 3];
        let ops = plan(&lines, &bg, (100, 100), (100.0, 100.0), |_, s| s);
        assert_eq!(ops.len(), 1, "只盖真正翻出来的那一行");
        assert_eq!(ops[0].text, "丙");
    }

    #[test]
    fn plan_picks_text_color_by_background_luminance() {
        let lines = vec![l((0, 0, 100, 20), "x", Some("甲")), l((0, 30, 100, 20), "y", Some("乙"))];
        let bg = vec![[250, 250, 250, 255], [20, 20, 24, 255]];
        let ops = plan(&lines, &bg, (200, 100), (200.0, 100.0), |_, s| s);
        assert!(ops[0].dark_text, "白底写深色字");
        assert!(!ops[1].dark_text, "深底写浅色字");
    }

    #[test]
    fn plan_shrinks_font_to_fit_the_line() {
        let lines = vec![l((0, 0, 100, 40), "x", Some("一段很长的译文"))];
        // 每 1 号字占 20 px：40*0.78=31.2 号会宽到 624，必须收缩
        let ops = plan(&lines, &[[255, 255, 255, 255]], (200, 100), (200.0, 100.0), |_, s| s * 20.0);
        // 收缩到塞得下，或者已经到了「再小就看不清」的下限
        assert!(
            ops[0].font_size * 20.0 <= ops[0].rect.2 + 0.01 || (ops[0].font_size - 9.0).abs() < 0.01,
            "既没收缩到塞得下、也不是触到下限：{}",
            ops[0].font_size
        );
        assert!(ops[0].font_size < 31.0, "起始字号 32.76 必须被收缩过");
    }

    fn line(dst: Option<&str>) -> Line {
        Line { rect: (0, 0, 100, 20), src: "hello".into(), dst: dst.map(str::to_string) }
    }

    #[test]
    fn only_cover_lines_that_actually_got_translated() {
        assert!(!line(None).worth_covering(), "还没翻到的行不盖");
        assert!(!line(Some("  ")).worth_covering(), "空译文不盖");
        assert!(!line(Some("hello")).worth_covering(), "与原文相同说明模型原样退回，不盖");
        assert!(line(Some("你好")).worth_covering());
    }

    /// 背景色要采行外侧：行内几乎全是字，采进去会得到字色，盖上去就是一条黑杠
    #[test]
    fn background_comes_from_around_the_line_not_the_glyphs() {
        // 白底黑字：行矩形 (10,10,100,20) 内部全黑，外面全白
        let px = |x: i32, y: i32| -> Option<[u8; 4]> {
            if !(0..200).contains(&x) || !(0..100).contains(&y) {
                return None;
            }
            let inside = (10..110).contains(&x) && (10..30).contains(&y);
            Some(if inside { [0, 0, 0, 255] } else { [255, 255, 255, 255] })
        };
        let bg = background_of(px, (10, 10, 100, 20), 3);
        assert_eq!(bg, [255, 255, 255, 255], "白底要原样返回纯白，量化只用于归票");
    }

    #[test]
    fn background_falls_back_to_white_outside_the_image() {
        let bg = background_of(|_, _| None, (0, 0, 10, 10), 2);
        assert_eq!(bg, [255, 255, 255, 255], "全部采样都在图外时要有个能看的兜底");
    }

    #[test]
    fn font_shrinks_until_the_line_fits() {
        // 宽度与字号成正比：每 1 号字占 10 px
        let measure = |size: f32| size * 10.0;
        let s = fit_font_size(40.0, 100.0, measure);
        assert!(measure(s) <= 100.0, "收缩后要塞得下，实际 {s}");
        // 宽松的行不该被无谓地缩小
        let s2 = fit_font_size(40.0, 10_000.0, measure);
        assert!((s2 - 40.0 * 0.78).abs() < 0.01, "放得下就按行高给，实际 {s2}");
    }

    #[test]
    fn font_never_shrinks_below_readable() {
        let s = fit_font_size(20.0, 1.0, |size| size * 100.0);
        assert!(s >= 9.0, "再挤也不能小到看不清，实际 {s}");
    }

    #[test]
    fn cover_rect_grows_but_stays_inside_the_shot() {
        assert_eq!(cover_rect((10, 10, 50, 20), 2, (200, 100)), (8, 8, 54, 24));
        // 贴边的行不能越出图外，否则画出来会糊到别的地方
        assert_eq!(cover_rect((0, 0, 50, 20), 3, (40, 10)), (0, 0, 40, 10));
    }
}
