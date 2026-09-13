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
/// **插件端有一份同规则的 TS 实现**（`extension/src/dict.ts` 的 `looksLikeWord`），
/// 两边必须一致，否则同一个词在桌面端给词典卡、在网页里只给一句译文。
/// 插件测试会从下面 `word_lookup_only_for_short_latin_terms` 的用例清单逐条对跑，
/// 改规则时把新用例加进那两个 `for s in [...]` 数组，插件侧就自动被盯住。
///
/// 判据只认拉丁字母的词：目标语言本来就是中文，中文段会被跳过；
/// 日韩文的汉字与中文难以区分，一并排除，免得把中文段误判成查词。
/// 按方向判「够不够格当查词」。
///
/// 中译英时要认**短的中文词**，否则「渲染」「并发」这种词会被当成句子翻、拿不到释义；
/// 英译中仍然只认拉丁词（理由见 [`looks_like_word`]）。
pub fn looks_like_word_in(s: &str, dir: crate::direction::Direction) -> bool {
    match dir {
        crate::direction::Direction::ToChinese => looks_like_word(s),
        crate::direction::Direction::ToEnglish => looks_like_chinese_word(s),
    }
}

/// 短中文词：去掉首尾标点后 1..=8 个字、全是汉字、不跨行。
/// 长度上限比英文那边小：中文一个字信息量大，超过八个字基本就是短句了。
pub fn looks_like_chinese_word(s: &str) -> bool {
    let t = s.trim().trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace() || "「」『』（）《》，。；：！？、“”‘’".contains(c));
    let n = t.chars().count();
    (1..=8).contains(&n)
        && !t.contains('\n')
        && t.chars().all(|c| ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c))
}

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

    /// 中译英时要认中文词，否则中文词拿不到词典卡；英译中时仍然只认拉丁词
    #[test]
    fn word_lookup_follows_direction() {
        use crate::direction::Direction;
        for s in ["渲染", "并发", "内存泄漏"] {
            assert!(looks_like_word_in(s, Direction::ToEnglish), "中译英应当认中文词：{s}");
            assert!(!looks_like_word_in(s, Direction::ToChinese), "英译中不认中文词：{s}");
        }
        for s in ["glance", "open source"] {
            assert!(looks_like_word_in(s, Direction::ToChinese), "英译中认拉丁词：{s}");
            assert!(!looks_like_word_in(s, Direction::ToEnglish), "中译英不把英文词当查词：{s}");
        }
        // 长中文是句子，不是词
        assert!(!looks_like_word_in("这是一句很长的中文句子用来测试", Direction::ToEnglish));
    }

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
