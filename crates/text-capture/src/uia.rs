//! 悬停取词：无障碍接口（UI Automation）按屏幕坐标取段落文本
//!
//! UIA 对无响应的窗口会长时间阻塞，所以每次调用都放到独立线程并加超时，绝不能直接在 UI 主线程上跑。

use std::time::Duration;

/// UIA 调用的默认超时；超过就当作取词失败，交给 OCR 兜底
pub const UIA_TIMEOUT: Duration = Duration::from_millis(600);

/// 悬停取词的长度上限。超过就说明拿到的不是「鼠标那一段」而是整篇文档，
/// 得换更小的文本单位重取。实测某些应用里 `TextUnit::Paragraph` 会返回 3000+ 字的全文。
pub const MAX_HOVER_CHARS: usize = 400;

#[cfg(windows)]
mod win {
    use super::*;
    use uiautomation::patterns::UITextPattern;
    use uiautomation::types::{Point, TextUnit};
    use uiautomation::UIAutomation;

    pub struct UiaCapture;

    impl UiaCapture {
        pub fn new() -> anyhow::Result<UiaCapture> {
            // 只探测一次能否建实例；真正的实例在工作线程里建（COM 单元不能跨线程共享）
            std::thread::spawn(|| UIAutomation::new().map(|_| ()))
                .join()
                .map_err(|_| anyhow::anyhow!("UIA 探测线程崩溃"))?
                .map_err(|e| anyhow::anyhow!("UI Automation 不可用：{e}"))?;
            Ok(UiaCapture)
        }

        /// 取屏幕坐标处的段落文本；拿不到返回 Ok(None)
        pub fn text_at(&self, x: i32, y: i32) -> anyhow::Result<Option<String>> {
            run_with_timeout(UIA_TIMEOUT, move || text_at_blocking(x, y))
        }

        /// 该点所属进程的可执行文件名（小写，如 "notepad.exe"）
        pub fn process_at(&self, x: i32, y: i32) -> anyhow::Result<Option<String>> {
            run_with_timeout(UIA_TIMEOUT, move || process_at_blocking(x, y))
        }
    }

    /// 在独立线程里跑 f，超时返回 Ok(None)（线程会继续跑完自己结束，不会卡住调用方）
    fn run_with_timeout<F>(timeout: Duration, f: F) -> anyhow::Result<Option<String>>
    where
        F: FnOnce() -> Option<String> + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("uia".into()).spawn(move || {
            let _ = tx.send(f());
        })?;
        match rx.recv_timeout(timeout) {
            Ok(v) => Ok(v),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                tracing::debug!("UIA 取词超时");
                Ok(None)
            }
            Err(_) => Ok(None),
        }
    }

    /// 按一个文本单位取鼠标处的文字；拿不到或不值得翻译返回 None
    fn text_by_unit(tp: &UITextPattern, pt: Point, unit: TextUnit) -> Option<String> {
        let range = tp.get_range_from_point(pt).ok()?;
        range.expand_to_enclosing_unit(unit).ok()?;
        let t = range.get_text(4000).ok()?;
        let t = t.trim();
        if crate::text::worth_translating(t) {
            Some(t.to_string())
        } else {
            None
        }
    }

    fn text_at_blocking(x: i32, y: i32) -> Option<String> {
        let auto = UIAutomation::new().ok()?;
        let pt = Point::new(x, y);
        let el = auto.element_from_point(pt).ok()?;

        // 逐级收窄：段落 → 行 → 词。
        // 只要「段落」这一级在某些应用里会把整篇文档算作一段（实测拿到过 3667 字），
        // 悬停就变成了翻整屏。超过上限就换更小的单位重取。
        if let Ok(tp) = el.get_pattern::<UITextPattern>() {
            let mut best: Option<String> = None;
            for unit in [TextUnit::Paragraph, TextUnit::Line, TextUnit::Word] {
                match text_by_unit(&tp, pt, unit) {
                    Some(t) if t.chars().count() <= MAX_HOVER_CHARS => return Some(t),
                    Some(t) => {
                        // 还是太长，记下最短的那个，万一更小的单位也拿不到就截断用它
                        if best.as_ref().map_or(true, |b: &String| t.chars().count() < b.chars().count()) {
                            best = Some(t);
                        }
                    }
                    None => continue,
                }
            }
            if let Some(t) = best {
                // 三级都超长（整篇没有分段信息）：截到上限，宁可少翻也别把整屏送出去
                let cut: String = t.chars().take(MAX_HOVER_CHARS).collect();
                tracing::debug!("UIA 取到 {} 字，三级单位都超长，截断到 {}", t.chars().count(), cut.chars().count());
                return Some(cut);
            }
        }

        // 退回元素名（按钮、菜单项、标签这类没有 TextPattern 的控件）
        let name = el.get_name().ok()?;
        let n = name.trim();
        if crate::text::worth_translating(n) && n.chars().count() <= MAX_HOVER_CHARS {
            return Some(n.to_string());
        }
        None
    }

    fn process_at_blocking(x: i32, y: i32) -> Option<String> {
        let auto = UIAutomation::new().ok()?;
        let el = auto.element_from_point(Point::new(x, y)).ok()?;
        let pid = el.get_process_id().ok()?;
        exe_name_of(pid as u32)
    }

    fn exe_name_of(pid: u32) -> Option<String> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 520];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            if ok == 0 {
                return None;
            }
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            Some(full.rsplit(['\\', '/']).next().unwrap_or(&full).to_ascii_lowercase())
        }
    }
}

#[cfg(windows)]
pub use win::UiaCapture;

#[cfg(not(windows))]
pub struct UiaCapture;
#[cfg(not(windows))]
impl UiaCapture {
    pub fn new() -> anyhow::Result<UiaCapture> {
        anyhow::bail!("当前平台没有 UI Automation")
    }
    pub fn text_at(&self, _x: i32, _y: i32) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    pub fn process_at(&self, _x: i32, _y: i32) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn creates_and_queries_without_panicking() {
        let c = UiaCapture::new().expect("本机应有 UI Automation");
        // 屏幕左上角通常是桌面或任务栏，取不到文本也必须是 Ok
        let r = c.text_at(0, 0);
        assert!(r.is_ok(), "{r:?}");
        let p = c.process_at(1, 1);
        assert!(p.is_ok(), "{p:?}");
    }

    #[test]
    fn out_of_screen_point_is_not_an_error() {
        let c = UiaCapture::new().unwrap();
        assert!(c.text_at(-999_999, -999_999).is_ok());
    }
}
