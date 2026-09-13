//! 服务商预设：选一个就把「接口地址 + 模型」填好，省得用户去翻各家文档。
//!
//! 只收 **OpenAI 兼容** 的服务：本项目的整页翻译只走这条路（见 CLAUDE.md 架构红线），
//! claude / codex 是另外两条常驻进程的路，不在这里选。
//!
//! **预设只负责填字段，不改变任何行为**：填完之后用户照样可以手改，
//! 地址对不上任何预设时显示「自定义」。

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
}

/// 认得的服务商。顺序即下拉里的顺序，把常用的放前面。
pub const PRESETS: &[Preset] = &[
    Preset { name: "DeepSeek", base_url: "https://api.deepseek.com", model: "deepseek-chat" },
    Preset {
        name: "通义千问",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        model: "qwen-plus",
    },
    Preset { name: "智谱 GLM", base_url: "https://open.bigmodel.cn/api/paas/v4", model: "glm-4-flash" },
    Preset { name: "月之暗面 Kimi", base_url: "https://api.moonshot.cn/v1", model: "moonshot-v1-8k" },
    Preset { name: "OpenAI", base_url: "https://api.openai.com/v1", model: "gpt-4o-mini" },
];

/// 当前地址属于哪个预设。**只比地址不比模型**：同一家的模型用户经常自己换，
/// 换了模型还认得出是哪一家，下拉里才不会莫名其妙跳到「自定义」。
pub fn detect(base_url: &str) -> Option<&'static Preset> {
    let u = normalize(base_url);
    PRESETS.iter().find(|p| normalize(p.base_url) == u)
}

/// 下拉里显示的名字
pub fn label_of(base_url: &str) -> &'static str {
    detect(base_url).map(|p| p.name).unwrap_or("自定义")
}

/// 比地址时忽略大小写、结尾的斜杠与首尾空白——用户从文档里粘过来常带一个 `/`
fn normalize(u: &str) -> String {
    u.trim().trim_end_matches('/').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_filled_in() {
        for p in PRESETS {
            assert!(p.base_url.starts_with("https://"), "{} 的地址要是 https", p.name);
            assert!(!p.model.is_empty(), "{} 少了默认模型", p.name);
        }
    }

    #[test]
    fn detect_ignores_trailing_slash_and_case() {
        assert_eq!(detect("https://api.deepseek.com").map(|p| p.name), Some("DeepSeek"));
        assert_eq!(detect("https://API.DeepSeek.com/").map(|p| p.name), Some("DeepSeek"));
        assert_eq!(detect(" https://api.deepseek.com ").map(|p| p.name), Some("DeepSeek"));
    }

    #[test]
    fn unknown_url_is_custom() {
        assert!(detect("https://llm.example.com/v1").is_none());
        assert_eq!(label_of("https://llm.example.com/v1"), "自定义");
        assert_eq!(label_of(""), "自定义", "还没填地址时也显示自定义，不要空着");
    }

    /// 换了模型仍然认得是哪一家，否则下拉会莫名跳到「自定义」
    #[test]
    fn detection_only_depends_on_the_url() {
        let p = detect("https://api.deepseek.com").unwrap();
        assert_eq!(p.name, "DeepSeek");
        assert_eq!(label_of(p.base_url), "DeepSeek");
    }
}
