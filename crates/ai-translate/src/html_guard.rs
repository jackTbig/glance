//! 行内 HTML 标签占位符保护：翻译前把 `<a href>` 之类换成 ⟦n⟧ / ⟦/n⟧ / ⟦n/⟧，译完还原。
//! 用特殊括号是因为模型几乎不会改动它们，也不会与正文冲突。

#[derive(Debug, Clone, PartialEq)]
pub struct Protected {
    pub text: String,
    /// tags[i] = 第 i+1 号占位符对应的原始标签串（开标签或自闭合标签）
    pub tags: Vec<String>,
}

const VOID_TAGS: &[&str] = &["br", "img", "wbr", "hr", "input", "source", "embed", "area", "col", "base", "link", "meta", "param", "track"];

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('<')
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// 把标签替换为占位符
pub fn protect(html: &str) -> Protected {
    let mut text = String::with_capacity(html.len());
    let mut tags: Vec<String> = Vec::new();
    // 未闭合的开标签栈：(编号, 标签名)
    let mut open: Vec<(usize, String)> = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let Some(end_rel) = rest[start..].find('>') else { break };
        let end = start + end_rel + 1;
        let tag = &rest[start..end];
        text.push_str(&rest[..start]);
        let name = tag_name(tag);
        if name.is_empty() {
            // 不是标签（如 "a < b"），原样保留
            text.push_str(tag);
        } else if tag.starts_with("</") {
            if let Some(pos) = open.iter().rposition(|(_, n)| *n == name) {
                let (num, _) = open.remove(pos);
                text.push_str(&format!("⟦/{num}⟧"));
            } else {
                // 配不上的闭标签按自闭合记
                tags.push(tag.to_string());
                text.push_str(&format!("⟦{}/⟧", tags.len()));
            }
        } else if tag.ends_with("/>") || VOID_TAGS.contains(&name.as_str()) {
            tags.push(tag.to_string());
            text.push_str(&format!("⟦{}/⟧", tags.len()));
        } else {
            tags.push(tag.to_string());
            let num = tags.len();
            open.push((num, name));
            text.push_str(&format!("⟦{num}⟧"));
        }
        rest = &rest[end..];
    }
    text.push_str(rest);
    Protected { text, tags }
}

fn closing_for(open_tag: &str) -> String {
    format!("</{}>", tag_name(open_tag))
}

/// 用占位符对应的原始标签还原；任一占位符缺失或多出 → Err
pub fn restore(translated: &str, tags: &[String]) -> anyhow::Result<String> {
    let mut out = String::with_capacity(translated.len() + tags.len() * 8);
    let mut seen = vec![0u32; tags.len()];
    let mut rest = translated;
    while let Some(start) = rest.find('⟦') {
        let Some(end_rel) = rest[start..].find('⟧') else {
            anyhow::bail!("占位符未闭合");
        };
        let end = start + end_rel + '⟧'.len_utf8();
        let inner = &rest[start + '⟦'.len_utf8()..end - '⟧'.len_utf8()];
        out.push_str(&rest[..start]);
        let (kind, num_str) = if let Some(n) = inner.strip_prefix('/') {
            ("close", n)
        } else if let Some(n) = inner.strip_suffix('/') {
            ("void", n)
        } else {
            ("open", inner)
        };
        let num: usize = num_str.trim().parse().map_err(|_| anyhow::anyhow!("占位符编号非法：{inner}"))?;
        let Some(tag) = tags.get(num.wrapping_sub(1)) else {
            anyhow::bail!("占位符编号越界：{num}");
        };
        match kind {
            "open" => {
                seen[num - 1] += 1;
                out.push_str(tag);
            }
            "close" => {
                seen[num - 1] += 1;
                out.push_str(&closing_for(tag));
            }
            _ => {
                seen[num - 1] += 2;
                out.push_str(tag);
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    // 开标签期望出现 open+close 各一次（计 2），自闭合一次（计 2）
    for (i, c) in seen.iter().enumerate() {
        if *c != 2 {
            anyhow::bail!("占位符 {} 出现次数异常（{}）", i + 1, c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_replaces_inline_tags() {
        let p = protect(r#"Hello <a href="x">world</a> and <b>bold</b><br>"#);
        assert_eq!(p.text, "Hello ⟦1⟧world⟦/1⟧ and ⟦2⟧bold⟦/2⟧⟦3/⟧");
        assert_eq!(p.tags, vec![r#"<a href="x">"#, "<b>", "<br>"]);
    }

    #[test]
    fn restore_round_trip() {
        let p = protect("A <i>x</i> B");
        assert_eq!(restore("甲 ⟦1⟧x⟦/1⟧ 乙", &p.tags).unwrap(), "甲 <i>x</i> 乙");
    }

    #[test]
    fn restore_missing_placeholder_is_error() {
        let p = protect("A <i>x</i> B");
        assert!(restore("甲 x 乙", &p.tags).is_err());
        assert!(restore("甲 ⟦1⟧x⟦/1⟧⟦/1⟧ 乙", &p.tags).is_err(), "多出也报错");
    }

    #[test]
    fn plain_text_untouched() {
        assert_eq!(protect("no tags").text, "no tags");
        assert_eq!(protect("a < b and c > d").text, "a < b and c > d");
        assert_eq!(restore("纯文本", &[]).unwrap(), "纯文本");
    }

    #[test]
    fn nested_and_self_closing() {
        let p = protect(r#"<b><i>x</i></b><img src="y"/>"#);
        assert_eq!(p.text, "⟦1⟧⟦2⟧x⟦/2⟧⟦/1⟧⟦3/⟧");
        assert_eq!(restore(&p.text, &p.tags).unwrap(), r#"<b><i>x</i></b><img src="y"/>"#);
    }
}
