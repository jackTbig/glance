//! 本地服务端到端：起 mock-llm 假后端 + 起本地服务，用裸 TCP 冒充 Chrome 插件请求
use ai_translate::Translator;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use translate_tool::config::Config;
use translate_tool::server;

fn http(port: u16, method: &str, path: &str, origin: Option<&str>, body: &str) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let o = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{o}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let resp = String::from_utf8_lossy(&buf).into_owned();
    let status: u16 = resp.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn setup() -> (mock_llm::MockServer, server::ServerHandle) {
    let mock = mock_llm::spawn(0);
    let mut cfg = Config::default();
    cfg.backend.openai_base_url = mock.base_url.clone();
    cfg.backend.openai_model = "mock".into();
    cfg.backend.max_retries = 0;
    let tr = Translator::new(cfg.translator_config()).unwrap();
    let h = server::spawn(0, cfg.server.allowed_extension_ids.clone(), Arc::new(Mutex::new(tr)), "test").unwrap();
    (mock, h)
}

#[test]
fn health_reports_ok() {
    let (_m, h) = setup();
    let (code, body) = http(h.port, "GET", "/v1/health", Some("chrome-extension://abc"), "");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["app"], "translate-tool");
    h.stop();
}

#[test]
fn web_page_origin_is_rejected() {
    let (_m, h) = setup();
    let (code, body) = http(h.port, "GET", "/v1/health", Some("https://evil.example.com"), "");
    assert_eq!(code, 403, "{body}");
    h.stop();
}

#[test]
fn full_page_batch_translates() {
    let (_m, h) = setup();
    let req = serde_json::json!({
        "scene": "full_page",
        "items": [
            { "id": "p1", "text": "Hello world" },
            { "id": "p2", "text": "Click <b>here</b>", "is_html": true },
            { "id": "p3", "text": "这段已经是中文" }
        ]
    })
    .to_string();
    let (code, body) = http(h.port, "POST", "/v1/translate", Some("chrome-extension://abc"), &req);
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rs = v["results"].as_array().unwrap();
    assert_eq!(rs.len(), 3);
    assert_eq!(rs[0]["text"], "zh=Hello world");
    assert_eq!(rs[1]["text"], "zh=Click <b>here</b>", "行内标签应还原");
    assert_eq!(rs[2]["text"], "这段已经是中文", "中文段原样返回");
    h.stop();
}

#[test]
fn selection_single_item_uses_streaming_path() {
    let (_m, h) = setup();
    let req = serde_json::json!({ "scene": "selection", "items": [{ "id": "s", "text": "hello" }] }).to_string();
    let (code, body) = http(h.port, "POST", "/v1/translate", Some("chrome-extension://abc"), &req);
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["results"][0]["text"], "zh=hello");
    h.stop();
}

#[test]
fn bad_scene_and_empty_items_are_400() {
    let (_m, h) = setup();
    let bad = serde_json::json!({ "scene": "nope", "items": [{ "id": "a", "text": "x" }] }).to_string();
    assert_eq!(http(h.port, "POST", "/v1/translate", None, &bad).0, 400);
    let empty = serde_json::json!({ "scene": "hover", "items": [] }).to_string();
    assert_eq!(http(h.port, "POST", "/v1/translate", None, &empty).0, 400);
    assert_eq!(http(h.port, "POST", "/v1/translate", None, "not json").0, 400);
    h.stop();
}

#[test]
fn options_preflight_returns_cors_headers() {
    let (_m, h) = setup();
    let mut s = std::net::TcpStream::connect(("127.0.0.1", h.port)).unwrap();
    s.write_all(b"OPTIONS /v1/translate HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: chrome-extension://abc\r\nConnection: close\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let resp = String::from_utf8_lossy(&buf).to_lowercase();
    assert!(resp.contains("204"), "{resp}");
    assert!(resp.contains("access-control-allow-origin"), "{resp}");
    h.stop();
}

#[test]
fn unknown_path_is_404() {
    let (_m, h) = setup();
    assert_eq!(http(h.port, "GET", "/nope", None, "").0, 404);
    h.stop();
}

#[test]
fn slow_request_does_not_block_others() {
    // 插件是一段一个请求发的：一段慢（或在服务端重试）不能把后面的请求堵住，
    // 否则「翻好一段显示一段」就退化成了排队等待。
    let (_m, h) = setup();
    let t0 = std::time::Instant::now();
    let port = h.port;

    // 先发一个要 1.5 秒的
    let slow = std::thread::spawn(move || {
        let body = serde_json::json!({ "scene": "hover", "items": [{ "id": "s", "text": "slow one [[slow:1500]]" }] }).to_string();
        http(port, "POST", "/v1/translate", Some("chrome-extension://x"), &body)
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    // 再发一个快的，它不该等前面那个
    let body = serde_json::json!({ "scene": "hover", "items": [{ "id": "f", "text": "fast one" }] }).to_string();
    let (code, resp) = http(port, "POST", "/v1/translate", Some("chrome-extension://x"), &body);
    let fast_ms = t0.elapsed().as_millis();

    assert_eq!(code, 200, "{resp}");
    assert!(fast_ms < 1200, "快的那个等了 {fast_ms}ms，说明被慢请求堵住了");

    let (slow_code, _) = slow.join().unwrap();
    assert_eq!(slow_code, 200);
    h.stop();
}

#[test]
fn many_single_item_requests_all_succeed() {
    // 整页翻译现在是一段一个请求，同时飞好几个，服务端要都能处理
    let (_m, h) = setup();
    let port = h.port;
    let handles: Vec<_> = (0..12)
        .map(|i| {
            std::thread::spawn(move || {
                let body =
                    serde_json::json!({ "scene": "full_page", "items": [{ "id": i.to_string(), "text": format!("paragraph {i}") }] })
                        .to_string();
                http(port, "POST", "/v1/translate", Some("chrome-extension://x"), &body)
            })
        })
        .collect();
    for (i, hd) in handles.into_iter().enumerate() {
        let (code, body) = hd.join().unwrap();
        assert_eq!(code, 200, "第 {i} 个请求失败：{body}");
        assert!(body.contains("zh=paragraph"), "第 {i} 个回包不对：{body}");
    }
    h.stop();
}
