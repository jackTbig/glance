//! 离线英汉词典：断网、或模型慢得像卡死的时候，先给一份本地释义。
//!
//! **为什么自己做一套二进制格式**：词库是 ECDICT 的 77 万词条 CSV（65 MB，MIT 授权）。
//! 带着 CSV 走的话每查一个词都要全表扫描；塞进 SQLite 又要多一个 C 依赖。
//! 这里只需要「按词查一条」，所以格式做得很窄：
//!
//! ```text
//! magic     8B      "TTDICT" + 版本
//! count     4B      词条数
//! per_block 4B      每个压缩块里放几条
//! hashes    count × 4B   词（小写）的 32 位哈希，升序；第 i 项对应第 i 条
//! blocks    (n+1) × 8B   每块在文件里的起止位置
//! data      每块 = deflate(块内各条：word \0 phonetic \0 translation \0 exchange \0)
//! ```
//!
//! 三个决定都是为了体积与速度的平衡（77 万条实测）：
//! - **索引只存哈希、不存偏移**：条目顺序与哈希顺序一致，第 i 条就在第 i 个位置，
//!   省掉 8 字节/条（约 6 MB）。32 位哈希会碰撞，所以取出来后必须核对词本身。
//! - **正文分块压缩**：整体压缩就没法随机读，一条一压又压不动（每条才几十字节）。
//!   一块 64 条约 3 KB，压完约一半，解压一块不到 0.1 ms。
//! - **哈希索引常驻、正文按需读**：索引 3 MB 可以一直留着，正文 20 MB 用到哪块读哪块。
//!
//! CSV 不进仓库（65 MB 不该进 git）：`build-dict` 从本地 CSV 生成 `.bin`，
//! 打包脚本再把 `.bin` 投进安装目录。

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"TTDICT\x02\x00";
/// 每个压缩块放几条。64 条约 3 KB：太小则压缩率差、块索引变大；太大则每次查词多解压无用数据
const PER_BLOCK: u32 = 64;

/// 一条词典条目（字段与 ECDICT 对齐）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entry {
    /// 词条本身（保留词库里的原始大小写）
    pub word: String,
    /// 音标，可能为空
    pub phonetic: String,
    /// 中文释义，每行一条「词性. 释义」
    pub translation: String,
    /// 词形变化，ECDICT 原格式：`p:过去式/d:过去分词/i:现在分词/3:三单/r:比较级/t:最高级/s:复数/0:原型`
    pub exchange: String,
}

impl Entry {
    pub fn is_empty(&self) -> bool {
        self.translation.trim().is_empty()
    }

    /// 从 `exchange` 里取原型（`0:xxx`）。
    ///
    /// 变形词在 ECDICT 里往往**自己也是一条**，但释义为空、只有一个指向原型的 `0:`；
    /// 查到这种条目要接着查原型，否则用户看到的是一张空卡片。
    pub fn lemma(&self) -> Option<&str> {
        self.exchange.split('/').find_map(|p| p.strip_prefix("0:")).map(str::trim).filter(|v| !v.is_empty())
    }

    /// 词形变化转成人话，给卡片的「词形」那一行用。认不出的类型直接跳过。
    pub fn forms_zh(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for part in self.exchange.split('/') {
            let Some((k, v)) = part.split_once(':') else { continue };
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            let label = match k.trim() {
                "p" => "过去式",
                "d" => "过去分词",
                "i" => "现在分词",
                "3" => "第三人称单数",
                "r" => "比较级",
                "t" => "最高级",
                "s" => "复数",
                _ => continue, // 0（原型）与 1（变换类型）不是给人看的
            };
            out.push(format!("{label} {v}"));
        }
        out.join("；")
    }
}

/// 打开的词典。哈希索引在内存里，正文按块解压。
pub struct Dict {
    file: File,
    /// 词（小写）的 32 位哈希，升序；下标就是条目号
    hashes: Vec<u32>,
    /// 每块在文件里的起止位置（n+1 项，最后一项是哨兵）
    blocks: Vec<u64>,
    per_block: u32,
    /// 最近解压的一块。连着查同一块（比如先查变形再查原型）时不必重复解压
    cache: Option<(u32, Vec<u8>)>,
}

impl Dict {
    pub fn open(path: &Path) -> anyhow::Result<Dict> {
        let mut file = File::open(path).map_err(|e| anyhow::anyhow!("打不开词库 {}：{e}", path.display()))?;
        let mut head = [0u8; 16];
        file.read_exact(&mut head).map_err(|e| anyhow::anyhow!("词库 {} 读不出文件头：{e}", path.display()))?;
        anyhow::ensure!(&head[..8] == MAGIC, "{} 不是本程序的词库文件（格式变了就要重新生成）", path.display());
        let count = u32::from_le_bytes(head[8..12].try_into().unwrap()) as usize;
        let per_block = u32::from_le_bytes(head[12..16].try_into().unwrap()).max(1);

        let mut raw = vec![0u8; count * 4];
        file.read_exact(&mut raw)?;
        let hashes: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();

        let nblocks = count.div_ceil(per_block as usize);
        let mut raw = vec![0u8; (nblocks + 1) * 8];
        file.read_exact(&mut raw)?;
        let blocks: Vec<u64> = raw.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();

        Ok(Dict { file, hashes, blocks, per_block, cache: None })
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    /// 精确查一个词（不区分大小写，不做词形还原）
    pub fn get(&mut self, word: &str) -> Option<Entry> {
        let key = normalize(word);
        if key.is_empty() {
            return None;
        }
        let h = hash32(&key);
        let mut i = self.hashes.partition_point(|x| *x < h);
        // 32 位哈希会碰撞，同一个哈希可能有好几条，挨个核对词本身
        while i < self.hashes.len() && self.hashes[i] == h {
            if let Some(e) = self.entry_at(i) {
                if normalize(&e.word) == key {
                    return Some(e);
                }
            }
            i += 1;
        }
        None
    }

    /// 查词：先精确查；查到空壳变形条目就跟到原型；还查不到就按常见词形还原再试。
    ///
    /// 返回 (条目, 是否做过还原)——做过还原时界面要说明「按原型 xxx 显示」，
    /// 否则用户查 running 看到 run，会以为查错了。
    pub fn lookup(&mut self, word: &str) -> Option<(Entry, bool)> {
        if let Some(e) = self.get(word) {
            if !e.is_empty() {
                return Some((e, false));
            }
            if let Some(base) = e.lemma().map(str::to_string) {
                if let Some(b) = self.get(&base) {
                    if !b.is_empty() {
                        return Some((b, true));
                    }
                }
            }
        }
        for cand in lemma_candidates(&normalize(word)) {
            if let Some(e) = self.get(&cand) {
                if !e.is_empty() {
                    return Some((e, true));
                }
            }
        }
        None
    }

    fn entry_at(&mut self, i: usize) -> Option<Entry> {
        let bi = (i / self.per_block as usize) as u32;
        let within = i % self.per_block as usize;
        let block = self.block(bi)?;
        // 块里各条依次排列，每条四段、每段以 \0 结尾
        let mut it = block.split(|b| *b == 0);
        for _ in 0..within * 4 {
            it.next()?;
        }
        let mut next = || String::from_utf8_lossy(it.next().unwrap_or(&[])).into_owned();
        let word = next();
        if word.is_empty() {
            return None;
        }
        Some(Entry { word, phonetic: next(), translation: next(), exchange: next() })
    }

    fn block(&mut self, bi: u32) -> Option<&[u8]> {
        if self.cache.as_ref().map(|(n, _)| *n) != Some(bi) {
            let (start, end) = (*self.blocks.get(bi as usize)?, *self.blocks.get(bi as usize + 1)?);
            let mut packed = vec![0u8; (end - start) as usize];
            self.file.seek(SeekFrom::Start(start)).ok()?;
            self.file.read_exact(&mut packed).ok()?;
            let raw = miniz_oxide::inflate::decompress_to_vec(&packed).ok()?;
            self.cache = Some((bi, raw));
        }
        self.cache.as_ref().map(|(_, b)| b.as_slice())
    }
}

/// 查词用的归一化：去首尾空白与标点、转小写。撇号与连字符留着（don't、e-mail）
pub fn normalize(w: &str) -> String {
    w.trim()
        .trim_matches(|c: char| c.is_ascii_punctuation() && c != '\'' && c != '-')
        .trim()
        .to_lowercase()
}

/// FNV-1a 折叠成 32 位。**换哈希函数等于换文件格式**：旧词库会整个查不到，别随手改。
pub fn hash32(s: &str) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    (h ^ (h >> 32)) as u32
}

/// 常见词形还原的候选词（顺序即优先级）。
///
/// 只做规则还原：-s/-es/-ies/-ed/-ing/-er/-est/-ly，外加双写还原与补 e。
/// 不规则变化（went → go）靠词库里变形条目的 `0:` 原型指针，不在这里穷举。
pub fn lemma_candidates(w: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if s.len() >= 2 && !out.contains(&s) && s != w {
            out.push(s);
        }
    };
    let n = w.len();
    for suf in ["ies", "es", "s", "ing", "ed", "er", "est", "ly"] {
        if n > suf.len() + 1 && w.ends_with(suf) {
            let stem = &w[..n - suf.len()];
            if suf == "ies" {
                push(format!("{stem}y"));
            }
            push(stem.to_string());
            push(format!("{stem}e"));
            // running → run：结尾双写辅音还原
            let b = stem.as_bytes();
            if b.len() >= 2 && b[b.len() - 1] == b[b.len() - 2] && (b[b.len() - 1] as char).is_ascii_alphabetic() {
                push(stem[..stem.len() - 1].to_string());
            }
        }
    }
    out
}

/// 生成词库文件。`entries` 是 (word, phonetic, translation, exchange)。
///
/// 按哈希排序后写——查询靠二分，顺序错了就查不到。
pub fn build(path: &Path, mut entries: Vec<(String, String, String, String)>) -> anyhow::Result<usize> {
    entries.retain(|(w, _, t, x)| !w.trim().is_empty() && !(t.trim().is_empty() && x.trim().is_empty()));
    let mut rows: Vec<(u32, Vec<u8>)> = entries
        .into_iter()
        .map(|(w, p, t, x)| {
            let mut b = Vec::with_capacity(w.len() + p.len() + t.len() + x.len() + 4);
            for field in [&w, &p, &t, &x] {
                b.extend_from_slice(field.as_bytes());
                b.push(0);
            }
            (hash32(&normalize(&w)), b)
        })
        .collect();
    rows.sort_by_key(|(h, _)| *h);

    let count = rows.len() as u32;
    let mut f = BufWriter::new(File::create(path)?);
    f.write_all(MAGIC)?;
    f.write_all(&count.to_le_bytes())?;
    f.write_all(&PER_BLOCK.to_le_bytes())?;
    for (h, _) in &rows {
        f.write_all(&h.to_le_bytes())?;
    }

    // 先压出每一块才知道块有多大，而块偏移表要写在正文前面，所以分两段攒
    let mut packed: Vec<Vec<u8>> = Vec::new();
    for chunk in rows.chunks(PER_BLOCK as usize) {
        let mut raw = Vec::new();
        for (_, b) in chunk {
            raw.extend_from_slice(b);
        }
        packed.push(miniz_oxide::deflate::compress_to_vec(&raw, 9));
    }
    let head = 16 + count as u64 * 4 + (packed.len() as u64 + 1) * 8;
    let mut at = head;
    for b in &packed {
        f.write_all(&at.to_le_bytes())?;
        at += b.len() as u64;
    }
    f.write_all(&at.to_le_bytes())?; // 哨兵：最后一块的结束位置
    for b in &packed {
        f.write_all(b)?;
    }
    f.flush()?;
    Ok(count as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<(String, String, String, String)> {
        let mut v = vec![
            ("glance".into(), "glɑ:ns".into(), "n. 一瞥\nvi. 扫视".into(), "d:glanced/i:glancing/3:glances".into()),
            ("run".into(), "rʌn".into(), "vi. 跑；奔跑".into(), "p:ran/d:run/i:running/3:runs".into()),
            // 变形条目：自己没释义，只有一个指向原型的 0:
            ("running".into(), String::new(), String::new(), "0:run/1:i".into()),
            ("studies".into(), String::new(), String::new(), "0:study/1:3".into()),
            ("study".into(), "'stʌdi".into(), "n. 学习；研究".into(), "s:studies/p:studied".into()),
            ("Beijing".into(), String::new(), "n. 北京".into(), String::new()),
        ];
        // 凑够好几块，顺便盖住「跨块查询」：只有一块的话块定位算错了也测不出来
        for i in 0..200 {
            v.push((format!("filler{i}"), String::new(), format!("n. 填充 {i}"), String::new()));
        }
        v
    }

    fn open_sample(name: &str) -> Dict {
        let p = std::env::temp_dir().join(name);
        build(&p, sample()).expect("写词库");
        match Dict::open(&p) {
            Ok(d) => d,
            Err(e) => panic!("打开词库失败：{e}"),
        }
    }

    #[test]
    fn exact_lookup_is_case_insensitive() {
        let mut d = open_sample("tt-dict-case.bin");
        assert_eq!(d.get("glance").unwrap().phonetic, "glɑ:ns");
        assert_eq!(d.get("GLANCE").unwrap().word, "glance");
        assert_eq!(d.get("  glance, ").unwrap().word, "glance", "首尾空白与标点不该影响查词");
        assert_eq!(d.get("beijing").unwrap().translation, "n. 北京");
        assert!(d.get("nosuchword").is_none());
    }

    /// 多块词库里每一条都要查得到：块定位（第几块、块内第几条）算错时只有一部分词查得到
    #[test]
    fn every_entry_is_reachable_across_blocks() {
        let mut d = open_sample("tt-dict-blocks.bin");
        assert!(d.len() > PER_BLOCK as usize * 2, "样例要跨好几块才有意义");
        for i in 0..200 {
            let w = format!("filler{i}");
            assert_eq!(d.get(&w).unwrap().translation, format!("n. 填充 {i}"), "{w}");
        }
    }

    /// 变形条目在 ECDICT 里往往没有释义、只有一个指向原型的 0:，
    /// 不跟过去的话用户看到的是一张空卡片
    #[test]
    fn inflected_entry_follows_its_lemma() {
        let mut d = open_sample("tt-dict-lemma.bin");
        let (e, restored) = d.lookup("running").unwrap();
        assert_eq!(e.word, "run");
        assert!(restored, "跟到原型要告诉界面，卡片上才好提示");
        assert_eq!(d.lookup("studies").unwrap().0.word, "study");
    }

    /// 词库里没有那条变形时，靠规则还原兜住（-s/-ed/-ing/双写/补 e）
    #[test]
    fn rule_based_lemmatization_catches_missing_forms() {
        let mut d = open_sample("tt-dict-rules.bin");
        for (q, want) in [("glances", "glance"), ("glanced", "glance"), ("glancing", "glance")] {
            let (e, restored) = d.lookup(q).unwrap_or_else(|| panic!("{q} 应当能还原到 {want}"));
            assert_eq!(e.word, want, "{q}");
            assert!(restored);
        }
    }

    #[test]
    fn candidates_cover_the_common_endings() {
        assert!(lemma_candidates("running").contains(&"run".to_string()));
        assert!(lemma_candidates("studies").contains(&"study".to_string()));
        assert!(lemma_candidates("boxes").contains(&"box".to_string()));
        assert!(lemma_candidates("larger").contains(&"large".to_string()));
        // 太短的词不乱切，免得 "is" 被切成 "i"
        assert!(lemma_candidates("is").is_empty());
    }

    #[test]
    fn forms_and_lemma_are_readable() {
        let e = Entry {
            word: "run".into(),
            exchange: "p:ran/d:run/i:running/3:runs/0:run/1:s".into(),
            ..Default::default()
        };
        let f = e.forms_zh();
        assert!(f.contains("过去式 ran") && f.contains("现在分词 running"));
        assert!(!f.contains("0:") && !f.contains("1:"), "原型与变换类型不给用户看：{f}");
        assert_eq!(e.lemma(), Some("run"));
    }

    /// 空条目不进词库：查到一条什么都没有的记录，比查不到更让人困惑
    #[test]
    fn empty_entries_are_dropped_when_building() {
        let p = std::env::temp_dir().join("tt-dict-empty.bin");
        let n = build(
            &p,
            vec![
                ("".into(), "x".into(), "y".into(), String::new()),
                ("ok".into(), String::new(), "n. 好".into(), String::new()),
                ("hollow".into(), String::new(), String::new(), String::new()),
            ],
        )
        .unwrap();
        assert_eq!(n, 1);
        let mut d = Dict::open(&p).unwrap_or_else(|e| panic!("打开词库失败：{e}"));
        assert_eq!(d.len(), 1);
        assert!(d.get("hollow").is_none());
    }

    #[test]
    fn wrong_magic_is_rejected_with_a_clear_message() {
        let p = std::env::temp_dir().join("tt-dict-bad.bin");
        std::fs::write(&p, b"not a dictionary at all, but long enough to read a header").unwrap();
        let err = match Dict::open(&p) {
            Ok(_) => panic!("不该认这个文件"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("不是本程序的词库文件"), "{err}");
    }
}
