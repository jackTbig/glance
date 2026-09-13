//! 翻译方向。默认英译中；Jack 2026-09-14 要求加中译英开关。
//!
//! **方向只影响三处**，加新方向时这三处都要跟着改：
//! 1. 提示词里「翻译成什么语言」与词典条目的释义语言（`prompts`）；
//! 2. 「这段已经是目标语言了」的判据（[`Direction::already_target`]）——
//!    整页批量会据此本地跳过，悬停取词也据此决定要不要弹卡片；
//! 3. 什么样的文本算「一个词」（`scene::looks_like_word_in`）：英译中只认拉丁词，
//!    中译英要认短的中文词，否则中文词会被当成句子翻。

/// 翻译方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// 英（或任意外语）→ 简体中文
    #[default]
    ToChinese,
    /// 中文 → 英文
    ToEnglish,
}

impl Direction {
    /// 配置文件里存的字符串。**认不出来一律回到默认方向**，别因为配置写错就不翻译了
    pub fn parse(s: &str) -> Direction {
        match s.trim().to_ascii_lowercase().as_str() {
            "zh2en" | "zh-en" | "toenglish" | "en" => Direction::ToEnglish,
            _ => Direction::ToChinese,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Direction::ToChinese => "en2zh",
            Direction::ToEnglish => "zh2en",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Direction::ToChinese => "英译中",
            Direction::ToEnglish => "中译英",
        }
    }

    /// 目标语言的中文名，直接进提示词
    pub fn target_zh(self) -> &'static str {
        match self {
            Direction::ToChinese => "简体中文",
            Direction::ToEnglish => "英文",
        }
    }

    pub fn flip(self) -> Direction {
        match self {
            Direction::ToChinese => Direction::ToEnglish,
            Direction::ToEnglish => Direction::ToChinese,
        }
    }

    /// 这段文字是不是已经是目标语言了（是的话没必要翻）。
    ///
    /// 判据只看「汉字多还是拉丁字母多」：混排时按多数派算，与整页批量一直用的口径一致。
    pub fn already_target(self, text: &str) -> bool {
        match self {
            Direction::ToChinese => mostly_han(text),
            Direction::ToEnglish => mostly_latin(text),
        }
    }
}

fn counts(text: &str) -> (usize, usize, usize) {
    let (mut han, mut latin, mut total) = (0usize, 0usize, 0usize);
    for c in text.chars() {
        if !c.is_alphanumeric() {
            continue;
        }
        total += 1;
        if ('\u{4e00}'..='\u{9fff}').contains(&c) || ('\u{3400}'..='\u{4dbf}').contains(&c) {
            han += 1;
        } else if c.is_ascii_alphanumeric() {
            latin += 1;
        }
    }
    (han, latin, total)
}

/// 汉字占字母数字汉字总数一半以上
pub fn mostly_han(text: &str) -> bool {
    let (han, _, total) = counts(text);
    total > 0 && han * 2 > total
}

/// 拉丁字母数字占一半以上，且一个汉字都没有。
///
/// 「一个汉字都没有」这条是刻意的：中译英时，一句里夹着几个汉字就说明还没翻，
/// 光比多数派会把「API 接口」这种判成已经是英文。
pub fn mostly_latin(text: &str) -> bool {
    let (han, latin, total) = counts(text);
    total > 0 && han == 0 && latin * 2 > total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_falls_back_to_default() {
        assert_eq!(Direction::parse("zh2en"), Direction::ToEnglish);
        assert_eq!(Direction::parse("EN2ZH"), Direction::ToChinese);
        assert_eq!(Direction::parse("乱写"), Direction::ToChinese, "配置写错也要能翻译");
        assert_eq!(Direction::parse(Direction::ToEnglish.as_str()), Direction::ToEnglish, "存进去再读出来要一致");
    }

    #[test]
    fn already_target_follows_direction() {
        let zh = "这是一段中文，含 API 两个字母";
        let en = "Mostly English with numbers 123";
        assert!(Direction::ToChinese.already_target(zh), "英译中时中文段不用翻");
        assert!(!Direction::ToChinese.already_target(en));
        assert!(Direction::ToEnglish.already_target(en), "中译英时英文段不用翻");
        assert!(!Direction::ToEnglish.already_target(zh));
        // 夹着汉字就说明还没翻完，不能算已经是英文
        assert!(!Direction::ToEnglish.already_target("API 接口"));
        // 空白与纯符号两边都不算目标语言，交给上层按「没内容」处理
        for s in ["", "   ", "…… —— ！"] {
            assert!(!Direction::ToChinese.already_target(s));
            assert!(!Direction::ToEnglish.already_target(s));
        }
    }

    #[test]
    fn flip_round_trips() {
        assert_eq!(Direction::ToChinese.flip().flip(), Direction::ToChinese);
        assert_eq!(Direction::ToChinese.flip(), Direction::ToEnglish);
    }
}
