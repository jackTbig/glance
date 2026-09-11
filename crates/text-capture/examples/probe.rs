//! 手工验证取词：
//!   probe point            鼠标当前位置取词（3 秒倒计时，期间把鼠标移到目标文字上）
//!   probe selection        取当前选中文本（先在别处选好文字）
//!   probe region x y w h   截屏指定区域做文字识别
//!   probe info             打印能力与 OCR 语言
use text_capture::{screen, Capturer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("info");
    let c = Capturer::new();
    match cmd {
        "info" => {
            println!("无障碍接口: {}", c.has_uia());
            println!("文字识别:   {}  语言 {:?}", c.has_ocr(), c.ocr_language());
            let (x, y, w, h) = screen::virtual_screen();
            println!("虚拟屏:     ({x},{y}) {w}x{h}");
        }
        "point" => {
            for i in (1..=3).rev() {
                println!("{i}... 把鼠标移到要取词的文字上");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            let (x, y) = screen::cursor_pos();
            println!("鼠标 ({x}, {y})  进程 {:?}", c.process_at(x, y));
            match c.at_point(x, y) {
                Ok(r) => println!("[{}] {}ms\n{}", r.source.label_zh(), r.took_ms, r.text),
                Err(e) => println!("失败: {e}"),
            }
        }
        "selection" => {
            for i in (1..=3).rev() {
                println!("{i}... 在任意窗口里选中一段文字（别松开焦点）");
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            // 倒计时期间用户自己的程序就在前台，不用切
            match c.selection(None) {
                Ok(r) => println!("[{}] {}ms\n{}", r.source.label_zh(), r.took_ms, r.text),
                Err(e) => println!("失败: {e}"),
            }
        }
        "region" => {
            let n = |i: usize, d: i64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
            let (x, y, w, h) = (n(2, 0) as i32, n(3, 0) as i32, n(4, 800) as u32, n(5, 300) as u32);
            match c.region(x, y, w, h) {
                Ok(r) => println!("[{}] {}ms\n{}", r.source.label_zh(), r.took_ms, r.text),
                Err(e) => println!("失败: {e}"),
            }
        }
        other => println!("未知命令 {other}"),
    }
}
