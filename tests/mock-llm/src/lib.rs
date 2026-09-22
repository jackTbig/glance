//! mock-llm：OpenAI 兼容假服务，供 ai-translate 集成测试与插件端到端测试使用。
//!
//! 行为约定：
//! - `POST /chat/completions`：取最后一条 user 消息文本。对每一行 `text`，输出 `zh=<text>`（占位符字符原样保留）。
//!   `stream=true` 按 SSE 分 3 片吐，否则普通 JSON。
//! - 文本含 `[[429]]` → 该文本首次返回 429 + `Retry-After: 0`，第二次正常；
//!   `[[500]]` → 永远 500；`[[slow:N]]` → 延迟 N ms；`[[drop:K]]` → 回包中删掉以 `⟨K⟩` 开头的那一行。
//! - `GET /stats` → `{ "requests", "max_inflight", "inflight" }`；`POST /reset` 清零。
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
    routing::{get, post},
    Json, Router,
};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    pub requests: u64,
    pub max_inflight: u64,
    pub inflight: u64,
}

#[derive(Default)]
struct Inner {
    stats: Stats,
    seen_429: HashSet<String>,
}

type Shared = Arc<Mutex<Inner>>;

fn last_user_text(body: &serde_json::Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|m| m.iter().rev().find(|x| x["role"] == "user"))
        .and_then(|x| x["content"].as_str())
        .unwrap_or("")
        .to_string()
}

fn system_text(body: &serde_json::Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|m| m.iter().find(|x| x["role"] == "system"))
        .and_then(|x| x["content"].as_str())
        .unwrap_or("")
        .to_string()
}

fn marker_num(text: &str, key: &str) -> Option<u64> {
    let start = text.find(&format!("[[{key}:"))? + key.len() + 3;
    let end = text[start..].find("]]")? + start;
    text[start..end].trim().parse().ok()
}

/// 取词场景（提示词含「词典条目」）回一份固定的行标记条目。
///
/// 只回 `zh=` 的话，插件与桌面端的词典卡永远走「解析失败 → 退回普通译文」那条路，
/// 真正的词典版式在自动化里一次都画不出来。词头用请求里的那个词，好让测试断言对得上。
fn dict_entry(text: &str) -> Option<String> {
    if !text.contains("词典条目") {
        return None;
    }
    let word = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.ends_with('：') && !l.ends_with(':')).last()?;
    Some(format!(
        "#词 {word}\n#音 英 /mɒk/ 美 /mɑːk/\n#释 v. 假装；模仿\n#释 n. 仿制品\n#形 过去式 mocked\n#例 This is a mock entry.\n#译 这是一条假条目。"
    ))
}

/// 每行前加 zh=；带 ⟨n⟩ 编号的行把 zh= 放在编号之后；以「：」结尾的指令行跳过
fn translate_lines(text: &str) -> String {
    let drop = marker_num(text, "drop");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.ends_with('：') && !l.ends_with(':'))
        .filter(|l| match drop {
            Some(k) => !l.starts_with(&format!("⟨{k}⟩")),
            None => true,
        })
        .map(|l| match l.strip_prefix('⟨').and_then(|r| r.find('⟩')) {
            Some(i) => {
                let (num, rest) = l.split_at(i + '⟨'.len_utf8() + '⟩'.len_utf8());
                format!("{num} zh={}", rest.trim())
            }
            None => format!("zh={l}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct InflightGuard(Shared);
impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap().stats.inflight -= 1;
    }
}

async fn chat(State(st): State<Shared>, Json(body): Json<serde_json::Value>) -> Response {
    {
        let mut g = st.lock().unwrap();
        g.stats.requests += 1;
        g.stats.inflight += 1;
        g.stats.max_inflight = g.stats.max_inflight.max(g.stats.inflight);
    }
    let _guard = InflightGuard(st.clone());
    let text = last_user_text(&body);
    if let Some(ms) = marker_num(&text, "slow") {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    if text.contains("[[500]]") {
        return (StatusCode::INTERNAL_SERVER_ERROR, "mock: permanent failure").into_response();
    }
    if text.contains("[[429]]") {
        let first = st.lock().unwrap().seen_429.insert(text.clone());
        if first {
            let mut h = HeaderMap::new();
            h.insert("retry-after", "0".parse().unwrap());
            return (StatusCode::TOO_MANY_REQUESTS, h, "mock: rate limited").into_response();
        }
    }
    let mut out = dict_entry(&text).unwrap_or_else(|| translate_lines(&text));
    // 中译英的系统提示词里写的是「翻译成英文」：前缀换成 en=，测试才分得清走的是哪个方向
    if system_text(&body).contains("翻译成英文") {
        out = out.replace("zh=", "en=");
    }
    let stream = body["stream"].as_bool().unwrap_or(false);
    if !stream {
        return Json(serde_json::json!({
            "id": "mock", "object": "chat.completion",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": out }, "finish_reason": "stop" }]
        }))
        .into_response();
    }
    // 按字符分 3 片
    let chars: Vec<char> = out.chars().collect();
    let n = chars.len();
    let cut = |i: usize| (n * i) / 3;
    let pieces: Vec<String> = (0..3).map(|i| chars[cut(i)..cut(i + 1)].iter().collect()).collect();
    let mut events: Vec<Result<Event, std::convert::Infallible>> = pieces
        .into_iter()
        .map(|p| {
            Ok(Event::default().data(
                serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": p } }] }).to_string(),
            ))
        })
        .collect();
    events.push(Ok(Event::default().data("[DONE]")));
    Sse::new(futures::stream::iter(events)).into_response()
}

async fn stats(State(st): State<Shared>) -> Json<Stats> {
    Json(st.lock().unwrap().stats.clone())
}

async fn reset(State(st): State<Shared>) -> StatusCode {
    let mut g = st.lock().unwrap();
    g.stats = Stats::default();
    g.seen_429.clear();
    StatusCode::OK
}

fn router(st: Shared) -> Router {
    Router::new()
        .route("/chat/completions", post(chat))
        .route("/v1/chat/completions", post(chat))
        .route("/stats", get(stats))
        .route("/reset", post(reset))
        .with_state(st)
}

pub struct MockServer {
    pub port: u16,
    pub base_url: String,
    _thread: std::thread::JoinHandle<()>,
}

/// 起服务（port = 0 随机端口），阻塞到端口可连后返回
pub fn spawn(port: u16) -> MockServer {
    let (tx, rx) = std::sync::mpsc::channel::<u16>();
    let thread = std::thread::Builder::new()
        .name("mock-llm".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
                let actual = listener.local_addr().unwrap().port();
                tx.send(actual).unwrap();
                axum::serve(listener, router(Shared::default())).await.unwrap();
            });
        })
        .unwrap();
    let port = rx.recv().unwrap();
    MockServer { port, base_url: format!("http://127.0.0.1:{port}"), _thread: thread }
}

/// 极简 HTTP/1.0 客户端（不引 reqwest，避免与被测 crate 的版本纠缠）
pub fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

impl MockServer {
    pub fn stats(&self) -> Stats {
        let (_, body) = http(self.port, "GET", "/stats", "");
        serde_json::from_str(&body).unwrap()
    }
    pub fn reset(&self) {
        http(self.port, "POST", "/reset", "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_stream_echoes_zh_prefix_and_counts() {
        let s = spawn(0);
        let body = serde_json::json!({ "model": "x", "messages": [{ "role": "user", "content": "hello" }] }).to_string();
        let (code, resp) = http(s.port, "POST", "/chat/completions", &body);
        assert_eq!(code, 200);
        let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "zh=hello");
        assert_eq!(s.stats().requests, 1);
    }

    #[test]
    fn markers_429_then_ok_and_500_permanent() {
        let s = spawn(0);
        let mk = |t: &str| serde_json::json!({ "messages": [{ "role": "user", "content": t }] }).to_string();
        assert_eq!(http(s.port, "POST", "/chat/completions", &mk("a [[429]]")).0, 429);
        assert_eq!(http(s.port, "POST", "/chat/completions", &mk("a [[429]]")).0, 200);
        assert_eq!(http(s.port, "POST", "/chat/completions", &mk("b [[500]]")).0, 500);
        assert_eq!(http(s.port, "POST", "/chat/completions", &mk("b [[500]]")).0, 500);
    }

    #[test]
    fn drop_marker_removes_numbered_line() {
        assert_eq!(translate_lines("请翻译以下各段：\n⟨1⟩ a [[drop:2]]\n⟨2⟩ b\n⟨3⟩ c"), "⟨1⟩ zh=a [[drop:2]]\n⟨3⟩ zh=c");
    }

    #[test]
    fn word_lookup_gets_a_dictionary_entry() {
        let out = dict_entry("请给出这个词的词典条目：\nglance").unwrap();
        assert!(out.starts_with("#词 glance\n"), "{out}");
        assert!(out.contains("#释 v. ") && out.contains("#例 ") && out.contains("#译 "));
        // 普通翻译请求不受影响
        assert!(dict_entry("请翻译：\nhello").is_none());
    }
}
