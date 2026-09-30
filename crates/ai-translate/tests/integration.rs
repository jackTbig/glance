//! 用 mock-llm 假服务做的集成测试：不依赖真实 Key
use ai_translate::translator::{Item, Translator, TranslatorConfig};
use ai_translate::{Backend, RetryPolicy, Scene};
use std::time::Duration;

fn setup(concurrency: usize, batch_size: usize) -> (mock_llm::MockServer, Translator) {
    let server = mock_llm::spawn(0);
    let mut cfg = TranslatorConfig::default();
    cfg.base.backend = Backend::OpenAi;
    cfg.base.openai_base_url = server.base_url.clone();
    cfg.base.openai_model = "mock".into();
    cfg.base.timeout = Duration::from_secs(10);
    cfg.concurrency = concurrency;
    cfg.batch_size = batch_size;
    cfg.retry = RetryPolicy { max_retries: 3, base_delay: Duration::from_millis(1) };
    let t = Translator::new(cfg).unwrap();
    (server, t)
}

fn items(texts: &[&str]) -> Vec<Item> {
    texts.iter().enumerate().map(|(i, t)| Item { id: format!("p{i}"), text: t.to_string(), is_html: false }).collect()
}

#[test]
fn single_translate_streams_and_returns() {
    let (s, t) = setup(4, 10);
    let mut calls = 0;
    let out = t.translate(Scene::Selection, "hello", None, &mut |_| calls += 1).unwrap();
    assert_eq!(out, "zh=hello");
    assert!(calls >= 2, "流式回调至少两次，实际 {calls}");
    assert_eq!(s.stats().requests, 1);
}

#[test]
fn batch_respects_concurrency_limit() {
    let (s, t) = setup(3, 1);
    let texts: Vec<String> = (0..40).map(|i| format!("line {i} [[slow:30]]")).collect();
    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let rs = t.translate_batch(Scene::FullPage, items(&refs));
    assert!(rs.iter().all(|r| r.result.is_ok()));
    assert_eq!(rs[5].result.as_ref().unwrap(), "zh=line 5 [[slow:30]]");
    let st = s.stats();
    assert!(st.max_inflight <= 3, "max_inflight={}", st.max_inflight);
    assert_eq!(st.requests, 40);
}

#[test]
fn batch_packs_by_batch_size() {
    let (s, t) = setup(8, 10);
    let texts: Vec<String> = (0..25).map(|i| format!("para {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let rs = t.translate_batch(Scene::FullPage, items(&refs));
    assert_eq!(rs.len(), 25);
    assert_eq!(rs[24].id, "p24");
    assert_eq!(rs[24].result.as_ref().unwrap(), "zh=para 24");
    assert_eq!(s.stats().requests, 3);
}

#[test]
fn html_items_round_trip() {
    let (_s, t) = setup(2, 10);
    let it = vec![Item { id: "a".into(), text: "Hi <b>x</b> and <a href=\"u\">y</a>".into(), is_html: true }];
    let rs = t.translate_batch(Scene::FullPage, it);
    assert_eq!(rs[0].result.as_ref().unwrap(), "zh=Hi <b>x</b> and <a href=\"u\">y</a>");
}

#[test]
fn retry_on_429_then_success() {
    let (s, t) = setup(2, 10);
    let out = t.translate(Scene::Hover, "x [[429]]", None, &mut |_| {}).unwrap();
    assert_eq!(out, "zh=x [[429]]");
    assert_eq!(s.stats().requests, 2);
}

#[test]
fn permanent_500_is_error_for_that_item_only() {
    let (_s, t) = setup(2, 1);
    let rs = t.translate_batch(Scene::FullPage, items(&["good", "bad [[500]]"]));
    assert_eq!(rs[0].result.as_ref().unwrap(), "zh=good");
    assert!(rs[1].result.is_err());
}

#[test]
fn dropped_line_falls_back_to_single() {
    let (s, t) = setup(2, 10);
    let rs = t.translate_batch(Scene::FullPage, items(&["a [[drop:2]]", "b", "c"]));
    assert_eq!(rs[0].result.as_ref().unwrap(), "zh=a [[drop:2]]");
    assert_eq!(rs[1].result.as_ref().unwrap(), "zh=b", "第 2 段经单条兜底");
    assert_eq!(rs[2].result.as_ref().unwrap(), "zh=c");
    assert_eq!(s.stats().requests, 2, "一次整包 + 一次兜底");
}

#[test]
fn chinese_paragraph_is_skipped_locally() {
    let (s, t) = setup(2, 10);
    let rs = t.translate_batch(Scene::FullPage, items(&["这一段已经是中文了。", "   "]));
    assert_eq!(rs[0].result.as_ref().unwrap(), "这一段已经是中文了。");
    assert_eq!(rs[1].result.as_ref().unwrap(), "   ");
    assert_eq!(s.stats().requests, 0);
}

#[test]
fn glossary_goes_into_user_message() {
    let server = mock_llm::spawn(0);
    let mut cfg = TranslatorConfig::default();
    cfg.base.openai_base_url = server.base_url.clone();
    cfg.glossary = ai_translate::Glossary::new(vec![ai_translate::GlossaryEntry { src: "pod".into(), dst: "Pod".into() }]);
    let t = Translator::new(cfg).unwrap();
    let out = t.translate(Scene::Hover, "a pod here", None, &mut |_| {}).unwrap();
    // mock 会把用户消息里每一行都回显，术语块的那一行也在
    assert!(out.contains("pod → Pod"), "{out}");
}

/// 相同 (model, system_prompt, user_message) 第二次不再发请求：截屏/划词重复点很常见
#[test]
fn cache_hits_duplicate_chat() {
    let (s, t) = setup(4, 10);
    let _ = t.translate(Scene::Selection, "deploy failed", None, &mut |_| {}).unwrap();
    assert_eq!(s.stats().requests, 1);
    let _ = t.translate(Scene::Selection, "deploy failed", None, &mut |_| {}).unwrap();
    assert_eq!(s.stats().requests, 1, "重复同文本应走缓存，不再请求：{}", s.stats().requests);
}

/// 不同 system_prompt 的同文本不共享缓存（场景不同输出可能不同）
#[test]
fn cache_distinguishes_system_prompt() {
    let (s, t) = setup(4, 10);
    let _ = t.translate(Scene::Selection, "deploy failed", None, &mut |_| {}).unwrap();
    let _ = t.translate(Scene::Screenshot, "deploy failed", None, &mut |_| {}).unwrap();
    assert_eq!(s.stats().requests, 2, "不同场景的同文本走两条请求");
}

/// 配置的 enable_thinking 要真的上线：config → BackendConfig → OpenAi → 请求体，
/// 中间任何一环漏拷贝就悄悄退回「思考开」的慢速路径
#[test]
fn thinking_flag_reaches_the_wire() {
    let server = mock_llm::spawn(0);
    let mut cfg = TranslatorConfig::default();
    cfg.base.openai_base_url = server.base_url.clone();
    cfg.base.enable_thinking = Some(false);
    let t = Translator::new(cfg).unwrap();
    let out = t.translate(Scene::Selection, "hello", None, &mut |_| {}).unwrap();
    assert_eq!(out, "zh=hello");
    assert_eq!(server.stats().last_thinking, Some(false));
}

/// 不配置就不带参数：未验证的服务（DeepSeek 官方等）保持既有行为
#[test]
fn thinking_flag_absent_unless_configured() {
    let (s, t) = setup(4, 10);
    t.translate(Scene::Selection, "hello", None, &mut |_| {}).unwrap();
    assert_eq!(s.stats().last_thinking, None);
}
