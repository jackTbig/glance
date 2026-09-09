//! 托盘图标与原生右键菜单
//!
//! 不挂 muda 菜单：muda 那条「TrackPopupMenu 不带 RETURNCMD → WM_COMMAND → 托盘窗口子类 → 全局通道」
//! 在 winit 循环里事件时有时无（live-caption 踩过）。这里收到点击后自己调 Win32 弹原生菜单，同步拿结果。

use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub const MENU_SETTINGS: u32 = 1;
pub const MENU_PAUSE: u32 = 2;
pub const MENU_QUIT: u32 = 3;

/// 图标：编译期嵌进来的 PNG（见 `assets/gen_icon.ps1`）。
/// 32 px 是托盘的常用尺寸；系统需要别的尺寸会自己缩。
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon_32.png");

/// 解出 RGBA 像素，给 tray-icon 与 eframe 的窗口图标共用
pub fn icon_rgba() -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(ICON_PNG)?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    Ok((img.into_raw(), w, h))
}

pub fn make_icon() -> anyhow::Result<Icon> {
    let (rgba, w, h) = icon_rgba()?;
    Icon::from_rgba(rgba, w, h).map_err(|e| anyhow::anyhow!("托盘图标构造失败：{e}"))
}

pub fn build(tooltip: &str) -> anyhow::Result<TrayIcon> {
    TrayIconBuilder::new()
        .with_tooltip(tooltip)
        .with_icon(make_icon()?)
        .build()
        .map_err(|e| anyhow::anyhow!("托盘图标创建失败：{e}"))
}

/// 弹原生菜单，返回所选项 id。items = (id, 文字, 是否打勾)，id 为 0 表示分隔线。
#[cfg(windows)]
pub fn show_menu(hwnd: isize, x: i32, y: i32, items: &[(u32, String, bool)]) -> Option<u32> {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetForegroundWindow, GetWindowThreadProcessId, PostMessageW,
        SetForegroundWindow, TrackPopupMenu, MF_CHECKED, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
        TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe {
        let hmenu = CreatePopupMenu();
        if hmenu.is_null() {
            return None;
        }
        let mut bufs: Vec<Vec<u16>> = Vec::new();
        for (id, text, checked) in items {
            if *id == 0 {
                AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
            } else {
                let w = wide(text);
                AppendMenuW(hmenu, MF_STRING | if *checked { MF_CHECKED } else { 0 }, *id as usize, w.as_ptr());
                bufs.push(w);
            }
        }
        let h = hwnd as *mut core::ffi::c_void;
        // 先把自己设为前台，否则点菜单外面不会收起、键盘也到不了菜单。
        // 进程没有前台权限时 SetForegroundWindow 会被拒，先把输入队列挂到当前前台线程上。
        {
            let fg = GetForegroundWindow();
            let fg_tid = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) };
            let my_tid = GetCurrentThreadId();
            let attached = fg_tid != 0 && fg_tid != my_tid && AttachThreadInput(fg_tid, my_tid, 1) != 0;
            SetForegroundWindow(h);
            if attached {
                AttachThreadInput(fg_tid, my_tid, 0);
            }
        }
        let cmd = TrackPopupMenu(
            hmenu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RIGHTBUTTON,
            x,
            y,
            0,
            h,
            std::ptr::null(),
        );
        PostMessageW(h, WM_NULL, 0, 0);
        DestroyMenu(hmenu);
        drop(bufs);
        if cmd > 0 {
            Some(cmd as u32)
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
pub fn show_menu(_hwnd: isize, _x: i32, _y: i32, _items: &[(u32, String, bool)]) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_builds() {
        assert!(make_icon().is_ok());
    }

    #[test]
    fn menu_ids_are_distinct() {
        let ids = [MENU_SETTINGS, MENU_PAUSE, MENU_QUIT];
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
        assert!(!ids.contains(&0), "0 是分隔线，不能当菜单 id");
    }
}
