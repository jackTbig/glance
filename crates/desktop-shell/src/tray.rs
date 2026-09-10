//! 托盘图标与原生右键菜单
//!
//! 不挂 muda 菜单：muda 那条「TrackPopupMenu 不带 RETURNCMD → WM_COMMAND → 托盘窗口子类 → 全局通道」
//! 在 winit 循环里事件时有时无（live-caption 踩过）。这里收到点击后自己调 Win32 弹原生菜单，同步拿结果。

use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// 解出 RGBA 像素，给 tray-icon 与 eframe 的窗口图标共用。
///
/// PNG 由调用方 `include_bytes!` 传进来：**两个应用的图标不一样，不能在这里写死**。
/// 32 px 是托盘的常用尺寸，系统需要别的尺寸会自己缩。
pub fn icon_rgba(png: &[u8]) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(png)?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    Ok((img.into_raw(), w, h))
}

pub fn make_icon(png: &[u8]) -> anyhow::Result<Icon> {
    let (rgba, w, h) = icon_rgba(png)?;
    Icon::from_rgba(rgba, w, h).map_err(|e| anyhow::anyhow!("托盘图标构造失败：{e}"))
}

pub fn build(tooltip: &str, icon_png: &[u8]) -> anyhow::Result<TrayIcon> {
    TrayIconBuilder::new()
        .with_tooltip(tooltip)
        .with_icon(make_icon(icon_png)?)
        .build()
        .map_err(|e| anyhow::anyhow!("托盘图标创建失败：{e}"))
}

/// 托盘上的一次输入（已从 tray-icon 的事件里剥出来，好让判定逻辑可以单测）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// 左键松开。**不能当场当成单击**——双击的第一下也长这样
    LeftUp((i32, i32)),
    /// 右键松开。没有双击语义，可以立即响应
    RightUp((i32, i32)),
    LeftDouble,
}

/// 判定要做什么
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// 在这个位置弹菜单
    Menu((i32, i32)),
    /// 打开主界面（双击）
    Activate,
}

/// 处理一批托盘输入，并决定要不要动作。
///
/// `pending` 是挂起的左键单击（位置 + 发生时刻），由调用方持有、本函数就地更新。
/// 判定规则：
/// - 右键松开 → 立即弹菜单，并撤掉挂起的左键（用户改主意了）
/// - 左键松开 → 挂起，不立即弹
/// - 左键双击 → **撤掉挂起的那次单击**，改为激活主界面
/// - 挂起超过 `interval` 还没等到双击 → 那才是真的单击，弹菜单
///
/// 同一批里既有 LeftUp 又有 LeftDouble 是常态（Windows 的序列是 UP → DBLCLK → UP），
/// 所以双击分支必须清 pending，否则菜单会在设置窗之后又弹出来。
pub fn reduce<I: IntoIterator<Item = Input>>(
    inputs: I,
    pending: &mut Option<((i32, i32), std::time::Instant)>,
    now: std::time::Instant,
    interval: std::time::Duration,
) -> Action {
    let mut action = Action::Nothing;
    for input in inputs {
        match input {
            Input::LeftDouble => {
                *pending = None;
                action = Action::Activate;
            }
            Input::LeftUp(at) => {
                // 已经判定要激活了就别再挂起——那是双击后面那次 UP
                if action != Action::Activate {
                    *pending = Some((at, now));
                }
            }
            Input::RightUp(at) => {
                *pending = None;
                action = Action::Menu(at);
            }
        }
    }
    if action != Action::Nothing {
        return action;
    }
    // 没有新动作时，看挂起的单击等够了没
    if let Some((at, since)) = *pending {
        if now.duration_since(since) >= interval {
            *pending = None;
            return Action::Menu(at);
        }
    }
    Action::Nothing
}

/// 系统设定的双击间隔。**区分单击与双击必须等这么久**：
/// Windows 双击托盘图标的消息序列是 UP → DBLCLK → UP，第一次 UP 与单击无法当场分辨，
/// 只能挂起等这个时长；期间来了 DBLCLK 就撤销挂起的单击。
///
/// 读系统值而不是写死 500 ms：用户在「鼠标属性」里调过速度的话，写死会与系统行为脱节。
#[cfg(windows)]
pub fn double_click_interval() -> std::time::Duration {
    windows_link::link!("user32.dll" "system" fn GetDoubleClickTime() -> u32);
    // 返回 0 表示取不到，退回 Windows 的默认值
    let ms = unsafe { GetDoubleClickTime() };
    std::time::Duration::from_millis(if ms == 0 { 500 } else { ms as u64 })
}

#[cfg(not(windows))]
pub fn double_click_interval() -> std::time::Duration {
    std::time::Duration::from_millis(500)
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

    const IV: std::time::Duration = std::time::Duration::from_millis(500);

    #[test]
    fn right_click_pops_the_menu_immediately() {
        let mut pending = None;
        let now = std::time::Instant::now();
        assert_eq!(reduce([Input::RightUp((10, 20))], &mut pending, now, IV), Action::Menu((10, 20)));
        assert!(pending.is_none(), "右键不该留下挂起状态");
    }

    #[test]
    fn left_click_waits_before_popping() {
        let mut pending = None;
        let t0 = std::time::Instant::now();
        // 左键松开当场什么都不做——分辨不出这是单击还是双击的第一下
        assert_eq!(reduce([Input::LeftUp((5, 6))], &mut pending, t0, IV), Action::Nothing);
        assert!(pending.is_some());
        // 还没等够
        assert_eq!(reduce([], &mut pending, t0 + IV / 2, IV), Action::Nothing);
        // 等够了才是真单击
        assert_eq!(reduce([], &mut pending, t0 + IV, IV), Action::Menu((5, 6)));
        assert!(pending.is_none(), "弹过一次就要清掉，不能反复弹");
    }

    #[test]
    fn double_click_cancels_the_pending_single_click() {
        // Windows 的真实序列：UP → DBLCLK → UP，三个事件可能同一批到达
        let mut pending = None;
        let now = std::time::Instant::now();
        let action = reduce([Input::LeftUp((5, 6)), Input::LeftDouble, Input::LeftUp((5, 6))], &mut pending, now, IV);
        assert_eq!(action, Action::Activate);
        assert!(pending.is_none(), "双击后还留着挂起的单击，菜单会在设置窗之后又弹出来");
        // 后续帧不该再冒出菜单
        assert_eq!(reduce([], &mut pending, now + IV * 2, IV), Action::Nothing);
    }

    #[test]
    fn double_click_split_across_frames_still_cancels() {
        // 事件也可能分帧到达：先来 UP，下一帧才来 DBLCLK
        let mut pending = None;
        let t0 = std::time::Instant::now();
        assert_eq!(reduce([Input::LeftUp((1, 2))], &mut pending, t0, IV), Action::Nothing);
        let action = reduce([Input::LeftDouble], &mut pending, t0 + IV / 4, IV);
        assert_eq!(action, Action::Activate);
        assert_eq!(reduce([], &mut pending, t0 + IV * 2, IV), Action::Nothing);
    }

    #[test]
    fn double_click_interval_is_sane() {
        let d = double_click_interval();
        // 取不到系统值时要退回默认，绝不能是 0——否则挂起的单击会立刻超时，双击永远判不出来
        assert!(d.as_millis() >= 100 && d.as_millis() <= 5000, "{d:?} 不像个双击间隔");
    }

    /// 现编一张 PNG 验解码链路；真图标由各应用自己 `include_bytes!` 传进来
    #[test]
    fn icon_decodes_from_caller_supplied_png() {
        let img = image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let (rgba, w, h) = icon_rgba(png.get_ref()).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(rgba.len(), 3 * 2 * 4);
        assert_eq!(&rgba[..4], &[1, 2, 3, 255]);
        assert!(icon_rgba(b"not a png").is_err(), "喂垃圾必须报错，不能悄悄给一张空图标");
    }
}
