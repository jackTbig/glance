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

/// 取词释义：要的是词典条目而不是一句译文，所以整段提示词独立，不叠 COMMON
/// （COMMON 那句「只输出译文，不加解释」与这里正相反）。
///
/// 行标记格式而不是 JSON：面板是流式上屏的，行格式每收到一行就能画一行，
/// 模型少给一行也只是少一块内容；JSON 截断则整块作废。
const WORD: &str = "你是英汉词典。用户取到一个词或短语，请给出词典条目。\
严格按下面的行格式输出，一行一条，行首标记不能改；不知道的整行省略，不要写「无」或「暂缺」；\
除这些行外不要输出任何说明、标题或代码块围栏。\n\
#词 词条本身，保持原样拼写\n\
#音 英 /国际音标/ 美 /国际音标/\n\
#释 词性缩写. 该词性下的常见中文释义，多个义项用「；」隔开\n\
#形 词形变化，如：复数 xxx；过去式 xxx；比较级 xxx\n\
#例 一个短例句，用原文\n\
#译 上面那句例句的中文翻译\n\
其中 #释 按常用程度排序，一个词性一行，最多 4 行；\
人名、地名、缩写、专有名词也用 #释 给出（词性写 n. 或 abbr.）。\
短语没有音标与词形变化时，省掉 #音 与 #形 两行。";

/// 系统提示：公共约束 + 场景约束 + 术语对照（若有）
pub fn system_prompt(scene: Scene, glossary_block: Option<&str>) -> String {
    let scene_part = match scene {
        Scene::FullPage => FULL_PAGE,
        Scene::Selection => SELECTION,
        Scene::Hover => HOVER,
        Scene::Screenshot => SCREENSHOT,
        Scene::Word => WORD,
    };
    let mut s = if scene == Scene::Word { scene_part.to_string() } else { format!("{COMMON}\n{scene_part}") };
    if let Some(g) = glossary_block.filter(|g| !g.trim().is_empty()) {
        s.push('\n');
        s.push_str(g);
    }
    s
}

/// 用户消息：可选上下文 + 待译文本（整页给 pack 好的多行文本）
/// 术语对照放用户消息而不是系统提示，是因为 claude 常驻会话的系统提示启动后不能变。
pub fn user_message(scene: Scene, text: &str, context: Option<&str>, glossary_block: Option<&str>) -> String {
    let mut m = String::new();
    if let Some(c) = context.map(str::trim).filter(|c| !c.is_empty()) {
        m.push_str("上下文（仅供参考，不要翻译）：\n");
        m.push_str(c);
        m.push_str("\n\n");
    }
    if let Some(g) = glossary_block.map(str::trim).filter(|g| !g.is_empty()) {
        m.push_str(g);
        m.push_str("\n\n");
    }
    m.push_str(match scene {
        Scene::FullPage => "请翻译以下各段：\n",
        Scene::Word => "请给出这个词的词典条目：\n",
        _ => "请翻译：\n",
    });
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
    fn word_prompt_is_dictionary_not_translation() {
        let s = system_prompt(Scene::Word, None);
        // COMMON 那句「只输出译文，不加解释」会把词典条目掐掉，这条场景必须绕开它
        assert!(!s.contains("只输出译文"));
        for marker in ["#词", "#音", "#释", "#形", "#例", "#译"] {
            assert!(s.contains(marker), "提示词缺少标记 {marker}");
        }
        assert!(user_message(Scene::Word, "glance", None, None).ends_with("词典条目：\nglance"));
    }

    #[test]
    fn user_message_with_context() {
        assert_eq!(user_message(Scene::Selection, "hi", Some("prev"), None), "上下文（仅供参考，不要翻译）：\nprev\n\n请翻译：\nhi");
        assert_eq!(user_message(Scene::FullPage, "⟨1⟩ a", None, None), "请翻译以下各段：\n⟨1⟩ a");
        assert_eq!(user_message(Scene::Hover, "x", None, Some("术语对照：\nA → 甲")), "术语对照：\nA → 甲\n\n请翻译：\nx");
    }
}
