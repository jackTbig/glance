//! 四套场景提示词。目标语言固定简体中文，源语言自动识别。
use crate::scene::Scene;

const COMMON: &str = "你是专业翻译。把用户给的内容翻译成简体中文；已经是中文的内容原样输出。\
专有名词、产品名、代码、URL、数字与单位保留原文。只输出译文，不加解释、不加引号、不加前缀。";

const FULL_PAGE: &str = "输入是网页里的多个段落，每段一行，行首有编号 ⟨n⟩。\
输出时每段同样一行、保留相同编号 ⟨n⟩ 与顺序，不合并、不拆分、不遗漏任何一段。\
文中形如 ⟦1⟧、⟦/1⟧、⟦2/⟧ 的记号是格式占位符：必须原样保留、数量不变、相对顺序不变，只翻译它们之间和周围的文字。";

const SELECTION: &str = "输入是用户在阅读时选中的一小段文字。\
如果只是一个单词或短语，第一行给译文，第二行用一句话给出简短释义（词性与常见含义）；\
如果是句子或段落，只给译文。";

const HOVER: &str = "输入是用户鼠标悬停的一个段落。输出整段译文，不加解释。";

const SCREENSHOT: &str = "输入来自屏幕截图的文字识别结果，可能有错字、断行与多余空格。\
先在心里纠正明显的识别错误并合并被断开的句子，再翻译；只输出译文。";

/// 系统提示：公共约束 + 场景约束 + 术语对照（若有）
pub fn system_prompt(scene: Scene, glossary_block: Option<&str>) -> String {
    let scene_part = match scene {
        Scene::FullPage => FULL_PAGE,
        Scene::Selection => SELECTION,
        Scene::Hover => HOVER,
        Scene::Screenshot => SCREENSHOT,
    };
    let mut s = format!("{COMMON}\n{scene_part}");
    if let Some(g) = glossary_block.filter(|g| !g.trim().is_empty()) {
        s.push('\n');
        s.push_str(g);
    }
    s
}

/// 用户消息：可选上下文 + 待译文本（整页给 pack 好的多行文本）
pub fn user_message(scene: Scene, text: &str, context: Option<&str>) -> String {
    let mut m = String::new();
    if let Some(c) = context.map(str::trim).filter(|c| !c.is_empty()) {
        m.push_str("上下文（仅供参考，不要翻译）：\n");
        m.push_str(c);
        m.push_str("\n\n");
    }
    m.push_str(if scene == Scene::FullPage { "请翻译以下各段：\n" } else { "请翻译：\n" });
    m.push_str(text);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_page_prompt_mentions_placeholders() {
        let s = system_prompt(Scene::FullPage, None);
        assert!(s.contains("⟨n⟩") && s.contains("⟦1⟧"));
        assert!(!s.contains("术语对照"));
    }

    #[test]
    fn glossary_appended() {
        let s = system_prompt(Scene::Hover, Some("术语对照（必须遵守）：\nA → 甲"));
        assert!(s.ends_with("A → 甲"));
        assert_eq!(system_prompt(Scene::Hover, Some("  ")), system_prompt(Scene::Hover, None));
    }

    #[test]
    fn user_message_with_context() {
        assert_eq!(user_message(Scene::Selection, "hi", Some("prev")), "上下文（仅供参考，不要翻译）：\nprev\n\n请翻译：\nhi");
        assert_eq!(user_message(Scene::FullPage, "⟨1⟩ a", None), "请翻译以下各段：\n⟨1⟩ a");
    }
}
