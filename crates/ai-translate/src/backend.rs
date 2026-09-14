//! 后端类型与后端实例参数（与场景无关）
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Claude,
    Codex,
    OpenAi,
}

impl Backend {
    /// 宽松解析：codex → Codex；openai / deepseek / qwen / api → OpenAi；其余一律 Claude
    pub fn parse(s: &str) -> Backend {
        match s.trim().to_ascii_lowercase().as_str() {
            "codex" => Backend::Codex,
            "openai" | "deepseek" | "qwen" | "api" => Backend::OpenAi,
            _ => Backend::Claude,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Claude => "claude",
            Backend::Codex => "codex",
            Backend::OpenAi => "openai",
        }
    }
}

/// 一个后端实例需要的全部参数
#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub backend: Backend,
    /// claude 可执行文件；写 "claude" 即按 PATH 与常见安装位置自动找
    pub claude_bin: String,
    pub claude_model: String,
    pub codex_bin: String,
    /// 留空用 codex 默认模型
    pub codex_model: String,
    /// base_url 后拼 /chat/completions；DeepSeek 填 https://api.deepseek.com
    pub openai_base_url: String,
    pub openai_api_key: String,
    pub openai_model: String,
    pub timeout: Duration,
    /// 测试用：整体替换 claude 命令（如 ["python", "tests/fake_claude.py"]）
    pub command_override: Vec<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            backend: Backend::Claude,
            claude_bin: "claude".into(),
            claude_model: "sonnet".into(),
            codex_bin: "codex".into(),
            codex_model: String::new(),
            openai_base_url: "https://api.deepseek.com".into(),
            openai_api_key: String::new(),
            openai_model: "deepseek-flash".into(),
            timeout: Duration::from_secs(15),
            command_override: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_parse_is_lenient() {
        assert_eq!(Backend::parse("Codex"), Backend::Codex);
        assert_eq!(Backend::parse("deepseek"), Backend::OpenAi);
        assert_eq!(Backend::parse("whatever"), Backend::Claude);
    }

    #[test]
    fn backend_serde_lowercase() {
        assert_eq!(serde_json::to_string(&Backend::OpenAi).unwrap(), "\"openai\"");
        assert_eq!(serde_json::from_str::<Backend>("\"codex\"").unwrap(), Backend::Codex);
    }
}
