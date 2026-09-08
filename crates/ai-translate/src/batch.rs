//! 多段打包 / 回包按编号拆：每段一行 "⟨n⟩ 文本"

/// 打包：n 从 1 起；段内换行替换为空格
pub fn pack(items: &[&str]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, t)| format!("⟨{}⟩ {}", i + 1, t.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 从一行开头解析 "⟨n⟩"，返回 (n, 余下文本)
fn parse_line(line: &str) -> Option<(usize, &str)> {
    let s = line.trim_start();
    let s = s.strip_prefix('⟨')?;
    let close = s.find('⟩')?;
    let n: usize = s[..close].trim().parse().ok()?;
    Some((n, s[close + '⟩'.len_utf8()..].trim()))
}

/// 拆包：返回长度 = n 的 Vec，缺的为 None；同一编号出现多次取第一次；无编号的行忽略
pub fn unpack(reply: &str, n: usize) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = vec![None; n];
    for line in reply.lines() {
        // 模型偶尔会把 "zh=" 之类前缀放在编号前，这里只认行内第一个 ⟨
        let line = match line.find('⟨') {
            Some(i) => &line[i..],
            None => continue,
        };
        if let Some((k, text)) = parse_line(line) {
            if k >= 1 && k <= n && out[k - 1].is_none() {
                out[k - 1] = Some(text.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_numbers_lines() {
        assert_eq!(pack(&["a", "b\nc"]), "⟨1⟩ a\n⟨2⟩ b c");
    }

    #[test]
    fn unpack_by_number_tolerates_reorder() {
        let r = unpack("⟨2⟩ 乙\n⟨1⟩ 甲", 2);
        assert_eq!(r, vec![Some("甲".into()), Some("乙".into())]);
    }

    #[test]
    fn unpack_missing_is_none() {
        let r = unpack("⟨1⟩ 甲\n⟨3⟩ 丙", 3);
        assert_eq!(r, vec![Some("甲".into()), None, Some("丙".into())]);
    }

    #[test]
    fn unpack_ignores_extra_lines_and_duplicates() {
        let r = unpack("好的，译文如下：\n⟨1⟩ 甲\n⟨1⟩ 又一遍\n⟨9⟩ 越界", 1);
        assert_eq!(r, vec![Some("甲".into())]);
    }

    #[test]
    fn unpack_accepts_prefix_before_number() {
        let r = unpack("zh=⟨1⟩ 甲", 1);
        assert_eq!(r, vec![Some("甲".into())]);
    }
}
