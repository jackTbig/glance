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
    let mut votes: Vec<([u8; 4], u32)> = Vec::new();
    let mut sample = |p: Option<[u8; 4]>| {
        let Some(c) = p else { return };
        // 量化到 8 级，免得同一片背景因为一两级色差被拆成许多票
        let q = [c[0] & 0xE0, c[1] & 0xE0, c[2] & 0xE0, 255];
        match votes.iter_mut().find(|(v, _)| *v == q) {
            Some((_, n)) => *n += 1,
            None => votes.push((q, 1)),
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
    votes.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c).unwrap_or([255, 255, 255, 255])
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(bg[0] > 200 && bg[1] > 200 && bg[2] > 200, "应当取到白底，实际 {bg:?}");
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
