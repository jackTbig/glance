//! 取词 + 翻译的后台工作线程。界面只发请求、收消息，绝不在 UI 线程里做取词或网络。
use ai_translate::{Scene, Translator};
use crossbeam_channel::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use text_capture::{Capturer, Source};

/// 界面发给工作线程的活
#[derive(Debug, Clone)]
pub enum Job {
    /// 划词：自己去取选中文本。`target` 是有选区的窗口，见 `Capturer::selection`
    /// `clip_seq`：拖选开始时的剪贴板序号，终端里读不到选区时用来认「程序自己复制的」
    Selection { target: Option<isize>, clip_seq: Option<u32> },
    /// 悬停：取该点的文字。`skip_target_lang` = 取到的已经是目标语言时不出卡片
    Hover { x: i32, y: i32, skip_target_lang: bool },
    /// 截屏区域。`overlay` = 要覆盖式译文（原图上按行盖回去），否则只给一张普通卡片
    Region { x: i32, y: i32, w: u32, h: u32, overlay: bool },
    /// 已有文本，直接翻（重试按钮用）
    Text { scene: Scene, text: String },
    /// 主窗口里手动输入的文字。方向按内容自动定（`Direction::for_input`），单词给词典条目
    Manual { text: String },
}

/// 工作线程发回界面的进度
#[derive(Debug, Clone)]
pub enum Event {
    /// 取到原文了（还没翻译）。scene 是**取词后**定的：取到单个词会改判成词典场景
    Captured { seq: u64, text: String, source: Source, scene: Scene, took_ms: u128 },
    /// 译文增量（流式）
    Delta { seq: u64, text: String },
    Done { seq: u64, text: String, took_ms: u128 },
    Failed { seq: u64, error: String },
    /// 什么都没取到
    Nothing { seq: u64, reason: String },
    /// 取到了，但不用翻（已经是目标语言），**界面要静默收掉卡片**，不是报错
    Skipped { seq: u64 },
    /// 覆盖式截屏：原图与每行的位置先到（译文还没来），界面可以立刻把图铺上去
    Shot { seq: u64, shot: Arc<text_capture::RegionShot> },
    /// 覆盖式截屏：第 `idx` 行的译文到了
    Line { seq: u64, idx: usize, text: String },
    /// 离线词典先查到的释义（行标记格式）。模型的词条回来后会把它替换掉
    Offline { seq: u64, text: String },
}

pub struct Engine {
    tx: Sender<(u64, Job)>,
    seq: std::sync::atomic::AtomicU64,
    /// 要不要先给离线释义（设置里可关）。用原子量是为了改完立刻生效，不必重建引擎
    offline_on: Arc<std::sync::atomic::AtomicBool>,
}

/// 离线词典。**打开一次留着**：打开要读 3 MB 索引，每次查词都重开就没意义了。
/// 没有词库文件时是 None——离线查词不工作，翻译照常走模型。
type OfflineDict = Option<Mutex<offline_dict::Dict>>;

impl Engine {
    /// 起工作线程。返回引擎与事件接收端。
    pub fn spawn(translator: Arc<Mutex<Translator>>, capturer: Arc<Capturer>) -> (Engine, Receiver<Event>) {
        let (tx, rx) = crossbeam_channel::unbounded::<(u64, Job)>();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded::<Event>();
        // 词库在工作线程里打开：读 3 MB 索引要几毫秒，别卡住启动那一帧
        let dict: Arc<std::sync::OnceLock<OfflineDict>> = Arc::new(std::sync::OnceLock::new());
        let offline_on = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let offline_for_thread = offline_on.clone();
        std::thread::Builder::new()
            .name("capture-translate".into())
            .spawn(move || {
                while let Ok((seq, job)) = rx.recv() {
                    // 只做最新一件活：界面连点时丢掉过期请求
                    let mut latest = (seq, job);
                    while let Ok(next) = rx.try_recv() {
                        latest = next;
                    }
                    let dict = dict.get_or_init(|| match crate::offline::dict_path() {
                        Some(p) => match offline_dict::Dict::open(&p) {
                            Ok(d) => {
                                tracing::info!("离线词典已加载：{} 条（{}）", d.len(), p.display());
                                Some(Mutex::new(d))
                            }
                            Err(e) => {
                                tracing::warn!("离线词典打不开，只走模型：{e}");
                                None
                            }
                        },
                        None => {
                            tracing::info!("没找到离线词典文件，查词只走模型");
                            None
                        }
                    });
                    let dict = offline_for_thread.load(std::sync::atomic::Ordering::Relaxed).then_some(dict).unwrap_or(&None);
                    run_job(latest.0, latest.1, &translator, &capturer, dict, &ev_tx);
                }
            })
            .expect("spawn capture-translate");
        (Engine { tx, seq: std::sync::atomic::AtomicU64::new(0), offline_on }, ev_rx)
    }

    /// 派活，返回这次的序号（界面用它丢弃过期事件）
    /// 开关「先给离线释义」。设置窗保存时调一次即可，不用重建引擎
    pub fn set_offline(&self, on: bool) {
        self.offline_on.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn submit(&self, job: Job) -> u64 {
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let _ = self.tx.send((seq, job));
        seq
    }
}

fn run_job(
    seq: u64,
    job: Job,
    translator: &Arc<Mutex<Translator>>,
    capturer: &Arc<Capturer>,
    dict: &OfflineDict,
    ev: &Sender<Event>,
) {
    let t0 = std::time::Instant::now();
    // 方向现取：设置里改完立刻生效，不用重启
    let base = translator.lock().unwrap().clone();
    let mut dir = base.config().direction;
    // 手动输入按内容定方向，并换成那个方向的翻译器
    let tr = match &job {
        Job::Manual { text } => {
            dir = dir.for_input(text);
            match base.for_direction(dir) {
                Ok(t) => t,
                Err(e) => {
                    let _ = ev.send(Event::Failed { seq, error: format!("翻译器建不起来：{e}") });
                    return;
                }
            }
        }
        _ => base,
    };
    // 第一步：拿到原文与场景
    let (scene, text, source) = match job {
        Job::Text { scene, text } => (scene, text, Source::Clipboard),
        Job::Manual { text } => (Scene::Selection, text.trim().to_string(), Source::Clipboard),
        Job::Selection { target, clip_seq } => match capturer.selection_since(target, clip_seq) {
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
        Job::Hover { x, y, skip_target_lang } => match capturer.at_point(x, y) {
            Ok(c) if !c.is_empty() => {
                tracing::debug!("悬停取词命中 物理({x},{y}) 来源{} {}字", c.source.label_zh(), c.text.chars().count());
                // 已经是目标语言就别弹卡片：鼠标扫过中文一路弹窗最烦人
                if skip_target_lang && dir.already_target(&c.text) {
                    tracing::debug!("悬停取到的已经是目标语言（{}），不弹卡片", dir.label_zh());
                    let _ = ev.send(Event::Skipped { seq });
                    return;
                }
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
        // 覆盖式截屏走自己的一条路：要的是「每行原文 + 每行位置 + 每行译文」，
        // 与普通卡片那条「一整段文本」完全不同，混在一起写会互相牵制
        Job::Region { x, y, w, h, overlay: true } => {
            match capturer.region_shot(x, y, w, h) {
                Ok(s) if !s.is_empty() => {
                    let shot = Arc::new(s);
                    tracing::debug!("截屏覆盖：识别到 {} 行", shot.lines.len());
                    let _ = ev.send(Event::Shot { seq, shot: shot.clone() });
                    // 逐行翻：批量接口内部会并发分块，比一行一个请求省得多
                    let items: Vec<ai_translate::Item> = shot
                        .lines
                        .iter()
                        .enumerate()
                        .map(|(i, l)| ai_translate::Item { id: i.to_string(), text: l.text.clone(), is_html: false })
                        .collect();
                    let t1 = std::time::Instant::now();
                    for r in tr.translate_batch(Scene::Screenshot, items) {
                        let Ok(idx) = r.id.parse::<usize>() else { continue };
                        match r.result {
                            Ok(text) => {
                                let _ = ev.send(Event::Line { seq, idx, text });
                            }
                            // 单行失败不算整次失败：剩下的行照样盖，用户还能看到大部分译文
                            Err(e) => tracing::warn!("截屏第 {idx} 行翻译失败：{e}"),
                        }
                    }
                    let _ = ev.send(Event::Done { seq, text: String::new(), took_ms: t1.elapsed().as_millis() });
                }
                Ok(_) => {
                    let _ = ev.send(Event::Nothing { seq, reason: "选区里没识别出文字".into() });
                }
                Err(e) => {
                    let _ = ev.send(Event::Failed { seq, error: format!("截屏识别失败：{e}") });
                }
            }
            return;
        }
        Job::Region { x, y, w, h, overlay: false } => match capturer.region(x, y, w, h) {
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
    // 取到的只是一个词或短语时，用户要的是释义而不是一句译文（有道那种取词卡）。
    // 判定放在取词之后：划词与悬停都可能取到单个词，重试走 Job::Text 时场景已经定好了。
    let scene = match scene {
        Scene::Selection | Scene::Hover if ai_translate::scene::looks_like_word_in(&text, dir) => Scene::Word,
        other => other,
    };
    let _ = ev.send(Event::Captured { seq, text: text.clone(), source, scene, took_ms: t0.elapsed().as_millis() });

    // 查词场景先给一份离线释义：模型有时要好几秒，干等着像卡死
    // （Jack 2026-09-20 报）。模型的词条回来会把它替换掉。
    if scene == Scene::Word {
        if let Some(d) = dict {
            let t = std::time::Instant::now();
            let found = d.lock().unwrap().lookup(&text);
            match found {
                Some((e, restored)) => {
                    tracing::info!("离线词典命中 {:?}（{} µs）", e.word, t.elapsed().as_micros());
                    let _ = ev.send(Event::Offline { seq, text: crate::offline::to_markers(&e, restored, &text) });
                }
                None => tracing::debug!("离线词典里没有 {text:?}"),
            }
        }
    }

    // 第二步：翻译
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
        // 用一句话而不是单个词：单个词会被改判成取词场景、走词典条目（见下一条测试）
        let a = e.submit(Job::Text { scene: Scene::Selection, text: "hello world, how are you".into() });
        assert_eq!(a, 1);
        assert_eq!(wait_done(&rx).as_deref(), Some("zh=hello world, how are you"));
    }

    /// 主窗口手动输入：英译中模式下打中文要反过来翻成英文，而不是被当成「已经是中文」退回
    #[test]
    fn manual_input_picks_direction_by_content() {
        let mock = mock_llm::spawn(0);
        let mut cfg = ai_translate::TranslatorConfig::default();
        cfg.base.openai_base_url = mock.base_url.clone();
        let tr = Arc::new(Mutex::new(Translator::new(cfg).unwrap()));
        let (e, rx) = Engine::spawn(tr, Arc::new(Capturer::new()));
        e.submit(Job::Manual { text: "今天天气很好，我们出去走走吧".into() });
        let done = wait_done(&rx).expect("应当翻出来");
        assert!(done.starts_with("en="), "中文输入应走中译英：{done}");
        e.submit(Job::Manual { text: "It is a fine day, let us go for a walk.".into() });
        let done = wait_done(&rx).expect("应当翻出来");
        assert!(done.starts_with("zh="), "英文输入应走英译中：{done}");
    }

    /// 划词取到单个词要改判成 `Scene::Word`、拿到词典条目，而不是一句译文。
    /// 假后端在提示词含「词典条目」时回行标记条目，所以这条能在自动化里真正走通。
    #[test]
    fn single_word_selection_is_rerouted_to_dictionary() {
        let mock = mock_llm::spawn(0);
        let mut cfg = ai_translate::TranslatorConfig::default();
        cfg.base.openai_base_url = mock.base_url.clone();
        let tr = Arc::new(Mutex::new(Translator::new(cfg).unwrap()));
        let (e, rx) = Engine::spawn(tr, Arc::new(Capturer::new()));
        e.submit(Job::Text { scene: Scene::Selection, text: "hello".into() });
        let mut captured_scene = None;
        let mut done = None;
        for _ in 0..10 {
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Event::Captured { scene, .. }) => captured_scene = Some(scene),
                Ok(Event::Done { text, .. }) => {
                    done = Some(text);
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert_eq!(captured_scene, Some(Scene::Word), "单个词应改判成取词场景，界面标题靠它变成「取词释义」");
        let entry = crate::dict::parse(&done.expect("没等到结果")).expect("应当是可解析的词典条目");
        assert_eq!(entry.head, "hello");
        assert!(!entry.senses.is_empty());
    }

    /// 等到 Done 为止，返回译文
    fn wait_done(rx: &crossbeam_channel::Receiver<Event>) -> Option<String> {
        for _ in 0..10 {
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Event::Done { text, .. }) => return Some(text),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    }
}
