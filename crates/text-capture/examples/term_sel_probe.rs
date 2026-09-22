//! 只读探针：通过无障碍接口问窗口「当前选中了什么」，**不发任何按键、不动鼠标**。
//!
//! 用来查终端（Windows Terminal 等）里划词取不到字的原因：终端是否暴露了选区、
//! 选区是终端自己的还是里面的程序（如带鼠标模式的 TUI）自己画的。
//!
//! 用法：`cargo run -p text-capture --example term_sel_probe -- [hwnd十进制]`，
//! 不给 hwnd 就扫描所有 Windows Terminal 顶层窗口。

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use uiautomation::patterns::UITextPattern;
    use uiautomation::types::{TreeScope, UIProperty};
    use uiautomation::variants::Variant;
    use uiautomation::UIAutomation;

    let auto = UIAutomation::new()?;
    let hwnds: Vec<isize> = match std::env::args().nth(1) {
        Some(h) => vec![h.parse()?],
        None => {
            let root = auto.get_root_element()?;
            let cond = auto.create_property_condition(
                UIProperty::ClassName,
                Variant::from("CASCADIA_HOSTING_WINDOW_CLASS"),
                None,
            )?;
            root.find_all(TreeScope::Children, &cond)?
                .iter()
                .filter_map(|e| e.get_native_window_handle().ok().map(|h| -> isize { h.into() }))
                .collect()
        }
    };
    for h in hwnds {
        let el = auto.element_from_handle(h.into())?;
        println!("窗口 {h} 类名 {:?} 标题 {:?}", el.get_classname().ok(), el.get_name().ok());
        let cond = auto.create_property_condition(UIProperty::IsTextPatternAvailable, Variant::from(true), None)?;
        let items = el.find_all(TreeScope::Descendants, &cond)?;
        println!("  带 TextPattern 的子元素 {} 个", items.len());
        for it in items {
            let cls = it.get_classname().unwrap_or_default();
            let Ok(tp) = it.get_pattern::<UITextPattern>() else { continue };
            let sel = tp.get_selection().unwrap_or_default();
            let texts: Vec<String> = sel.iter().filter_map(|r| r.get_text(400).ok()).collect();
            println!("  - 类名 {cls:?}：选区 {} 段 {:?}", sel.len(), texts);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
