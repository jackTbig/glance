//! 简译的托盘：图标与菜单项。**弹菜单的机制在 `desktop_shell::tray`**，这里只提供本应用特有的部分。
//!
//! 分家的理由：两个应用的托盘菜单项完全不同（字幕有暂停/穿透/迷你条），
//! 但「怎么把原生菜单弹出来并同步拿到结果」那套 Win32 折腾一模一样。

pub use desktop_shell::tray::show_menu;

pub const MENU_SETTINGS: u32 = 1;
pub const MENU_PAUSE: u32 = 2;
pub const MENU_QUIT: u32 = 3;
/// 一键换翻译方向（英译中 ⇄ 中译英）。设置窗里也有，但这个是随手就能点到的
pub const MENU_DIRECTION: u32 = 4;
// 功能开关：与设置窗、热键改的是同一份配置，勾选态随时反映当前值
pub const MENU_HOVER: u32 = 5;
pub const MENU_SELECTION: u32 = 6;
pub const MENU_REGION_OVERLAY: u32 = 7;
pub const MENU_OFFLINE_DICT: u32 = 8;
pub const MENU_HOVER_SKIP: u32 = 9;
// 实时字幕那一块。字幕是子进程，这些项转成控制命令发过去（见 `crate::caption`）。
pub const MENU_CAPTION_TOGGLE: u32 = 10;
pub const MENU_CAPTION_PAUSE: u32 = 11;
pub const MENU_CAPTION_MINI: u32 = 12;
pub const MENU_CAPTION_CLICKABLE: u32 = 13;
pub const MENU_CAPTION_SETTINGS: u32 = 14;
/// 打开主窗口（手动输入翻译）
pub const MENU_MAIN: u32 = 15;

/// 图标：编译期嵌进来的 PNG（见 `assets/gen_icon.ps1`）。
/// 32 px 是托盘的常用尺寸；系统需要别的尺寸会自己缩。
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon_32.png");

/// 解出 RGBA，给托盘与 eframe 的窗口图标共用
pub fn icon_rgba() -> anyhow::Result<(Vec<u8>, u32, u32)> {
    desktop_shell::tray::icon_rgba(ICON_PNG)
}

pub fn build_app_tray(tooltip: &str) -> anyhow::Result<tray_icon::TrayIcon> {
    desktop_shell::tray::build(tooltip, ICON_PNG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_builds() {
        assert!(icon_rgba().is_ok());
    }

    #[test]
    fn menu_ids_are_distinct() {
        let ids = [
            MENU_SETTINGS,
            MENU_PAUSE,
            MENU_QUIT,
            MENU_DIRECTION,
            MENU_HOVER,
            MENU_SELECTION,
            MENU_REGION_OVERLAY,
            MENU_OFFLINE_DICT,
            MENU_HOVER_SKIP,
            MENU_CAPTION_TOGGLE,
            MENU_CAPTION_PAUSE,
            MENU_CAPTION_MINI,
            MENU_CAPTION_CLICKABLE,
            MENU_CAPTION_SETTINGS,
            MENU_MAIN,
        ];
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
        assert!(!ids.contains(&0), "0 是分隔线，不能当菜单 id");
    }
}
