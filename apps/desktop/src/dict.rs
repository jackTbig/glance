//! 把模型返回的行标记词典条目解析成结构，供取词卡片渲染。
//!
//! 格式见 `ai_translate::prompts` 的 WORD 提示词。解析刻意宽容：
//! 面板是流式上屏的，每来一行就重解析一次，所以半截的输出也必须能画；
//! 模型偶尔多写一行说明、加个代码块围栏，都不该让整张卡片退化成一坨原文。

/// 一个词性下的释义
#[derive(Debug, Clone, PartialEq)]
pub struct Sense {
    /// 词性缩写，如 `v.`；模型没给就是空串
    pub pos: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub head: String,
    pub phonetic: String,
    pub senses: Vec<Sense>,
    pub forms: String,
    pub example: String,
    pub example_zh: String,
}

impl Entry {
    /// 卡片上「复制」按钮拷走的纯文本
    pub fn to_plain(&self) -> String {
        let mut s = String::new();
        if !self.head.is_empty() {
            s.push_str(&self.head);
            s.push('\n');
        }
        if !self.phonetic.is_empty() {
            s.push_str(&self.phonetic);
            s.push('\n');
        }
        for sense in &self.senses {
            if !sense.pos.is_empty() {
                s.push_str(&sense.pos);
                s.push(' ');
            }
            s.push_str(&sense.text);
            s.push('\n');
        }
        if !self.forms.is_empty() {
            s.push_str(&self.forms);
            s.push('\n');
        }
        if !self.example.is_empty() {
            s.push_str(&self.example);
            s.push('\n');
        }
        if !self.example_zh.is_empty() {
            s.push_str(&self.example_zh);
            s.push('\n');
        }
        s.trim_end().to_string()
    }
}

/// 常见词性缩写。用白名单而不是「点号前都算词性」：
/// 释义里出现 `e.g.` 之类会把半句话当成词性，卡片上就是一个莫名其妙的标签。
const POS: &[&str] = &[
    "n", "v", "vt", "vi", "adj", "adv", "prep", "conj", "pron", "int", "interj", "num", "art", "aux", "abbr", "det", "pl",
    "modal", "phr", "prefix", "suffix",
];

/// 解析词典条目。一条标记行都没有就返回 None，调用方退回普通译文渲染。
pub fn parse(raw: &str) -> Option<Entry> {
    let mut e = Entry::default();
    let mut hit = false;
    for line in raw.lines() {
        // 模型偶尔会把整块包进 ``` 围栏，或者行首缩进
        let line = line.trim().trim_start_matches("```").trim();
        let Some(rest) = line.strip_prefix('#') else { continue };
        let mut chars = rest.chars();
        let Some(tag) = chars.next() else { continue };
        let body = chars.as_str().trim();
        if body.is_empty() {
            continue;
        }
        match tag {
            '词' => e.head = body.into(),
            '音' => e.phonetic = body.into(),
            '释' => e.senses.push(split_pos(body)),
            '形' => e.forms = body.into(),
            '例' => e.example = body.into(),
            '译' => e.example_zh = body.into(),
            _ => continue,
        }
        hit = true;
    }
    hit.then_some(e)
}

/// 把 `v. 一瞥，看一眼` 拆成词性与释义；开头不是已知词性就整行当释义
fn split_pos(body: &str) -> Sense {
    if let Some((head, tail)) = body.split_once('.') {
        let key = head.trim().to_ascii_lowercase();
        if POS.contains(&key.as_str()) && !tail.trim().is_empty() {
            return Sense { pos: format!("{}.", head.trim()), text: tail.trim().into() };
        }
    }
    Sense { pos: String::new(), text: body.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "#词 glance\n\
#音 英 /ɡlɑːns/ 美 /ɡlæns/\n\
#释 v. 一瞥，看一眼；浏览；掠过\n\
#释 n. 一瞥；闪光\n\
#形 复数 glances；过去式 glanced\n\
#例 She glanced at her watch.\n\
#译 她瞥了一眼手表。";

    #[test]
    fn parses_full_entry() {
        let e = parse(SAMPLE).unwrap();
        assert_eq!(e.head, "glance");
        assert_eq!(e.phonetic, "英 /ɡlɑːns/ 美 /ɡlæns/");
        assert_eq!(e.senses, vec![
            Sense { pos: "v.".into(), text: "一瞥，看一眼；浏览；掠过".into() },
            Sense { pos: "n.".into(), text: "一瞥；闪光".into() },
        ]);
        assert_eq!(e.forms, "复数 glances；过去式 glanced");
        assert_eq!(e.example_zh, "她瞥了一眼手表。");
        assert!(e.to_plain().starts_with("glance\n英 /ɡlɑːns/"));
    }

    #[test]
    fn tolerates_partial_and_noisy_output() {
        // 流式到一半：只有词头与半行音标，也要能画
        let e = parse("#词 glance\n#音 英 /ɡlɑ").unwrap();
        assert_eq!(e.head, "glance");
        assert!(e.senses.is_empty());
        // 围栏、缩进、以及模型多嘴的一行说明
        let e = parse("```\n  #词 ok\n这是多余的说明\n#释 adj. 好的\n```").unwrap();
        assert_eq!(e.head, "ok");
        assert_eq!(e.senses.len(), 1);
    }

    #[test]
    fn plain_translation_is_not_an_entry() {
        assert!(parse("她瞥了一眼手表。").is_none());
        assert!(parse("").is_none());
        // 只有 # 号却不是已知标记，同样不算
        assert!(parse("#备注 什么的").is_none());
    }

    #[test]
    fn unknown_pos_stays_in_the_definition() {
        // 白名单外的前缀不该被当成词性标签切走
        let e = parse("#释 e.g. 举个例子").unwrap();
        assert_eq!(e.senses[0], Sense { pos: String::new(), text: "e.g. 举个例子".into() });
        // 词性后面没内容也不切
        let e = parse("#释 n.").unwrap();
        assert_eq!(e.senses[0].pos, "");
    }
}
