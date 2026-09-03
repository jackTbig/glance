use crossbeam_channel::bounded;
use live_caption::messages::UiMsg;
use live_caption::translate::{spawn_translator, Backend, TranslatorConfig};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn base_cfg() -> TranslatorConfig {
    TranslatorConfig {
        backend: Backend::Claude,
        claude_bin: "claude".into(),
        claude_model: "fake".into(),
        codex_bin: "codex".into(),
        codex_model: String::new(),
        openai_base_url: String::new(),
        openai_api_key: String::new(),
        openai_model: "fake".into(),
        timeout: Duration::from_secs(5),
        rotate_every: 100,
        max_backlog: 10,
        command_override: vec!["python".into(), "tests/fake_claude.py".into()],
    }
}

fn next_chinese(rx: &crossbeam_channel::Receiver<UiMsg>) -> (u64, String) {
    loop {
        match rx.recv_timeout(Duration::from_secs(15)).expect("等译文超时") {
            UiMsg::Chinese { id, text } => return (id, text),
            _ => continue,
        }
    }
}

fn pid_of(s: &str) -> &str {
    s.split_whitespace().next().unwrap()
}

#[test]
fn translates_in_order_and_rotates_session() {
    let mut cfg = base_cfg();
    cfg.rotate_every = 2;
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_translator(cfg, cfg_rx, rx, ui_tx);
    tx.send((1, "one".into())).unwrap();
    tx.send((2, "two".into())).unwrap();
    tx.send((3, "three".into())).unwrap();
    let (id1, t1) = next_chinese(&ui_rx);
    let (id2, t2) = next_chinese(&ui_rx);
    let (id3, t3) = next_chinese(&ui_rx);
    assert_eq!((id1, id2, id3), (1, 2, 3));
    assert!(t1.ends_with("zh=one") && t2.ends_with("zh=two") && t3.ends_with("zh=three"));
    assert_eq!(pid_of(&t1), pid_of(&t2), "前两句同一会话");
    assert_ne!(pid_of(&t2), pid_of(&t3), "第三句应已轮换新会话");
}

#[test]
fn timeout_restarts_session_and_continues() {
    let mut cfg = base_cfg();
    cfg.timeout = Duration::from_secs(1);
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_translator(cfg, cfg_rx, rx, ui_tx);
    tx.send((1, "SLEEP please".into())).unwrap();
    let (id1, t1) = next_chinese(&ui_rx);
    assert_eq!(id1, 1);
    assert_eq!(t1, "（翻译超时）");
    tx.send((2, "after".into())).unwrap();
    let (id2, t2) = next_chinese(&ui_rx);
    assert_eq!(id2, 2);
    assert!(t2.ends_with("zh=after"));
}

#[test]
fn streams_partials_before_final() {
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_translator(base_cfg(), cfg_rx, rx, ui_tx);
    tx.send((1, "alpha beta gamma".into())).unwrap();
    let mut partials = Vec::new();
    let final_text;
    loop {
        match ui_rx.recv_timeout(Duration::from_secs(15)).expect("超时") {
            UiMsg::ChinesePartial { id, text } => {
                assert_eq!(id, 1);
                partials.push(text);
            }
            UiMsg::Chinese { id, text } => {
                assert_eq!(id, 1);
                final_text = text;
                break;
            }
            _ => {}
        }
    }
    assert!(partials.len() >= 3, "应有逐词增量，实际 {}", partials.len());
    for w in partials.windows(2) {
        assert!(w[1].starts_with(&w[0]), "增量必须是累积前缀：{:?} -> {:?}", w[0], w[1]);
    }
    assert_eq!(partials.last().unwrap(), &final_text);
}

#[test]
fn hot_switch_to_openai_backend_via_cfg_channel() {
    // 起假的 OpenAI 服务
    let port = 18931;
    let mut server = std::process::Command::new("python")
        .args(["tests/fake_openai.py", &port.to_string()])
        .spawn()
        .expect("python");
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(t0.elapsed() < Duration::from_secs(10), "假服务没起来");
        std::thread::sleep(Duration::from_millis(100));
    }

    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(256);
    let (cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_translator(base_cfg(), cfg_rx, rx, ui_tx);
    tx.send((1, "first".into())).unwrap();
    let (_, t1) = next_chinese(&ui_rx);
    assert!(t1.ends_with("zh=first"));

    let mut oc = base_cfg();
    oc.backend = Backend::OpenAi;
    oc.openai_base_url = format!("http://127.0.0.1:{port}/v1");
    cfg_tx.send(oc).unwrap();
    tx.send((2, "second via http".into())).unwrap();
    let mut partials = 0;
    let t2 = loop {
        match ui_rx.recv_timeout(Duration::from_secs(15)).expect("超时") {
            UiMsg::ChinesePartial { id: 2, .. } => partials += 1,
            UiMsg::Chinese { id: 2, text } => break text,
            _ => {}
        }
    };
    let _ = server.kill();
    let _ = server.wait();
    assert!(t2.ends_with("zh=second via http"), "{t2}");
    assert!(partials >= 2, "OpenAI 后端也应流式增量，实际 {partials}");
    assert_ne!(pid_of(&t1), pid_of(&t2), "两句应来自不同后端进程");
}
