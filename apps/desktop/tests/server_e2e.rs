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
