//! 取词 + 翻译的后台工作线程。界面只发请求、收消息，绝不在 UI 线程里做取词或网络。
use ai_translate::{Scene, Translator};
use crossbeam_channel::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use text_capture::{Capturer, Source};

/// 界面发给工作线程的活
#[derive(Debug, Clone)]
pub enum Job {
    /// 划词：自己去取选中文本
    Selection,
    /// 悬停：取该点的文字
    Hover { x: i32, y: i32 },
    /// 截屏区域
    Region { x: i32, y: i32, w: u32, h: u32 },
    /// 已有文本，直接翻（重试按钮用）
    Text { scene: Scene, text: String },
}

/// 工作线程发回界面的进度
#[derive(Debug, Clone)]
pub enum Event {
    /// 取到原文了（还没翻译）
    Captured { seq: u64, text: String, source: Source, took_ms: u128 },
    /// 译文增量（流式）
    Delta { seq: u64, text: String },
    Done { seq: u64, text: String, took_ms: u128 },
    Failed { seq: u64, error: String },
    /// 什么都没取到
    Nothing { seq: u64, reason: String },
}

pub struct Engine {
    tx: Sender<(u64, Job)>,
    seq: std::sync::atomic::AtomicU64,
}

impl Engine {
    /// 起工作线程。返回引擎与事件接收端。
    pub fn spawn(translator: Arc<Mutex<Translator>>, capturer: Arc<Capturer>) -> (Engine, Receiver<Event>) {
        let (tx, rx) = crossbeam_channel::unbounded::<(u64, Job)>();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded::<Event>();
        std::thread::Builder::new()
            .name("capture-translate".into())
            .spawn(move || {
                while let Ok((seq, job)) = rx.recv() {
                    // 只做最新一件活：界面连点时丢掉过期请求
                    let mut latest = (seq, job);
                    while let Ok(next) = rx.try_recv() {
                        latest = next;
                    }
                    run_job(latest.0, latest.1, &translator, &capturer, &ev_tx);
                }
            })
            .expect("spawn capture-translate");
        (Engine { tx, seq: std::sync::atomic::AtomicU64::new(0) }, ev_rx)
    }

    /// 派活，返回这次的序号（界面用它丢弃过期事件）
    pub fn submit(&self, job: Job) -> u64 {
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let _ = self.tx.send((seq, job));
        seq
    }
}

fn run_job(seq: u64, job: Job, translator: &Arc<Mutex<Translator>>, capturer: &Arc<Capturer>, ev: &Sender<Event>) {
    let t0 = std::time::Instant::now();
    // 第一步：拿到原文与场景
    let (scene, text, source) = match job {
        Job::Text { scene, text } => (scene, text, Source::Clipboard),
        Job::Selection => match capturer.selection() {
            Ok(c) if !c.is_empty() => (Scene::Selection, c.text, c.source),
            Ok(_) => {
                let _ = ev.send(Event::Nothing { seq, reason: "没有选中文字".into() });
                return;
            }
            Err(e) => {
                let _ = ev.send(Event::Failed { seq, error: format!("取词失败：{e}") });
                return;
            }
        },
        Job::Hover { x, y } => match capturer.at_point(x, y) {
            Ok(c) if !c.is_empty() => {
                tracing::debug!("悬停取词命中 物理({x},{y}) 来源{} {}字", c.source.label_zh(), c.text.chars().count());
                (Scene::Hover, c.text, c.source)
            }
            Ok(_) => {
                tracing::debug!("悬停取词落空 物理({x},{y})：无障碍接口与文字识别都没拿到文本");
                let _ = ev.send(Event::Nothing { seq, reason: "这里没有可识别的文字".into() });
                return;
            }
            Err(e) => {
                let _ = ev.send(Event::Failed { seq, error: format!("取词失败：{e}") });
                return;
            }
        },
        Job::Region { x, y, w, h } => match capturer.region(x, y, w, h) {
            Ok(c) if !c.is_empty() => {
                tracing::debug!("截屏识别命中 物理({x},{y}) {w}x{h} {}字", c.text.chars().count());
                (Scene::Screenshot, c.text, c.source)
            }
            Ok(_) => {
                tracing::debug!("截屏识别落空 物理({x},{y}) {w}x{h}");
                let _ = ev.send(Event::Nothing { seq, reason: "选区里没识别出文字".into() });
                return;
            }
            Err(e) => {
                let _ = ev.send(Event::Failed { seq, error: format!("截屏识别失败：{e}") });
                return;
            }
        },
    };
    let _ = ev.send(Event::Captured { seq, text: text.clone(), source, took_ms: t0.elapsed().as_millis() });

    // 第二步：翻译
    let tr = translator.lock().unwrap().clone();
    let t1 = std::time::Instant::now();
    let ev2 = ev.clone();
    let mut on_delta = |acc: &str| {
        let _ = ev2.send(Event::Delta { seq, text: acc.to_string() });
    };
    match tr.translate(scene, &text, None, &mut on_delta) {
        Ok(zh) => {
            let _ = ev.send(Event::Done { seq, text: zh, took_ms: t1.elapsed().as_millis() });
        }
        Err(e) => {
            let _ = ev.send(Event::Failed { seq, error: format!("翻译失败：{e}") });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_returns_increasing_sequence() {
        let mock = mock_llm::spawn(0);
        let mut cfg = ai_translate::TranslatorConfig::default();
        cfg.base.openai_base_url = mock.base_url.clone();
        let tr = Arc::new(Mutex::new(Translator::new(cfg).unwrap()));
        let (e, rx) = Engine::spawn(tr, Arc::new(Capturer::new()));
        let a = e.submit(Job::Text { scene: Scene::Selection, text: "hello".into() });
        assert_eq!(a, 1);
        // 应当先收到 Captured 再收到 Done
        let mut saw_done = None;
        for _ in 0..10 {
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Event::Done { text, .. }) => {
                    saw_done = Some(text);
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert_eq!(saw_done.as_deref(), Some("zh=hello"));
    }
}
