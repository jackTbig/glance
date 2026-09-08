//! 与平台无关的文本整理：OCR 断行合并、噪声清理、是否值得翻译

/// 行尾没有句末标点、且下一行以小写字母或标点续写 → 认为是同一句被断开，合并成一行
pub fn merge_ocr_lines(lines: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let joinable = out
            .last()
            .map(|prev: &String| {
                let last = prev.trim_end().chars().last().unwrap_or(' ');
                let ends_sentence = matches!(last, '.' | '!' | '?' | '。' | '！' | '？' | '：' | ':' | ';' | '；');
                let next = line.chars().next().unwrap_or(' ');
                let continues = next.is_lowercase() || next.is_ascii_digit() || matches!(next, ',' | '，' | ')' | '）');
                !ends_sentence && continues
            })
            .unwrap_or(false);
        if joinable {
            let prev = out.last_mut().unwrap();
            if !prev.ends_with('-') {
                prev.push(' ');
            } else {
                prev.pop();
            }
            prev.push_str(line);
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

/// 去掉空白后长度 ≥ 2，且含至少一个字母或汉字
pub fn worth_translating(text: &str) -> bool {
    let t = text.trim();
    t.chars().filter(|c| !c.is_whitespace()).count() >= 2 && t.chars().any(|c| c.is_alphabetic())
}

/// OCR 常见噪声：连续空白压成一个空格、全角连接号还原、行首行尾空白
pub fn clean_ocr_text(text: &str) -> String {
    text.lines()
        .map(|l| {
            let l = l.replace('一', "-");
            let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
            l.trim().to_string()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn merge_wrapped_sentence() {
        let r = merge_ocr_lines(&v(&["It runs blazingly fast and", "prevents segfaults."]));
        assert_eq!(r, "It runs blazingly fast and prevents segfaults.");
    }

    #[test]
    fn keeps_paragraph_break() {
        let r = merge_ocr_lines(&v(&["First sentence.", "Second one starts here."]));
        assert_eq!(r, "First sentence.\nSecond one starts here.");
    }

    #[test]
    fn merge_joins_hyphenated_word() {
        let r = merge_ocr_lines(&v(&["program-", "ming language"]));
        assert_eq!(r, "programming language");
    }

    #[test]
    fn worth_translating_rejects_symbols() {
        assert!(worth_translating("hi"));
        assert!(worth_translating("你好"));
        assert!(!worth_translating("+"));
        assert!(!worth_translating("  "));
        assert!(!worth_translating("12 34"));
    }

    #[test]
    fn clean_collapses_spaces_and_dashes() {
        assert_eq!(clean_ocr_text("  a   b  \n\n 2026 一 09 一 08 "), "a b\n2026 - 09 - 08");
    }
}
