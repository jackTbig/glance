use crossbeam_channel::bounded;
use live_caption::messages::UiMsg;
use live_caption::translate::{spawn_translator, TranslatorConfig};
use std::time::Duration;

fn cfg(rotate_every: u32, timeout_secs: u64) -> TranslatorConfig {
    TranslatorConfig {
        command: vec!["python".into(), "tests/fake_claude.py".into()],
        model: "fake".into(),
        timeout: Duration::from_secs(timeout_secs),
        rotate_every,
        max_backlog: 10,
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
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(64);
    let _h = spawn_translator(cfg(2, 5), rx, ui_tx);
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
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(64);
    let _h = spawn_translator(cfg(100, 1), rx, ui_tx);
    tx.send((1, "SLEEP please".into())).unwrap();
    let (id1, t1) = next_chinese(&ui_rx);
    assert_eq!(id1, 1);
    assert_eq!(t1, "（翻译超时）");
    tx.send((2, "after".into())).unwrap();
    let (id2, t2) = next_chinese(&ui_rx);
    assert_eq!(id2, 2);
    assert!(t2.ends_with("zh=after"));
}
