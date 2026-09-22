//! 鼠标监视线程：判断「选中结束」与「悬停停留」两种触发，发消息给主线程。
//!
//! 用轮询而不是低级鼠标钩子：钩子要求消息循环及时返回，卡一下整个系统的鼠标都会顿；
//! 每 30 ms 问一次鼠标状态足够灵敏，且不影响别的程序。

use crate::hotkey::HoverModifier;
use crossbeam_channel::Sender;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    /// 用户拖选完一段文字，鼠标抬起在这个位置
    SelectionEnded { x: i32, y: i32 },
    /// 按住修饰键并在此停留够久
    Hovered { x: i32, y: i32 },
}

#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub hover_modifier: HoverModifier,
    pub hover_delay: Duration,
    /// 拖动超过这个距离才算划选，避免普通点击也触发
    pub min_drag_px: i32,
    pub poll: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            hover_modifier: HoverModifier::Alt,
            hover_delay: Duration::from_millis(300),
            min_drag_px: 12,
            poll: Duration::from_millis(30),
        }
    }
}

/// 与平台无关的状态机，便于单测：喂入每一帧的鼠标状态，吐出触发
#[derive(Debug)]
pub struct WatchState {
    cfg: WatchConfig,
    down_at: Option<(i32, i32)>,
    was_down: bool,
    /// 悬停：位置与开始时间
    still_since: Option<(i32, i32, Instant)>,
    hover_fired_at: Option<(i32, i32)>,
    /// 暂停期间不产生任何触发
    pub paused: bool,
}

impl WatchState {
    pub fn new(cfg: WatchConfig) -> WatchState {
        WatchState { cfg, down_at: None, was_down: false, still_since: None, hover_fired_at: None, paused: false }
    }

    pub fn set_config(&mut self, cfg: WatchConfig) {
        self.cfg = cfg;
        self.still_since = None;
        self.hover_fired_at = None;
    }

    /// 喂一帧：鼠标位置、左键是否按下、修饰键是否按下、当前时刻
    pub fn step(&mut self, x: i32, y: i32, left_down: bool, mod_down: bool, now: Instant) -> Option<Trigger> {
        if self.paused {
            self.was_down = left_down;
            return None;
        }
        // 划选：按下记起点，抬起时若移动够远就触发
        let mut out = None;
        if left_down && !self.was_down {
            self.down_at = Some((x, y));
        } else if !left_down && self.was_down {
            if let Some((sx, sy)) = self.down_at.take() {
                if (x - sx).abs() + (y - sy).abs() >= self.cfg.min_drag_px {
                    out = Some(Trigger::SelectionEnded { x, y });
                }
            }
        }
        self.was_down = left_down;
        if out.is_some() {
            self.still_since = None;
            return out;
        }

        // 悬停：修饰键按住 + 位置基本不动 + 够久 + 这个位置还没触发过
        if self.cfg.hover_modifier == HoverModifier::None || !mod_down || left_down {
            self.still_since = None;
            self.hover_fired_at = None;
            return None;
        }
        match self.still_since {
            Some((sx, sy, t0)) if (x - sx).abs() + (y - sy).abs() <= 6 => {
                if now.duration_since(t0) >= self.cfg.hover_delay && self.hover_fired_at != Some((sx, sy)) {
                    self.hover_fired_at = Some((sx, sy));
                    return Some(Trigger::Hovered { x, y });
                }
            }
            _ => {
                self.still_since = Some((x, y, now));
                self.hover_fired_at = None;
            }
        }
        None
    }
}

/// 起监视线程。返回的句柄丢掉即停。
pub struct Watcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    cfg_tx: Sender<WatchConfig>,
    pause_tx: Sender<bool>,
    /// 最近一次左键按下那一刻的剪贴板序号（见 `text_capture::Capturer::selection_since`）
    press_seq: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl Watcher {
    /// 最近一次左键按下时的剪贴板序号。划词结束弹图标时读它，就是「这次拖选开始时」的序号
    pub fn clip_seq_at_press(&self) -> u32 {
        self.press_seq.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn set_config(&self, cfg: WatchConfig) {
        let _ = self.cfg_tx.send(cfg);
    }
    pub fn set_paused(&self, paused: bool) {
        let _ = self.pause_tx.send(paused);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(windows)]
pub fn spawn(cfg: WatchConfig, tx: Sender<Trigger>) -> Watcher {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let (cfg_tx, cfg_rx) = crossbeam_channel::unbounded::<WatchConfig>();
    let (pause_tx, pause_rx) = crossbeam_channel::unbounded::<bool>();
    let press_seq = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(text_capture::clipboard::sequence_number()));
    let press_seq2 = press_seq.clone();
    std::thread::Builder::new()
        .name("mouse-watcher".into())
        .spawn(move || {
            tracing::info!(
                "鼠标监视已启动：悬停修饰键 {}，停留 {} ms 触发",
                cfg.hover_modifier.label_zh(),
                cfg.hover_delay.as_millis()
            );
            let mut st = WatchState::new(cfg.clone());
            let mut poll = cfg.poll;
            let mut last_mod = false;
            let mut last_left = false;
            while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
                while let Ok(c) = cfg_rx.try_recv() {
                    poll = c.poll;
                    tracing::info!("鼠标监视换配置：修饰键 {}，停留 {} ms", c.hover_modifier.label_zh(), c.hover_delay.as_millis());
                    st.set_config(c);
                }
                while let Ok(p) = pause_rx.try_recv() {
                    st.paused = p;
                }
                let (x, y) = text_capture::screen::cursor_pos();
                let left_down = unsafe { (GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000) != 0 };
                if left_down && !last_left {
                    press_seq2.store(text_capture::clipboard::sequence_number(), std::sync::atomic::Ordering::SeqCst);
                }
                last_left = left_down;
                let mod_down = st.cfg.hover_modifier.is_down();
                // 修饰键按下 / 松开各记一行：排「按住 Alt 没反应」时，
                // 先要能分清是没读到按键，还是读到了但后面没走通
                if mod_down != last_mod {
                    tracing::debug!("悬停修饰键 {} {}，鼠标物理({x},{y})", st.cfg.hover_modifier.label_zh(), if mod_down { "按下" } else { "松开" });
                    last_mod = mod_down;
                }
                if let Some(t) = st.step(x, y, left_down, mod_down, Instant::now()) {
                    tracing::debug!("鼠标监视触发 {t:?}");
                    if tx.send(t).is_err() {
                        break;
                    }
                }
                std::thread::sleep(poll);
            }
        })
        .expect("spawn mouse watcher");
    Watcher { stop, cfg_tx, pause_tx, press_seq }
}

#[cfg(not(windows))]
pub fn spawn(_cfg: WatchConfig, _tx: Sender<Trigger>) -> Watcher {
    let (cfg_tx, _) = crossbeam_channel::unbounded();
    let (pause_tx, _) = crossbeam_channel::unbounded();
    Watcher {
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        cfg_tx,
        pause_tx,
        press_seq: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> WatchState {
        WatchState::new(WatchConfig { hover_delay: Duration::from_millis(300), ..Default::default() })
    }

    #[test]
    fn drag_then_release_fires_selection() {
        let mut s = st();
        let t = Instant::now();
        assert_eq!(s.step(10, 10, true, false, t), None);
        assert_eq!(s.step(60, 10, true, false, t), None);
        assert_eq!(s.step(60, 10, false, false, t), Some(Trigger::SelectionEnded { x: 60, y: 10 }));
    }

    #[test]
    fn plain_click_does_not_fire() {
        let mut s = st();
        let t = Instant::now();
        s.step(10, 10, true, false, t);
        assert_eq!(s.step(12, 11, false, false, t), None, "移动不足 12 px 不算划选");
    }

    #[test]
    fn hover_fires_once_after_delay() {
        let mut s = st();
        let t0 = Instant::now();
        assert_eq!(s.step(50, 50, false, true, t0), None, "刚停下不触发");
        assert_eq!(s.step(50, 50, false, true, t0 + Duration::from_millis(200)), None);
        assert_eq!(
            s.step(50, 50, false, true, t0 + Duration::from_millis(320)),
            Some(Trigger::Hovered { x: 50, y: 50 })
        );
        assert_eq!(s.step(50, 50, false, true, t0 + Duration::from_millis(900)), None, "同一位置只触发一次");
    }

    #[test]
    fn moving_resets_hover_timer() {
        let mut s = st();
        let t0 = Instant::now();
        s.step(50, 50, false, true, t0);
        s.step(200, 200, false, true, t0 + Duration::from_millis(200));
        assert_eq!(s.step(200, 200, false, true, t0 + Duration::from_millis(400)), None, "移动后重新计时");
        assert!(s.step(200, 200, false, true, t0 + Duration::from_millis(560)).is_some());
    }

    #[test]
    fn hover_needs_modifier() {
        let mut s = st();
        let t0 = Instant::now();
        s.step(50, 50, false, false, t0);
        assert_eq!(s.step(50, 50, false, false, t0 + Duration::from_millis(500)), None);
    }

    #[test]
    fn modifier_none_disables_hover() {
        let mut s = WatchState::new(WatchConfig { hover_modifier: HoverModifier::None, ..Default::default() });
        let t0 = Instant::now();
        s.step(50, 50, false, true, t0);
        assert_eq!(s.step(50, 50, false, true, t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn paused_state_fires_nothing() {
        let mut s = st();
        s.paused = true;
        let t0 = Instant::now();
        s.step(10, 10, true, false, t0);
        assert_eq!(s.step(80, 10, false, false, t0), None);
        assert_eq!(s.step(80, 10, false, true, t0 + Duration::from_secs(1)), None);
    }
}
