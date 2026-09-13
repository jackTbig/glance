//! 四套场景提示词。目标语言由 `Direction` 决定（默认简体中文），源语言自动识别。
use crate::direction::Direction;
use crate::scene::Scene;

/// 公共约束。**目标语言要写死进句子**：只说「翻译」的话，
/// 中译英时模型经常把中文原样退回来。
fn common(dir: Direction) -> String {
    let t = dir.target_zh();
    format!("你是专业翻译。把用户给的内容翻译成{t}；已经是{t}的内容原样输出。专有名词、产品名、代码、URL、数字与单位保留原文。只输出译文，不加解释、不加引号、不加前缀。")
}

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
/// 取词释义：要的是词典条目而不是一句译文，所以整段提示词独立，不叠 `common`
/// （`common` 那句「只输出译文，不加解释」与这里正相反）。
///
/// 两个方向**各写一份**，不要套同一份模板换个词：实测只把「中文释义」换成「英文释义」，
/// 模型照样按英汉词典的套路给中文释义和英文音标（2026-09-14 实测 渲染 一词）。
fn word(dir: Direction) -> String {
    match dir {
        Direction::ToChinese => "你是英汉词典。用户取到一个英文词或短语，请给出词典条目。严格按下面的行格式输出，一行一条，行首标记不能改；不知道的整行省略，不要写「无」或「暂缺」；除这些行外不要输出任何说明、标题或代码块围栏。\n#词 词条本身，保持原样拼写\n#音 英 /国际音标/ 美 /国际音标/\n#释 词性缩写. 该词性下的常见中文释义，多个义项用「；」隔开\n#形 词形变化，如：复数 xxx；过去式 xxx；比较级 xxx\n#例 一个英文短例句\n#译 上面那句例句的中文翻译\n其中 #释 按常用程度排序，一个词性一行，最多 4 行；人名、地名、缩写、专有名词也用 #释 给出（词性写 n. 或 abbr.）。短语没有音标与词形变化时，省掉 #音 与 #形 两行。".to_string(),
        Direction::ToEnglish => "你是汉英词典。用户取到一个中文词或短语，请给出词典条目。**释义一律用英文给对应词**，不要再写中文解释——用户要的就是这个词的英文说法。严格按下面的行格式输出，一行一条，行首标记不能改；不知道的整行省略，不要写「无」或「暂缺」；除这些行外不要输出任何说明、标题或代码块围栏。\n#词 词条本身，保持原样\n#音 汉语拼音，带声调，如 xuàn rǎn\n#释 词性缩写. 对应的英文词或短语，多个用「; 」隔开；必要时补一句英文释义\n#例 一个中文短例句\n#译 上面那句例句的英文翻译\n其中 #释 按常用程度排序，一个词性一行，最多 4 行；中文没有词形变化，**不要输出 #形 这一行**。人名、地名、机构名也用 #释 给出通行英文译名（词性写 n.）。".to_string(),
    }
}

/// 系统提示：公共约束 + 场景约束 + 术语对照（若有）
pub fn system_prompt(scene: Scene, glossary_block: Option<&str>, dir: Direction) -> String {
    let scene_part = match scene {
        Scene::FullPage => FULL_PAGE,
        Scene::Selection => SELECTION,
        Scene::Hover => HOVER,
        Scene::Screenshot => SCREENSHOT,
        Scene::Word => "", // 取词走 word(dir)，整段提示词独立
    };
    let mut s = if scene == Scene::Word { word(dir) } else { format!("{}\n{scene_part}", common(dir)) };
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
        let s = system_prompt(Scene::FullPage, None, Direction::ToChinese);
        assert!(s.contains("⟨n⟩") && s.contains("⟦1⟧"));
        assert!(!s.contains("术语对照"));
    }

    #[test]
    fn glossary_appended() {
        let s = system_prompt(Scene::Hover, Some("术语对照（必须遵守）：\nA → 甲"), Direction::ToChinese);
        assert!(s.ends_with("A → 甲"));
        assert_eq!(system_prompt(Scene::Hover, Some("  "), Direction::ToChinese), system_prompt(Scene::Hover, None, Direction::ToChinese));
    }

    #[test]
    fn direction_switches_target_language() {
        let zh = system_prompt(Scene::Hover, None, Direction::ToChinese);
        let en = system_prompt(Scene::Hover, None, Direction::ToEnglish);
        assert!(zh.contains("简体中文") && !zh.contains("翻译成英文"));
        assert!(en.contains("英文") && !en.contains("简体中文"), "中译英的提示词里不能还写着简体中文：{en}");
    }

    #[test]
    fn word_prompt_follows_direction() {
        let zh = system_prompt(Scene::Word, None, Direction::ToChinese);
        let en = system_prompt(Scene::Word, None, Direction::ToEnglish);
        assert!(zh.contains("英汉词典") && zh.contains("中文释义"));
        assert!(en.contains("汉英词典") && en.contains("释义一律用英文"));
        // 中文没有词形变化，汉英那份不给 #形
        for marker in ["#词", "#音", "#释", "#形", "#例", "#译"] {
            assert!(zh.contains(marker), "英汉条目缺 {marker}");
        }
        for s in [&en] {
            for marker in ["#词", "#音", "#释", "#例", "#译"] {
                assert!(s.contains(marker), "两个方向都要给全行标记，缺 {marker}");
            }
        }
    }

    #[test]
    fn word_prompt_is_dictionary_not_translation() {
        let s = system_prompt(Scene::Word, None, Direction::ToChinese);
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
