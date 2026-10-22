//! 运行环境自检（控制台版）：`tt-doctor.exe`
//!
//! 与设置窗里的「运行环境」页查的是同一套（`desktop_shell::deps`），区别只在展示方式。
//! 单独做一个控制台程序，是为了覆盖界面起不来的场景——那正是最需要自检的时候。
//!
//! 退出码：0 = 必需项齐全（可选项缺不算失败），1 = 有硬缺失。安装脚本与排障可以据此判断。

fn main() {
    let app_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    // 建 Capturer 会去初始化 OCR 引擎；这里只要语言列表，失败也不该让自检本身挂掉
    let langs = std::panic::catch_unwind(|| text_capture::Capturer::new().ocr_languages()).unwrap_or_default();
    // 朗读用的音色列表（WinRT 引擎）。取不到就当没有，不让自检本身挂掉
    let voices = std::panic::catch_unwind(translate_tool::tts::voice_languages).unwrap_or_default();
    let items = desktop_shell::deps::check_all(app_dir.as_deref(), &langs, &voices);

    print!("{}", desktop_shell::deps::report(&items));

    if desktop_shell::deps::has_blocking(&items) {
        println!("\n有必需项缺失。桌面端的翻译功能仍可用（只要能连上模型接口），");
        println!("受影响的是上面标了「缺失」的那几项对应的功能。");
        println!("在简译的设置窗 →「运行环境」页里可以逐项一键修复。");
        std::process::exit(1);
    }
    println!("\n必需项齐全。");
}
