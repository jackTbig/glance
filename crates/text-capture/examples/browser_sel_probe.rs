//! 回归验证：浏览器窗口里，桌面端的划词取词路径能读到选区，且走的是无障碍接口。
//!
//! 用法：cargo run -p text-capture --example browser_sel_probe -- <浏览器窗口标题>
//! 配套用例：tests/e2e/browser_sel.spec.ts

use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn main() {
    let title = std::env::args().nth(1).unwrap_or_else(|| "Test Article".to_string());
    let w = wide(&title);
    let hwnd = unsafe { FindWindowW(std::ptr::null(), w.as_ptr()) };
    if hwnd.is_null() {
        println!("CHROME_WINDOW_NOT_FOUND");
        return;
    }
    let class = text_capture::clipboard::class_name(hwnd as isize);
    println!("HWND={:#x} CLASS={class}", hwnd as isize);

    let cap = text_capture::Capturer::new();
    match cap.selection_since(Some(hwnd as isize), None) {
        Ok(c) => {
            println!("SOURCE={:?} MS={}", c.source, c.took_ms);
            println!("TEXT={}", c.text);
        }
        Err(e) => println!("ERROR={e}"),
    }
}
