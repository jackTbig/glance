//! 拉 OpenAI 兼容接口的模型列表，给设置页「模型」下拉用（Jack 2026-09-25 要求）。
//!
//! 网络往返几百毫秒到几秒，放后台线程；界面每帧 `drain` 一次收结果。
//! 列表按「接口地址|Key」记归属：改了地址或 Key，旧列表自动作废，下拉时重新拉。

use std::sync::mpsc::{Receiver, Sender};

pub type FetchResult = (String, anyhow::Result<Vec<String>>);

#[derive(Default)]
pub struct ModelList {
    rx: Option<Receiver<FetchResult>>,
    /// 当前列表（或在途请求）对应的「base|key」
    pub key: Option<String>,
    pub items: Vec<String>,
    pub error: Option<String>,
    pub fetching: bool,
}

pub fn list_key(base: &str, api_key: &str) -> String {
    format!("{}|{}", base.trim(), api_key.trim())
}

/// 接口地址 → 模型列表端点。兼容填到 `/v1`、`/chat/completions` 或裸域名的写法
pub fn models_url(base: &str) -> String {
    let b = base.trim().trim_end_matches('/');
    let b = b.strip_suffix("/chat/completions").unwrap_or(b);
    format!("{b}/models")
}

/// 解析 `/models` 的回包。只认 OpenAI 兼容的 `{"data":[{"id":…}]}` 形状
pub fn parse_ids(text: &str) -> anyhow::Result<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow::anyhow!("回包里没有 data 数组，可能不是 OpenAI 兼容接口"))?;
    let mut ids: Vec<String> =
        arr.iter().filter_map(|it| it.get("id").and_then(|i| i.as_str()).map(str::to_string)).collect();
    ids.sort_unstable();
    ids.dedup();
    anyhow::ensure!(!ids.is_empty(), "接口返回的模型列表是空的");
    Ok(ids)
}

fn fetch_blocking(base: &str, api_key: &str) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(!base.trim().is_empty(), "还没填接口地址");
    anyhow::ensure!(!api_key.trim().is_empty(), "还没填 API Key");
    let resp = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()?
        .get(models_url(base))
        .bearer_auth(api_key.trim())
        .send()?;
    let status = resp.status();
    let text = resp.text()?;
    anyhow::ensure!(status.is_success(), "接口返回 {status}：{}", text.chars().take(120).collect::<String>());
    parse_ids(&text)
}

impl ModelList {
    /// 发起一次拉取；同一个 key 已在途就忽略。返回是否真的发起了
    pub fn request(&mut self, base: &str, api_key: &str) -> bool {
        let k = list_key(base, api_key);
        if self.fetching && self.key.as_deref() == Some(k.as_str()) {
            return false;
        }
        let (tx, rx): (Sender<FetchResult>, Receiver<FetchResult>) = std::sync::mpsc::channel();
        let (b, key, k2) = (base.to_string(), api_key.to_string(), k.clone());
        if std::thread::Builder::new()
            .name("model-list".into())
            .spawn(move || {
                let _ = tx.send((k2, fetch_blocking(&b, &key)));
            })
            .is_err()
        {
            self.error = Some("起不了后台线程".into());
            return false;
        }
        self.rx = Some(rx);
        self.key = Some(k);
        self.fetching = true;
        self.error = None;
        true
    }

    /// 收在途结果。过期请求（key 已换）直接丢
    pub fn drain(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok((k, res)) = rx.try_recv() {
            self.fetching = false;
            if self.key.as_deref() != Some(k.as_str()) {
                continue;
            }
            match res {
                Ok(items) => {
                    self.items = items;
                    self.error = None;
                }
                Err(e) => {
                    self.items.clear();
                    self.error = Some(format!("{e}"));
                }
            }
        }
    }

    /// 手里的列表是不是当前地址+Key 的
    pub fn fresh_for(&self, base: &str, api_key: &str) -> bool {
        !self.fetching && !self.items.is_empty() && self.key.as_deref() == Some(list_key(base, api_key).as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_handles_the_common_base_shapes() {
        assert_eq!(models_url("https://api.deepseek.com"), "https://api.deepseek.com/models");
        assert_eq!(models_url("https://x/v1/"), "https://x/v1/models");
        assert_eq!(models_url("https://x/v1/chat/completions"), "https://x/v1/models");
    }

    #[test]
    fn parses_openai_models_payload() {
        let ids = parse_ids(r#"{"object":"list","data":[{"id":"b"},{"id":"a"},{"id":"a"}]}"#).unwrap();
        assert_eq!(ids, vec!["a", "b"]);
        assert!(parse_ids(r#"{"ok":true}"#).is_err());
        assert!(parse_ids(r#"{"data":[]}"#).is_err(), "空列表要报错，别给用户一个空下拉");
        assert!(parse_ids("not json").is_err());
    }

    #[test]
    fn stale_responses_are_dropped() {
        let mut m = ModelList::default();
        let (tx, rx) = std::sync::mpsc::channel();
        m.rx = Some(rx);
        m.key = Some("new|k".into());
        m.fetching = true;
        tx.send(("old|k".into(), Ok(vec!["x".into()]))).unwrap();
        m.drain();
        assert!(m.items.is_empty(), "旧地址的列表不能冒充新地址的");
        assert!(!m.fetching);
    }
}
