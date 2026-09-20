//! 把 ECDICT 的 CSV 编译成本程序的词库文件。
//!
//! 用法：`cargo run --release -p offline-dict --bin build-dict -- <ecdict.csv> <ecdict.bin>`
//!
//! CSV（65 MB / 77 万行）不进仓库，也不进安装包：安装包里只放编译出来的 `.bin`。
//! 词库来源 <https://github.com/skywind3000/ECDICT>，MIT 授权，**分发时要带上它的许可声明**。
//!
//! 只留四个字段：词、音标、中文释义、词形变化。英文释义（definition）占了原始体积的一大半，
//! 而卡片上只显示中文释义，带着它等于让每个用户多装几十 MB。

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 3, "用法：build-dict <ecdict.csv> <输出.bin>");
    let src = std::path::PathBuf::from(&args[1]);
    let dst = std::path::PathBuf::from(&args[2]);

    let text = std::fs::read_to_string(&src).map_err(|e| anyhow::anyhow!("读不了 {}：{e}", src.display()))?;
    let mut rows = parse_csv(&text);
    anyhow::ensure!(!rows.is_empty(), "{} 里一行都没有", src.display());
    let header = rows.remove(0);
    let col = |name: &str| header.iter().position(|h| h.trim() == name);
    let (cw, cp, ct, cx) = (
        col("word").ok_or_else(|| anyhow::anyhow!("CSV 里找不到 word 列"))?,
        col("phonetic"),
        col("translation").ok_or_else(|| anyhow::anyhow!("CSV 里找不到 translation 列"))?,
        col("exchange"),
    );

    let pick = |r: &Vec<String>, i: Option<usize>| i.and_then(|i| r.get(i)).cloned().unwrap_or_default();
    let entries: Vec<(String, String, String, String)> = rows
        .iter()
        .filter_map(|r| {
            let w = r.get(cw)?.trim().to_string();
            if w.is_empty() {
                return None;
            }
            // ECDICT 的多义项在 CSV 里是两个字符的 \n，不是真换行；不还原的话卡片上会连成一坨
            let t = unescape(r.get(ct).cloned().unwrap_or_default().trim());
            Some((w, pick(r, cp).trim().to_string(), t, pick(r, cx).trim().to_string()))
        })
        .collect();

    println!("读入 {} 行，准备写 {}", entries.len(), dst.display());
    let n = offline_dict::build(&dst, entries)?;
    let size = std::fs::metadata(&dst)?.len();
    println!("完成：{n} 条，{:.1} MB", size as f64 / 1_048_576.0);
    Ok(())
}

/// 把 CSV 里的 `\n` `\r` `\"` 这类两字符转义还原成真字符
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('r') => {}
            Some('t') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// 最小 CSV 解析：够读 ECDICT 就行（双引号包裹、`""` 转义、字段内可含换行与逗号）。
///
/// 没有引第三方 csv crate：这个格式一共十几行就能解析对，而多一个依赖就多一份升级负担。
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            '"' if field.is_empty() => quoted = true,
            ',' if !quoted => row.push(std::mem::take(&mut field)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_handles_quotes_commas_and_newlines() {
        let t = "word,translation\nrun,\"n. 跑，奔跑\nvi. 经营\"\nsay,\"他说 \"\"好\"\"\"\n";
        let rows = parse_csv(t);
        assert_eq!(rows[0], vec!["word", "translation"]);
        assert_eq!(rows[1][0], "run");
        assert!(rows[1][1].contains("奔跑") && rows[1][1].contains("经营"), "{:?}", rows[1][1]);
        assert_eq!(rows[2][1], "他说 \"好\"", "两个双引号是一个转义");
    }

    #[test]
    fn escaped_newlines_become_real_ones() {
        assert_eq!(unescape(r"n. 罩\nv. 覆盖"), "n. 罩\nv. 覆盖");
        assert_eq!(unescape(r"C:\path"), r"C:\path", "认不出的转义原样留着，别把路径吃掉");
    }
}
