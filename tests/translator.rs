use crossbeam_channel::bounded;
use live_caption::messages::UiMsg;
use live_caption::translate::{new_context, push_context, spawn_partial_translator, spawn_translator, Backend, TranslatorConfig};
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
        partial_translate: true,
        context_sentences: 2,
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
    let _h = spawn_translator(cfg, cfg_rx, rx, ui_tx, new_context());
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
    let _h = spawn_translator(cfg, cfg_rx, rx, ui_tx, new_context());
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
    let _h = spawn_translator(base_cfg(), cfg_rx, rx, ui_tx, new_context());
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
fn settings_test_translate_roundtrip() {
    let (zh, took) = live_caption::translate::test_translate(&base_cfg(), "ping").expect("测试翻译应成功");
    assert!(zh.ends_with("zh=ping"), "{zh}");
    assert!(took < Duration::from_secs(10));
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
    let _h = spawn_translator(base_cfg(), cfg_rx, rx, ui_tx, new_context());
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

fn next_provisional(rx: &crossbeam_channel::Receiver<UiMsg>) -> (u64, String) {
    loop {
        match rx.recv_timeout(Duration::from_secs(15)).expect("等临时译文超时") {
            UiMsg::ChineseProvisional { id, text } => return (id, text),
            _ => continue,
        }
    }
}

/// 收临时译文直到出现以 want 结尾的一条，返回它和之前收到的全部临时译文（临时翻译不流式，正常情况下之前应为空）
fn provisional_until(rx: &crossbeam_channel::Receiver<UiMsg>, want: &str) -> (u64, String, Vec<String>) {
    let t0 = Instant::now();
    let mut seen = Vec::new();
    loop {
        let (id, t) = next_provisional(rx);
        if t.ends_with(want) {
            return (id, t, seen);
        }
        assert!(t0.elapsed() < Duration::from_secs(15), "没等到 {want:?}，最后一条 {t:?}");
        seen.push(t);
    }
}

#[test]
fn final_translation_carries_previous_sentences_as_context() {
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let ctx = new_context();
    let _h = spawn_translator(base_cfg(), cfg_rx, rx, ui_tx, ctx.clone());
    tx.send((1, "one".into())).unwrap();
    let (_, t1) = next_chinese(&ui_rx);
    assert!(t1.contains(" ctx=0 "), "第一句没有前文：{t1}");
    tx.send((2, "two".into())).unwrap();
    let (_, t2) = next_chinese(&ui_rx);
    assert!(t2.contains(" ctx=1 ") && t2.contains("prov=0"), "第二句应带 1 句前文：{t2}");
    tx.send((3, "three".into())).unwrap();
    let (_, t3) = next_chinese(&ui_rx);
    assert!(t3.contains(" ctx=2 "), "第三句带 2 句前文：{t3}");
    tx.send((4, "four".into())).unwrap();
    let (_, t4) = next_chinese(&ui_rx);
    assert!(t4.contains(" ctx=2 "), "上限 2 句：{t4}");
    assert!(t4.ends_with("zh=four"));
}

#[test]
fn provisional_thread_translates_half_sentence_with_note_and_context() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let ctx = new_context();
    push_context(&ctx, "Earlier sentence.", "前一句。", 2);
    let _h = spawn_partial_translator(base_cfg(), cfg_rx, rx, ui_tx, ctx);
    tx.send((7, "so when you have".into())).unwrap();
    let (id, t, earlier) = provisional_until(&ui_rx, "zh=so when you have");
    assert_eq!(id, 7);
    assert!(earlier.is_empty(), "临时翻译不流式上屏，只该有一条：{earlier:?}");
    assert!(t.contains(" ctx=1 ") && t.contains(" prov=1 ") && t.contains(" prev=0 "), "应带前文与半句标记、首次没有已译文本：{t}");
}

#[test]
fn provisional_thread_translates_on_change_and_continues_previous() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_partial_translator(base_cfg(), cfg_rx, rx, ui_tx, new_context());
    tx.send((1, "a b c".into())).unwrap();
    let (_, t1, _) = provisional_until(&ui_rx, "zh=a b c");
    assert!(t1.contains(" prev=0 "), "首次没有已译文本：{t1}");
    // 同一快照重复到来：不翻
    tx.send((1, "a b c".into())).unwrap();
    tx.send((1, "a b c ".into())).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(ui_rx.try_recv().is_err(), "英文没变不该再翻");
    // 英文一变就翻，且带上此前译出的临时中文让模型续写
    tx.send((1, "a b c d".into())).unwrap();
    let (_, t2, earlier) = provisional_until(&ui_rx, "zh=a b c d");
    assert!(earlier.is_empty(), "{earlier:?}");
    assert!(t2.contains(" prev=1 "), "第二次应带此前已译文本：{t2}");
}

#[test]
fn provisional_thread_takes_newest_while_busy() {
    // 线程忙着翻第一条时连发三条：翻完后只翻最新那条
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_partial_translator(base_cfg(), cfg_rx, rx, ui_tx, new_context());
    tx.send((1, "one two three four five six seven eight nine ten".into())).unwrap();
    // 让线程先把第一条取走并进入翻译（取走是瞬间的，翻译要几百毫秒）
    std::thread::sleep(Duration::from_millis(100));
    tx.send((1, "x".into())).unwrap();
    tx.send((1, "x y".into())).unwrap();
    tx.send((1, "x y z".into())).unwrap();
    let (_, first, _) = provisional_until(&ui_rx, "zh=one two three four five six seven eight nine ten");
    assert!(first.contains(" prev=0 "));
    let (_, second, earlier) = provisional_until(&ui_rx, "zh=x y z");
    assert!(earlier.is_empty(), "中间两条应作废：{earlier:?}");
    assert!(second.contains(" prev=1 "));
    std::thread::sleep(Duration::from_millis(600));
    assert!(ui_rx.try_recv().is_err(), "没有更多翻译");
}

#[test]
fn provisional_thread_resets_counter_on_new_utterance() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (_cfg_tx, cfg_rx) = bounded(4);
    let _h = spawn_partial_translator(base_cfg(), cfg_rx, rx, ui_tx, new_context());
    tx.send((1, "one two three four five six".into())).unwrap();
    let _ = provisional_until(&ui_rx, "zh=one two three four five six");
    // 新话轮：此前译出的临时中文清零，不带到下一句
    tx.send((2, "new one here".into())).unwrap();
    let (id, t, _) = provisional_until(&ui_rx, "zh=new one here");
    assert_eq!(id, 2);
    assert!(t.contains(" prev=0 "), "新话轮不该带上一话轮的已译文本：{t}");
}

#[test]
fn provisional_thread_is_idle_when_disabled_or_codex() {
    let (tx, rx) = bounded(4);
    let (ui_tx, ui_rx) = bounded(256);
    let (cfg_tx, cfg_rx) = bounded(4);
    let mut cfg = base_cfg();
    cfg.partial_translate = false;
    let _h = spawn_partial_translator(cfg, cfg_rx, rx, ui_tx, new_context());
    tx.send((1, "a b c d".into())).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(ui_rx.try_recv().is_err(), "关闭时不翻");
    let mut on = base_cfg();
    on.backend = Backend::Codex;
    cfg_tx.send(on).unwrap();
    tx.send((1, "a b c d e f g".into())).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    assert!(ui_rx.try_recv().is_err(), "codex 后端不翻");
}

#[test]
fn final_reuses_last_provisional_when_words_match() {
    // 临时线程和定稿线程共享同一份 Context：定稿英文只差标点时直接复用临时译文，不再调后端
    let ctx = new_context();
    let (ptx, prx) = bounded(4);
    let (ftx, frx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(256);
    let (_c1, c1rx) = bounded(4);
    let (_c2, c2rx) = bounded(4);
    let _p = spawn_partial_translator(base_cfg(), c1rx, prx, ui_tx.clone(), ctx.clone());
    let _f = spawn_translator(base_cfg(), c2rx, frx, ui_tx, ctx);
    ptx.send((9, "good morning everyone".into())).unwrap();
    let (_, prov, _) = provisional_until(&ui_rx, "zh=good morning everyone");
    assert!(prov.contains(" prov=1 "));
    let t0 = Instant::now();
    ftx.send((9, "Good morning everyone!".into())).unwrap();
    let (id, zh) = next_chinese(&ui_rx);
    assert_eq!(id, 9);
    assert_eq!(zh, prov, "定稿应原样复用临时译文（假后端回显里带 prov=1 可证明没再调用）");
    assert!(t0.elapsed() < Duration::from_millis(300), "复用不该有后端往返：{:?}", t0.elapsed());
    // 英文有实质变化就照常翻
    ptx.send((10, "see you".into())).unwrap();
    let _ = provisional_until(&ui_rx, "zh=see you");
    ftx.send((10, "see you tomorrow".into())).unwrap();
    let (_, zh2) = next_chinese(&ui_rx);
    assert!(zh2.contains(" prov=0 ") && zh2.ends_with("zh=see you tomorrow"), "{zh2}");
}
