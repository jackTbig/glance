//! 托管模式下的控制端点与心跳。
//!
//! 两条链路方向相反，别搞混：
//! - **父 → 子**：本模块起的 HTTP 服务，父进程用它查状态（托盘菜单的勾选态）与下命令。
//! - **子 → 父**：心跳线程去请求父进程的 `/v1/health`，连不上就自救（见 `managed::Link`）。
//!
//! 只绑 127.0.0.1。这里的命令能开关鼠标穿透、能让进程退出，绝不能对外可达。

use crate::managed::{Link, HEARTBEAT_EVERY};
pub use desktop_shell::caption_ipc::{extract_action, Command, State, PATH_CMD, PATH_STATE};
use crossbeam_channel::Sender;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// UI 线程与控制服务之间的共享桥。
pub struct Bridge {
    /// UI 每帧写进来的最新状态，供 `/state` 读
    state: Mutex<State>,
    /// 收到的命令，UI 每帧取走执行
    tx: Sender<Command>,
}

impl Bridge {
    pub fn new(tx: Sender<Command>) -> Arc<Bridge> {
        Arc::new(Bridge { state: Mutex::new(State::default()), tx })
    }

    pub fn publish(&self, s: State) {
        *self.state.lock().unwrap() = s;
    }

    pub fn snapshot(&self) -> State {
        *self.state.lock().unwrap()
    }
}

/// 起控制服务。失败只打日志不阻断——托管链路断了有心跳兜底，
/// 但字幕本身还得能用，不该因为端口占用就起不来。
pub fn spawn_control(bridge: Arc<Bridge>, port: u16) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    std::thread::Builder::new()
        .name("caption-control".into())
        .spawn(move || {
            let server = match tiny_http::Server::http(("127.0.0.1", port)) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("控制端点起不来（127.0.0.1:{port}）：{e}");
                    return;
                }
            };
            tracing::info!("控制端点在 127.0.0.1:{port}");
            for mut req in server.incoming_requests() {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                let url = req.url().to_string();
                let body = if url.starts_with("/cmd") {
                    let mut s = String::new();
                    let _ = std::io::Read::read_to_string(req.as_reader(), &mut s);
                    s
                } else {
                    String::new()
                };
                let (code, text) = handle(&bridge, &url, &body);
                let resp = tiny_http::Response::from_string(text)
                    .with_status_code(code)
                    .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap());
                let _ = req.respond(resp);
            }
        })
        .expect("spawn caption-control");
    stop
}

/// 路由。抽出来是为了能直接单测，不用真起服务。
fn handle(bridge: &Bridge, url: &str, body: &str) -> (u16, String) {
    match url.split('?').next().unwrap_or("") {
        PATH_STATE => (200, bridge.snapshot().to_json()),
        PATH_CMD => match extract_action(body).and_then(Command::parse) {
            Some(c) => {
                let _ = bridge.tx.send(c);
                (200, format!(r#"{{"ok":true,"action":"{}"}}"#, c.as_str()))
            }
            None => (400, r#"{"ok":false,"error":"未知命令"}"#.into()),
        },
        _ => (404, r#"{"ok":false,"error":"没有这个端点"}"#.into()),
    }
}

/// 起心跳线程：定期敲父进程的 `/v1/health`，连续失败就把 `Link` 标成失联。
pub fn spawn_heartbeat(link: Arc<Link>) {
    if !link.is_managed() {
        return;
    }
    let url = format!("http://127.0.0.1:{}/v1/health", link.args.parent_port);
    std::thread::Builder::new()
        .name("caption-heartbeat".into())
        .spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_millis(1500))
                .build()
                .unwrap_or_default();
            loop {
                let ok = client.get(&url).send().map(|r| r.status().is_success()).unwrap_or(false);
                if link.record(ok) {
                    // 只在刚判定失联时说一次；真正的自救动作在 UI 线程里做
                    tracing::warn!("父进程失联，字幕转为独立运行");
                }
                std::thread::sleep(HEARTBEAT_EVERY);
            }
        })
        .expect("spawn caption-heartbeat");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> (Arc<Bridge>, crossbeam_channel::Receiver<Command>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (Bridge::new(tx), rx)
    }

    #[test]
    fn state_endpoint_reports_what_ui_published() {
        let (b, _rx) = bridge();
        b.publish(State { paused: true, passthrough: true, mini: false });
        let (code, body) = handle(&b, "/state", "");
        assert_eq!(code, 200);
        assert_eq!(State::from_json(&body), Some(State { paused: true, passthrough: true, mini: false }));
    }

    #[test]
    fn cmd_endpoint_forwards_to_ui() {
        let (b, rx) = bridge();
        let (code, _) = handle(&b, "/cmd", r#"{"action":"clickable"}"#);
        assert_eq!(code, 200);
        assert_eq!(rx.try_recv().unwrap(), Command::Clickable);

        let (code, _) = handle(&b, "/cmd", r#"{ "action" : "mini" }"#);
        assert_eq!(code, 200);
        assert_eq!(rx.try_recv().unwrap(), Command::ToggleMini);
    }

    #[test]
    fn bad_input_is_rejected_not_guessed() {
        let (b, rx) = bridge();
        for body in [r#"{"action":"rm -rf"}"#, "{}", "", r#"{"action":123}"#] {
            let (code, _) = handle(&b, "/cmd", body);
            assert_eq!(code, 400, "{body:?} 该被拒绝");
        }
        assert!(rx.try_recv().is_err(), "拒绝的请求不能往 UI 送命令");
        assert_eq!(handle(&b, "/绝无此端点", "").0, 404);
    }
}
