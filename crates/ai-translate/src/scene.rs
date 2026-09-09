//! 场景：整页 / 划词 / 悬停 / 截屏 / 取词释义

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scene {
    FullPage,
    Selection,
    Hover,
    Screenshot,
    /// 取到的是单个词或短语：要的是词典释义，不是一句译文
    Word,
}

impl Scene {
    pub const ALL: [Scene; 5] = [Scene::FullPage, Scene::Selection, Scene::Hover, Scene::Screenshot, Scene::Word];

    pub fn parse(s: &str) -> Option<Scene> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "full_page" | "fullpage" | "page" => Some(Scene::FullPage),
            "selection" | "select" => Some(Scene::Selection),
            "hover" => Some(Scene::Hover),
            "screenshot" | "ocr" => Some(Scene::Screenshot),
            "word" | "lookup" | "dict" => Some(Scene::Word),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scene::FullPage => "full_page",
            Scene::Selection => "selection",
            Scene::Hover => "hover",
            Scene::Screenshot => "screenshot",
            Scene::Word => "word",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Scene::FullPage => "整页翻译",
            Scene::Selection => "划词翻译",
            Scene::Hover => "悬停翻译",
            Scene::Screenshot => "截屏翻译",
            Scene::Word => "取词释义",
        }
    }
}

/// 这段取到的文字够不够格当「查词」：够格就给词典卡片，不够格就当句子翻。
///
/// 判据只认拉丁字母的词：目标语言本来就是中文，中文段会被跳过；
/// 日韩文的汉字与中文难以区分，一并排除，免得把中文段误判成查词。
pub fn looks_like_word(s: &str) -> bool {
    // 首尾的引号、括号、句读都不算词的一部分（划词很容易多带一个）
    let t = s.trim().trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace() || "「」『』（）《》，。；：！？、“”‘’".contains(c));
    if t.is_empty() || t.chars().count() > 32 || t.contains('\n') {
        return false;
    }
    if !t.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if t.chars().any(is_cjk) {
        return false;
    }
    let words = t.split_whitespace().count();
    if words == 0 || words > 3 {
        return false;
    }
    // 多个词里还夹着句读，那是半句话，不是词组；单个词里的点允许（U.S、Ph.D）
    words == 1 || !t.contains(['.', ',', ';', ':', '!', '?'])
}

/// 中日韩文字（含假名与谚文），用来把非拉丁文本挡在查词之外
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}'   // 假名
        | '\u{3400}'..='\u{4DBF}' // 扩展 A
        | '\u{4E00}'..='\u{9FFF}' // 基本汉字
        | '\u{AC00}'..='\u{D7AF}' // 谚文
        | '\u{F900}'..='\u{FAFF}' // 兼容汉字
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_lookup_only_for_short_latin_terms() {
        for s in ["glance", " Glance. ", "machine learning", "don't", "U.S", "「glance」", "give up on"] {
            assert!(looks_like_word(s), "该判成查词：{s:?}");
        }
        for s in [
            "",
            "  ",
            "简译",                                    // 中文不查词典
            "翻译 glance",                             // 混排也不查
            "She glanced at her watch.",               // 句子
            "a b c d",                                 // 词太多
            "supercalifragilisticexpialidociousandthenmore", // 超长
            "42",                                      // 没字母
            "hello\nworld",                            // 跨行
            "well, ok",                                // 半句话
        ] {
            assert!(!looks_like_word(s), "不该判成查词：{s:?}");
        }
    }

    #[test]
    fn scene_parse_round_trip() {
        for s in Scene::ALL {
            assert_eq!(Scene::parse(s.as_str()), Some(s));
        }
        assert_eq!(Scene::parse("Full-Page"), Some(Scene::FullPage));
        assert_eq!(Scene::parse("nope"), None);
        assert_eq!(serde_json::to_string(&Scene::FullPage).unwrap(), "\"full_page\"");
    }
}
