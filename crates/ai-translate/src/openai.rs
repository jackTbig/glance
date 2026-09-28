//! OpenAI 兼容接口（/chat/completions，SSE 流式，非 SSE 回包也兼容）
use crate::backend::BackendConfig;
use std::io::{BufRead, BufReader};
use std::time::Duration;

/// HTTP 非 2xx 的结构化错误，供重试逻辑判断
#[derive(Debug)]
pub struct HttpStatusError {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub body: String,
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.body.chars().take(200).collect::<String>())
    }
}
impl std::error::Error for HttpStatusError {}

#[derive(Clone)]
pub struct OpenAi {
    client: reqwest::blocking::Client,
    url: String,
    key: String,
    model: String,
    enable_thinking: Option<bool>,
}

/// 请求体。enable_thinking 只在配置了才带上（未验证的服务对未知参数的容忍度不明）
fn chat_body(model: &str, system_prompt: &str, user_message: &str, enable_thinking: Option<bool>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "stream": true,
        "temperature": 0.2,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": user_message }
        ]
    });
    if let Some(t) = enable_thinking {
        body["enable_thinking"] = serde_json::Value::Bool(t);
    }
    body
}

impl OpenAi {
    pub fn new(cfg: &BackendConfig) -> anyhow::Result<Self> {
        let base = cfg.openai_base_url.trim().trim_end_matches('/');
        anyhow::ensure!(!base.is_empty(), "openai_base_url 为空");
        let client = reqwest::blocking::Client::builder().timeout(cfg.timeout).build()?;
        Ok(Self {
            client,
            url: format!("{base}/chat/completions"),
            key: cfg.openai_api_key.trim().to_string(),
            model: cfg.openai_model.trim().to_string(),
            enable_thinking: cfg.enable_thinking,
        })
    }

    /// 换模型（场景路由指定了模型时用）
    pub fn with_model(&self, model: &str) -> Self {
        let mut c = self.clone();
        if !model.trim().is_empty() {
            c.model = model.trim().to_string();
        }
        c
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn chat(&self, system_prompt: &str, user_message: &str, timeout: Duration, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        let body = chat_body(&self.model, system_prompt, user_message, self.enable_thinking);
        let mut req = self.client.post(&self.url).timeout(timeout).json(&body);
        if !self.key.is_empty() {
            req = req.bearer_auth(&self.key);
        }
        let resp = req.send()?;
        let status = resp.status();
        if !status.is_success() {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = resp.text().unwrap_or_default();
            return Err(anyhow::Error::new(HttpStatusError { status: status.as_u16(), retry_after, body }));
        }
        let mut acc = String::new();
        let mut saw_sse = false;
        let mut raw = String::new();
        for line in BufReader::new(resp).lines() {
            let line = line?;
            if let Some(data) = line.strip_prefix("data:") {
                saw_sse = true;
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }
                let v: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(c) = v["choices"][0]["delta"]["content"].as_str() {
                    if !c.is_empty() {
                        acc.push_str(c);
                        on_delta(&acc);
                    }
                }
            } else if !saw_sse {
                raw.push_str(&line);
            }
        }
        if !saw_sse {
            let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("响应不是 SSE 也不是 JSON：{e}"))?;
            if let Some(c) = v["choices"][0]["message"]["content"].as_str() {
                acc = c.to_string();
            } else if let Some(err) = v["error"]["message"].as_str() {
                anyhow::bail!("接口错误：{err}");
            }
        }
        Ok(acc.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::chat_body;

    /// 没配置时请求体里不许出现 enable_thinking：DeepSeek 官方 API 等未验证服务
    /// 对未知参数的容忍度不明，默认一律不带，保持既有行为
    #[test]
    fn body_omits_enable_thinking_by_default() {
        let b = chat_body("m", "sys", "user", None);
        assert!(b.get("enable_thinking").is_none());
        assert_eq!(b["model"], "m");
        assert_eq!(b["stream"], true);
        assert_eq!(b["messages"][0]["content"], "sys");
        assert_eq!(b["messages"][1]["content"], "user");
    }

    /// 配置了就原样带上：百炼网关 deepseek-v4-flash 默认开思考，翻译任务 97% 的
    /// 生成量耗在思考上，关掉实测 21.2s → 1.76s（2026-09-28）
    #[test]
    fn body_carries_enable_thinking_when_set() {
        assert_eq!(chat_body("m", "s", "u", Some(false))["enable_thinking"], false);
        assert_eq!(chat_body("m", "s", "u", Some(true))["enable_thinking"], true);
    }
}
