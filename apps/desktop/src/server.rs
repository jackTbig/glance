//! 本地 HTTP 服务：只绑 127.0.0.1，给 Chrome 插件调用。
//!
//! 接口：
//! - `GET  /v1/health`        → 版本、后端、并发上限
//! - `POST /v1/translate`     → `{ scene, items: [{id, text, is_html}], context? }` → `{ results: [{id, text?, error?}] }`
//! - `POST /v1/lookup_offline`→ `{ text }` → `{ found: bool, text?: string }` — 插件划词先秒出本地词库条目，模型回来再覆盖
//! - `OPTIONS *`              → CORS 预检
//!
//! 来源校验：只接受 `chrome-extension://` 开头的 Origin（可在配置里进一步限定扩展 id）；
//! 网页发来的请求一律 403，防止任意站点借本机服务翻译。

use ai_translate::translator::Item;
use ai_translate::{Scene, Translator};

use std::sync::{Arc, Mutex};

pub const EXT_ORIGIN_PREFIX: &str = "chrome-extension://";

#[derive(Debug, Clone, PartialEq)]
pub enum OriginCheck {
    Allow,
    Deny(&'static str),
}

/// 允许哪些来源：只允许扩展；配置里列了 id 就再限定 id
pub fn check_origin(origin: Option<&str>, allowed_ids: &[String]) -> OriginCheck {
    let Some(o) = origin.map(str::trim).filter(|o| !o.is_empty()) else {
        // 没有 Origin 头：curl 或本机脚本，放行（便于排障，且本机已受 127.0.0.1 限制）
        return OriginCheck::Allow;
    };
    if !o.starts_with(EXT_ORIGIN_PREFIX) {
        return OriginCheck::Deny("只接受浏览器扩展的请求");
    }
    if allowed_ids.is_empty() {
        return OriginCheck::Allow;
    }
    let id = o.trim_start_matches(EXT_ORIGIN_PREFIX).trim_end_matches('/');
    if allowed_ids.iter().any(|a| a.eq_ignore_ascii_case(id)) {
        OriginCheck::Allow
    } else {
        OriginCheck::Deny("扩展 id 不在允许列表里")
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct TranslateRequest {
    pub scene: String,
    pub items: Vec<ReqItem>,
    #[serde(default)]
    pub context: Option<String>,
}

/// `POST /v1/lookup_offline` 的请求体
#[derive(Debug, serde::Deserialize)]
pub struct LookupOfflineRequest {
    pub text: String,
}

/// `POST /v1/lookup_offline` 的响应：`found=false` 表示没词库或查无此词
#[derive(Debug, serde::Serialize)]
pub struct LookupOfflineResponse {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReqItem {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub is_html: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct TranslateResponse {
    pub results: Vec<ResItem>,
}

#[derive(Debug, serde::Serialize)]
pub struct ResItem {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 词库：进程内只开一次。`None` = 没装词库或打开失败。`lookup` 要 &mut self，所以得用 Mutex
type SharedDict = Arc<Mutex<Option<offline_dict::Dict>>>;

/// 打开词库（找不到 / 打不开都返回 None，不报错——离线查词是"有就快"而不是"没就坏"）
pub fn open_shared_dict() -> SharedDict {
    let dict = crate::offline::dict_path()
        .and_then(|p| match offline_dict::Dict::open(&p) {
            Ok(d) => {
                tracing::info!("服务侧已加载离线词库：{} 条（{}）", d.len(), p.display());
                Some(d)
            }
            Err(e) => {
                tracing::warn!("服务侧打开离线词库失败：{e}");
                None
            }
        });
    Arc::new(Mutex::new(dict))
}

/// 查离线词库。返回 `(found, marker_text)`：found=false 表示没词库或查无此词，
/// text 始终是模型那套行标记格式，扩展端拿到直接调现有的 dict.ts 解析器画卡
pub fn handle_lookup_offline(dict: &SharedDict, text: &str) -> LookupOfflineResponse {
    let mut g = dict.lock().unwrap();
    let Some(dict) = g.as_mut() else {
        return LookupOfflineResponse { found: false, text: None };
    };
    match dict.lookup(text) {
        Some((entry, restored)) => {
            let markers = crate::offline::to_markers(&entry, restored, text);
            LookupOfflineResponse { found: true, text: Some(markers) }
        }
        None => LookupOfflineResponse { found: false, text: None },
    }
}

/// 把请求交给翻译核心；与 HTTP 无关，便于单测
pub fn handle_translate(tr: &Translator, req: TranslateRequest) -> anyhow::Result<TranslateResponse> {
    let scene = Scene::parse(&req.scene).ok_or_else(|| anyhow::anyhow!("未知场景：{}", req.scene))?;
    anyhow::ensure!(!req.items.is_empty(), "items 为空");
    anyhow::ensure!(req.items.len() <= 500, "一次最多 500 段");
    // 单段且非整页：走流式单条接口，能带上下文
    if req.items.len() == 1 && scene != Scene::FullPage {
        let it = &req.items[0];
        let r = tr.translate(scene, &it.text, req.context.as_deref(), &mut |_| {});
        return Ok(TranslateResponse {
            results: vec![match r {
                Ok(t) => ResItem { id: it.id.clone(), text: Some(t), error: None },
                Err(e) => ResItem { id: it.id.clone(), text: None, error: Some(e.to_string()) },
            }],
        });
    }
    let items: Vec<Item> = req
        .items
        .into_iter()
        .map(|i| Item { id: i.id, text: i.text, is_html: i.is_html })
        .collect();
    let rs = tr.translate_batch(scene, items);
    Ok(TranslateResponse {
        results: rs
            .into_iter()
            .map(|r| match r.result {
                Ok(t) => ResItem { id: r.id, text: Some(t), error: None },
                Err(e) => ResItem { id: r.id, text: None, error: Some(e.to_string()) },
            })
            .collect(),
    })
}

/// 插件最近一次来访：设置页靠它显示「装没装、通不通」
#[derive(Debug, Clone, Default)]
pub struct ExtensionSeen {
    /// 最近一次收到扩展请求的时刻
    pub at: Option<std::time::Instant>,
    /// 扩展 id（Origin 里解出来的）
    pub id: Option<String>,
}

pub struct ServerHandle {
    pub port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// 与处理线程共享：最近一次插件来访
    pub seen: Arc<Mutex<ExtensionSeen>>,
}

impl ServerHandle {
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // 每个处理线程都堵在 recv 上，挨个连一次把它们唤醒
        for _ in 0..WORKERS {
            let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        }
    }
}

/// 同时处理几个请求。插件是一段一个请求发的，单线程会让它们排队，
/// 一段慢（或在服务端重试）就把后面全堵住，流式上屏的效果荡然无存。
const WORKERS: usize = 8;

/// 起服务。`translator` 用 Mutex 包着以便设置变更后热替换。
/// `dict` 是离线词库（可能没装/打不开 → 查词返回 found:false），给扩展端"先离线后模型"用
pub fn spawn(
    port: u16,
    allowed_ids: Vec<String>,
    translator: Arc<Mutex<Translator>>,
    dict: SharedDict,
    app_version: &'static str,
) -> anyhow::Result<ServerHandle> {
    let server = tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| anyhow::anyhow!("本地服务无法监听 {port} 端口：{e}"))?;
    let actual = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(port);
    let server = Arc::new(server);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen: Arc<Mutex<ExtensionSeen>> = Arc::new(Mutex::new(ExtensionSeen::default()));
    tracing::info!("本地服务已监听 http://127.0.0.1:{actual}（{WORKERS} 个处理线程）");
    for i in 0..WORKERS {
      let server = server.clone();
      let stop2 = stop.clone();
      let allowed_ids = allowed_ids.clone();
      let translator = translator.clone();
      let dict = dict.clone();
      let seen = seen.clone();
      std::thread::Builder::new().name(format!("local-server-{i}")).spawn(move || {
        loop {
            if stop2.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let mut request = match server.recv() {
                Ok(r) => r,
                Err(_) => break,
            };
            let origin = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Origin"))
                .map(|h| h.value.as_str().to_string());
            let url = request.url().to_string();
            let method = request.method().as_str().to_string();

            // CORS 预检
            if method == "OPTIONS" {
                let _ = request.respond(cors_response(tiny_http::Response::empty(204), origin.as_deref()));
                continue;
            }
            if let OriginCheck::Deny(why) = check_origin(origin.as_deref(), &allowed_ids) {
                tracing::warn!("拒绝来源 {origin:?}：{why}");
                let _ = request.respond(json_response(403, &serde_json::json!({ "error": why }), origin.as_deref()));
                continue;
            }

            // 记一笔「插件来过」：设置页据此显示装没装
            if let Some(o) = origin.as_deref().filter(|o| o.starts_with(EXT_ORIGIN_PREFIX)) {
                let id = o.trim_start_matches(EXT_ORIGIN_PREFIX).trim_end_matches('/').to_string();
                let mut g = seen.lock().unwrap();
                g.at = Some(std::time::Instant::now());
                g.id = Some(id);
            }

            let path = url.split('?').next().unwrap_or("");
            match (method.as_str(), path) {
                ("GET", "/v1/health") => {
                    let cfg_summary = {
                        let t = translator.lock().unwrap();
                        let c = t.config();
                        // 整页那条路由就是插件实际会用到的后端
                        let route = c.route(Scene::FullPage);
                        let model = if route.model.trim().is_empty() {
                            match route.backend {
                                ai_translate::Backend::Claude => c.base.claude_model.clone(),
                                ai_translate::Backend::Codex => c.base.codex_model.clone(),
                                ai_translate::Backend::OpenAi => c.base.openai_model.clone(),
                            }
                        } else {
                            route.model.clone()
                        };
                        serde_json::json!({
                            "ok": true,
                            "app": "简译",
                            "version": app_version,
                            "backend": route.backend.as_str(),
                            "model": model,
                            "concurrency": c.concurrency,
                            "batch_size": c.batch_size,
                        })
                    };
                    let _ = request.respond(json_response(200, &cfg_summary, origin.as_deref()));
                }
                ("POST", "/v1/lookup_offline") => {
                    let mut raw = Vec::new();
                    if std::io::Read::read_to_end(&mut request.as_reader(), &mut raw).is_err() {
                        let _ = request.respond(json_response(400, &serde_json::json!({ "error": "读取请求体失败" }), origin.as_deref()));
                        continue;
                    }
                    let body = String::from_utf8_lossy(&raw).into_owned();
                    let parsed: Result<LookupOfflineRequest, _> = serde_json::from_str(&body);
                    let resp = match parsed {
                        Err(e) => json_response(400, &serde_json::json!({ "error": format!("请求格式错误：{e}") }), origin.as_deref()),
                        Ok(req) => {
                            let r = handle_lookup_offline(&dict, &req.text);
                            json_response(200, &serde_json::to_value(r).unwrap(), origin.as_deref())
                        }
                    };
                    let _ = request.respond(resp);
                }
                ("POST", "/v1/translate") => {
                    // 按字节读再宽松解码：请求体若有不完整的多字节字符，
                    // 不该整个请求 400，交给 JSON 解析报更具体的错
                    let mut raw = Vec::new();
                    if std::io::Read::read_to_end(&mut request.as_reader(), &mut raw).is_err() {
                        let _ = request.respond(json_response(400, &serde_json::json!({ "error": "读取请求体失败" }), origin.as_deref()));
                        continue;
                    }
                    let body = String::from_utf8_lossy(&raw).into_owned();
                    let parsed: Result<TranslateRequest, _> = serde_json::from_str(&body);
                    let resp = match parsed {
                        Err(e) => json_response(400, &serde_json::json!({ "error": format!("请求格式错误：{e}") }), origin.as_deref()),
                        Ok(req) => {
                            let tr = translator.lock().unwrap().clone();
                            match handle_translate(&tr, req) {
                                Ok(r) => json_response(200, &serde_json::to_value(r).unwrap(), origin.as_deref()),
                                Err(e) => json_response(400, &serde_json::json!({ "error": e.to_string() }), origin.as_deref()),
                            }
                        }
                    };
                    let _ = request.respond(resp);
                }
                _ => {
                    let _ = request.respond(json_response(404, &serde_json::json!({ "error": "没有这个接口" }), origin.as_deref()));
                }
            }
        }
        tracing::debug!("本地服务处理线程 {i} 退出");
      })?;
    }
    Ok(ServerHandle { port: actual, stop, seen })
}

fn cors_response<R: std::io::Read>(resp: tiny_http::Response<R>, origin: Option<&str>) -> tiny_http::Response<R> {
    let o = origin.unwrap_or("*");
    resp.with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Origin"[..], o.as_bytes()).unwrap())
        .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Headers"[..], &b"Content-Type"[..]).unwrap())
        .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Allow-Methods"[..], &b"GET, POST, OPTIONS"[..]).unwrap())
        .with_header(tiny_http::Header::from_bytes(&b"Access-Control-Max-Age"[..], &b"600"[..]).unwrap())
}

fn json_response(status: u16, body: &serde_json::Value, origin: Option<&str>) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let s = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
    let resp = tiny_http::Response::from_string(s)
        .with_status_code(status)
        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap());
    cors_response(resp, origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_must_be_extension() {
        assert_eq!(check_origin(Some("chrome-extension://abc"), &[]), OriginCheck::Allow);
        assert_eq!(check_origin(None, &[]), OriginCheck::Allow, "无 Origin 的本机请求放行");
        assert!(matches!(check_origin(Some("https://evil.com"), &[]), OriginCheck::Deny(_)));
        assert!(matches!(check_origin(Some("http://127.0.0.1:8080"), &[]), OriginCheck::Deny(_)));
    }

    #[test]
    fn allowed_ids_further_restrict() {
        let ids = vec!["abcdefg".to_string()];
        assert_eq!(check_origin(Some("chrome-extension://abcdefg"), &ids), OriginCheck::Allow);
        assert_eq!(check_origin(Some("chrome-extension://ABCDEFG/"), &ids), OriginCheck::Allow);
        assert!(matches!(check_origin(Some("chrome-extension://other"), &ids), OriginCheck::Deny(_)));
    }

    #[test]
    fn request_parses_with_defaults() {
        let r: TranslateRequest = serde_json::from_str(r#"{"scene":"full_page","items":[{"id":"a","text":"x"}]}"#).unwrap();
        assert_eq!(r.scene, "full_page");
        assert!(!r.items[0].is_html);
        assert!(r.context.is_none());
    }

    #[test]
    fn response_omits_null_fields() {
        let r = TranslateResponse { results: vec![ResItem { id: "a".into(), text: Some("译".into()), error: None }] };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"results":[{"id":"a","text":"译"}]}"#);
    }

    #[test]
    fn lookup_offline_response_skips_text_when_not_found() {
        // 没装词库 / 查无此词 → found=false, text 不出现
        let r = LookupOfflineResponse { found: false, text: None };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"found":false}"#);
    }

    #[test]
    fn lookup_offline_response_includes_text_when_found() {
        let r = LookupOfflineResponse { found: true, text: Some("#词 glance".into()) };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""found":true"#), "{s}");
        assert!(s.contains(r#""text":"#), "{s}");
    }

    /// 查无此词时 found=false。本机的真实词库在 dev path 上，避开它直接用查不到的字符串。
    /// 跑这条时假定整台机器的词库里都没这个串；机器自带词库的话这条仍是绿的——这串明显是合成字符串
    #[test]
    fn lookup_offline_not_in_dict() {
        let dict = open_shared_dict();
        let r = handle_lookup_offline(&dict, "xyzqwertynonsensewordzzz");
        assert!(!r.found, "虚构字符串不该查到：{r:?}");
        assert!(r.text.is_none());
    }
}
