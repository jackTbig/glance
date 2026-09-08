//! OCR 引擎：接口 + Windows 自带实现（Windows.Media.Ocr）
//!
//! 引擎可插拔：以后加 RapidOCR 等本地模型只需再实现一次 [`OcrEngine`]，在 [`default_engine`] 里选。

#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    /// 版面坐标（相对送进来的图片）：x, y, w, h
    pub rect: (i32, i32, i32, i32),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OcrResult {
    pub lines: Vec<OcrLine>,
    /// 各行按断句合并后的整段文本
    pub text: String,
}

pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &str;
    fn recognize(&self, img: &image::RgbaImage) -> anyhow::Result<OcrResult>;
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine as WinOcrEngine;
    use windows::Storage::Streams::DataWriter;
    use windows::core::HSTRING;

    pub struct WindowsOcr {
        engine: WinOcrEngine,
        lang: String,
    }

    impl WindowsOcr {
        /// 按用户配置语言建；失败则挑第一个可用识别语言
        pub fn new() -> anyhow::Result<WindowsOcr> {
            let engine = match WinOcrEngine::TryCreateFromUserProfileLanguages() {
                Ok(e) => e,
                Err(_) => {
                    let langs = WinOcrEngine::AvailableRecognizerLanguages()?;
                    let first = langs.into_iter().next().ok_or_else(|| anyhow::anyhow!("本机没有任何 OCR 语言包"))?;
                    WinOcrEngine::TryCreateFromLanguage(&first)?
                }
            };
            let lang = engine.RecognizerLanguage()?.LanguageTag()?.to_string_lossy();
            Ok(WindowsOcr { engine, lang })
        }

        /// 指定语言标签（如 "en-US"）建引擎
        pub fn with_language(tag: &str) -> anyhow::Result<WindowsOcr> {
            let l = Language::CreateLanguage(&HSTRING::from(tag))?;
            let engine = WinOcrEngine::TryCreateFromLanguage(&l)?;
            let lang = engine.RecognizerLanguage()?.LanguageTag()?.to_string_lossy();
            Ok(WindowsOcr { engine, lang })
        }

        pub fn language(&self) -> &str {
            &self.lang
        }

        pub fn available_languages() -> Vec<String> {
            WinOcrEngine::AvailableRecognizerLanguages()
                .map(|ls| ls.into_iter().filter_map(|l| l.LanguageTag().ok().map(|t| t.to_string_lossy())).collect())
                .unwrap_or_default()
        }

        /// RgbaImage → SoftwareBitmap（Bgra8 预乘）。
        /// 用 DataWriter 造 IBuffer 再 CreateCopyFromBuffer，避开 IMemoryBufferByteAccess 那套 unsafe 接口。
        fn to_software_bitmap(img: &image::RgbaImage) -> anyhow::Result<SoftwareBitmap> {
            let (w, h) = (img.width(), img.height());
            anyhow::ensure!(w > 0 && h > 0, "空图片");
            let mut bytes = Vec::with_capacity((w * h * 4) as usize);
            for p in img.pixels() {
                let a = p.0[3] as u32;
                let m = |c: u8| ((c as u32 * a) / 255) as u8;
                bytes.push(m(p.0[2]));
                bytes.push(m(p.0[1]));
                bytes.push(m(p.0[0]));
                bytes.push(p.0[3]);
            }
            let writer = DataWriter::new()?;
            writer.WriteBytes(&bytes)?;
            let buf = writer.DetachBuffer()?;
            // 该重载不带 alphaMode，默认 Premultiplied，与上面的预乘写入一致
            Ok(SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Bgra8, w as i32, h as i32)?)
        }
    }

    impl OcrEngine for WindowsOcr {
        fn name(&self) -> &str {
            "windows"
        }

        fn recognize(&self, img: &image::RgbaImage) -> anyhow::Result<OcrResult> {
            let bmp = Self::to_software_bitmap(img)?;
            // 异步用固有方法 join()；windows-future 的 Async trait 未公开导出，不要 import
            let r = self.engine.RecognizeAsync(&bmp)?.join()?;
            let mut lines = Vec::new();
            for l in r.Lines()? {
                let text = l.Text()?.to_string_lossy();
                let mut rect = (i32::MAX, i32::MAX, 0i32, 0i32);
                for w in l.Words()? {
                    let b = w.BoundingRect()?;
                    let (x0, y0) = (b.X as i32, b.Y as i32);
                    let (x1, y1) = (x0 + b.Width as i32, y0 + b.Height as i32);
                    rect.0 = rect.0.min(x0);
                    rect.1 = rect.1.min(y0);
                    rect.2 = rect.2.max(x1);
                    rect.3 = rect.3.max(y1);
                }
                let rect = if rect.0 == i32::MAX { (0, 0, 0, 0) } else { (rect.0, rect.1, rect.2 - rect.0, rect.3 - rect.1) };
                lines.push(OcrLine { text, rect });
            }
            let raw: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
            let text = crate::text::merge_ocr_lines(&raw);
            let text = crate::text::clean_ocr_text(&text);
            Ok(OcrResult { lines, text })
        }
    }
}

#[cfg(windows)]
pub use win::WindowsOcr;

/// 默认引擎：Windows 自带
pub fn default_engine() -> anyhow::Result<Box<dyn OcrEngine>> {
    #[cfg(windows)]
    {
        Ok(Box::new(WindowsOcr::new()?))
    }
    #[cfg(not(windows))]
    {
        anyhow::bail!("当前平台没有可用的 OCR 引擎")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 画一张写着英文的白底图（不引字体库：用 8×8 点阵画几个字太脆，改为验证引擎能跑通 + 空图行为）
    fn solid(w: u32, h: u32) -> image::RgbaImage {
        image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
    }

    #[test]
    fn engine_creates_and_reports_language() {
        let e = WindowsOcr::new().expect("本机应至少有一个 OCR 语言包");
        assert!(!e.language().is_empty());
        assert!(!WindowsOcr::available_languages().is_empty());
        assert_eq!(e.name(), "windows");
    }

    #[test]
    fn blank_image_yields_empty_text() {
        let e = WindowsOcr::new().unwrap();
        let r = e.recognize(&solid(200, 80)).unwrap();
        assert!(r.text.trim().is_empty(), "空白图不该识别出文字：{:?}", r.text);
    }

    #[test]
    fn recognizes_text_from_fixture() {
        // 由 tests/make_fixture.rs 生成；没有就跳过（CI 无 GDI 时）
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr_en.png");
        if !p.exists() {
            eprintln!("跳过：缺 {}", p.display());
            return;
        }
        let img = image::open(&p).unwrap().to_rgba8();
        let e = WindowsOcr::new().unwrap();
        let r = e.recognize(&img).unwrap();
        assert!(r.text.to_lowercase().contains("rust"), "识别结果：{}", r.text);
        assert!(!r.lines.is_empty());
        assert!(r.lines[0].rect.2 > 0, "应有版面坐标");
    }
}
