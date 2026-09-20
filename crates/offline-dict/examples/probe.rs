//! 离线词典探针：拿真实词库查几个词，看内容对不对、快不快。
//!
//! 用法：`cargo run --release -p offline-dict --example probe -- <词库.bin> glance running went ...`
//! 不给词就查一组默认样例（覆盖原形、规则变形、不规则变形、短语、专有名词、查不到）。

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| r"D:\DevCache\tt-dict\ecdict.bin".into());
    let words: Vec<String> = args.collect();
    let words: Vec<String> = if words.is_empty() {
        ["glance", "running", "studies", "went", "better", "open source", "Kubernetes", "zzzznotaword"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        words
    };

    let t0 = std::time::Instant::now();
    let mut d = offline_dict::Dict::open(std::path::Path::new(&path))?;
    println!("打开 {path}：{} 条，用时 {} ms", d.len(), t0.elapsed().as_millis());

    for w in &words {
        let t = std::time::Instant::now();
        match d.lookup(w) {
            Some((e, restored)) => {
                let head = if restored { format!("{w} → {}", e.word) } else { e.word.clone() };
                let first = e.translation.lines().next().unwrap_or("");
                println!(
                    "{:>14}  {:<14} {}  [{}]  {} µs",
                    head,
                    e.phonetic,
                    first,
                    e.forms_zh(),
                    t.elapsed().as_micros()
                );
            }
            None => println!("{w:>14}  查不到  {} µs", t.elapsed().as_micros()),
        }
    }
    Ok(())
}
