//! 文件翻译：一期覆盖 `.docx` / `.md` / `.txt`
//!
//! 设计见 `docs/superpowers/design/2026-10-11-file-translate-design.md`。
//! 复用 `ai_translate::Translator::translate_batch_tuned` + `Scene::FullPage`，
//! 翻译核心一行不动。

use ai_translate::scene::Scene;
use ai_translate::translator::{Item, Translator};
use ai_translate::RetryPolicy;
use anyhow::{anyhow, bail, Context, Result};
use encoding_rs::Encoding;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// 文本块；markdown 与 txt 走这个
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    /// 段内多行用 `\n` 合并
    pub text: String,
}

/// 一份解析后的文件
#[derive(Debug, Clone)]
pub struct ParsedDoc {
    pub blocks: Vec<TextBlock>,
    pub source_hash: String,
}

/// 进度事件
#[derive(Debug, Clone)]
pub enum FileEvent {
    Started { path: PathBuf, total: usize },
    Progress { path: PathBuf, done: usize, total: usize },
    Done { path: PathBuf, output: PathBuf },
    Failed { path: PathBuf, error: String },
}

/// 汇总
#[derive(Debug, Default, Clone)]
pub struct FileSummary {
    pub succeeded: usize,
    pub failed: usize,
    pub outputs: Vec<PathBuf>,
    pub errors: Vec<(PathBuf, String)>,
}

/// 文件格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Docx,
    Markdown,
    Text,
}

impl Kind {
    pub fn from_path(p: &Path) -> Option<Kind> {
        let ext = p.extension()?.to_ascii_lowercase();
        let s = ext.to_string_lossy();
        match s.as_ref() {
            "docx" => Some(Kind::Docx),
            "md" | "markdown" => Some(Kind::Markdown),
            "txt" => Some(Kind::Text),
            _ => None,
        }
    }
}

/// 调系统的「打开文件」对话框，多选
#[cfg(target_os = "windows")]
pub fn pick_files_dialog(_owner_hwnd: Option<isize>) -> Option<Vec<PathBuf>> {
    // 不传父窗口：简译主窗口平时缩在屏幕外，传了反而会让对话框跑去屏幕外。
    // Windows 下裸对话框本身就能拿到前台权限（弹完会被调用线程收回焦点）
    rfd::FileDialog::new()
        .add_filter("可翻译文件", &["docx", "md", "markdown", "txt"])
        .add_filter("Word 文档", &["docx"])
        .add_filter("Markdown", &["md", "markdown"])
        .add_filter("纯文本", &["txt"])
        .add_filter("所有文件", &["*"])
        .pick_files()
}

#[cfg(not(target_os = "windows"))]
pub fn pick_files_dialog(_owner_hwnd: Option<isize>) -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new().pick_files()
}

/// 翻译一组文件
pub fn translate_paths(
    paths: Vec<PathBuf>,
    translator: &Translator,
    chunk: usize,
    mut on_event: impl FnMut(&FileEvent),
) -> FileSummary {
    let mut sum = FileSummary::default();
    let chunk = chunk.max(1);
    for path in paths {
        let kind = match Kind::from_path(&path) {
            Some(k) => k,
            None => {
                let err = "不支持的文件格式".to_string();
                on_event(&FileEvent::Failed { path: path.clone(), error: err.clone() });
                sum.failed += 1;
                sum.errors.push((path, err));
                continue;
            }
        };

        let parsed = match parse(&path, kind) {
            Ok(p) => p,
            Err(e) => {
                let err = format!("解析失败：{e:#}");
                on_event(&FileEvent::Failed { path: path.clone(), error: err.clone() });
                sum.failed += 1;
                sum.errors.push((path, err));
                continue;
            }
        };

        let total = parsed.blocks.len();
        on_event(&FileEvent::Started { path: path.clone(), total });

        let items: Vec<Item> = parsed
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| Item { id: i.to_string(), text: b.text.clone(), is_html: false })
            .collect();
        let results = translator.translate_batch_tuned(Scene::FullPage, items, chunk, &RetryPolicy::default());
        let mut translated: Vec<String> = Vec::with_capacity(total);
        let mut first_err: Option<String> = None;
        let mut done = 0usize;
        for (i, r) in results.into_iter().enumerate() {
            match r.result {
                Ok(t) => {
                    translated.push(t);
                    done += 1;
                    if i + 1 == total || (i + 1) % 10 == 0 {
                        on_event(&FileEvent::Progress { path: path.clone(), done, total });
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(format!("第 {} 段失败：{}", i + 1, e));
                    }
                    translated.push(parsed.blocks.get(i).map(|b| b.text.clone()).unwrap_or_default());
                }
            }
        }

        let output = match output_path(&path) {
            Ok(p) => p,
            Err(e) => {
                let err = format!("生成输出路径失败：{e:#}");
                on_event(&FileEvent::Failed { path: path.clone(), error: err.clone() });
                sum.failed += 1;
                sum.errors.push((path, err));
                continue;
            }
        };

        let write_res = match kind {
            Kind::Docx => write_docx(&path, &output, &translated, &parsed.source_hash),
            Kind::Markdown => write_md(&output, &parsed.blocks, &translated),
            Kind::Text => write_txt(&path, &output, &parsed.blocks, &translated),
        };
        if let Err(e) = write_res {
            let err = format!("写入失败：{e:#}");
            on_event(&FileEvent::Failed { path: path.clone(), error: err.clone() });
            sum.failed += 1;
            sum.errors.push((path, err));
            continue;
        }
        if let Some(e) = first_err {
            on_event(&FileEvent::Done { path: path.clone(), output: output.clone() });
            on_event(&FileEvent::Failed { path: path.clone(), error: format!("部分段翻译失败：{e}") });
            sum.errors.push((path, e));
        } else {
            on_event(&FileEvent::Done { path: path.clone(), output: output.clone() });
        }
        sum.succeeded += 1;
        sum.outputs.push(output);
    }
    sum
}

// ────── 解析 ────────────────────────────────────────────────────────────

fn parse(path: &Path, kind: Kind) -> Result<ParsedDoc> {
    let bytes = fs::read(path).with_context(|| format!("读 {}", path.display()))?;
    let source_hash = blake3_like(&bytes);
    let blocks = match kind {
        Kind::Docx => parse_docx(&bytes)?.blocks,
        Kind::Markdown => parse_md(&bytes)?,
        Kind::Text => parse_txt(&bytes)?,
    };
    Ok(ParsedDoc { blocks, source_hash })
}

fn parse_txt(bytes: &[u8]) -> Result<Vec<TextBlock>> {
    let (text, _enc, _had_bom) = decode_bytes(bytes);
    Ok(split_paragraphs(&text))
}

fn parse_md(bytes: &[u8]) -> Result<Vec<TextBlock>> {
    let (text, _enc, _had_bom) = decode_bytes(bytes);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    let parser = Parser::new_ext(&text, opts);
    let mut blocks: Vec<TextBlock> = Vec::new();
    let mut current: Option<String> = None;
    let mut in_skip: usize = 0;

    for ev in parser {
        match ev {
            Event::Start(tag) => match tag {
                Tag::CodeBlock(_) | Tag::MetadataBlock(_) | Tag::HtmlBlock => in_skip += 1,
                Tag::Paragraph | Tag::Heading { .. } | Tag::Item => current = Some(String::new()),
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::CodeBlock | TagEnd::HtmlBlock => in_skip = in_skip.saturating_sub(1),
                TagEnd::MetadataBlock(_) => in_skip = in_skip.saturating_sub(1),
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => {
                    if let Some(buf) = current.take() {
                        let t = buf.trim();
                        if !t.is_empty() {
                            blocks.push(TextBlock { text: t.to_string() });
                        }
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_skip > 0 {
                    continue;
                }
                if let Some(buf) = current.as_mut() {
                    if !buf.is_empty() && !buf.ends_with(char::is_whitespace) && !t.starts_with(char::is_whitespace) {
                        buf.push(' ');
                    }
                    buf.push_str(&t);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(buf) = current.as_mut() {
                    if !buf.ends_with('\n') {
                        buf.push('\n');
                    }
                }
            }
            _ => {}
        }
    }
    Ok(blocks)
}

fn parse_docx(bytes: &[u8]) -> Result<ParsedDoc> {
    use quick_xml::events::Event as X;
    use quick_xml::Reader;

    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("打开 docx (zip) 失败")?;
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .context("docx 缺 word/document.xml")?
        .read_to_string(&mut xml)
        .context("读 document.xml")?;
    let dbg_path = std::env::temp_dir().join("tt_parse_docx_xml.txt");
std::fs::write(&dbg_path, &xml).ok();
eprintln!("PARSEDOCX wrote {} bytes to {}", xml.len(), dbg_path.display());

    let mut reader = Reader::from_str(&xml);
    // 不 trim_text：原文作者用 `<w:t xml:space="preserve">` 在 `<w:r>` 之间手动插的空格
    // 是有意义的，去掉的话原本带空格的文本会糊在一起。最终段落整体 trim 一次即可
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut blocks: Vec<TextBlock> = Vec::new();
    let mut current_text = String::new();
    let mut in_p = false;
    let mut in_t = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => bail!("docx XML 解析失败：{e}"),
            Ok(X::Eof) => break,
            Ok(X::Start(e)) => match e.name().as_ref() {
                b"w:p" => {
                    in_p = true;
                    current_text.clear();
                }
                b"w:t" => in_t = true,
                b"w:br" | b"w:tab" => {
                    if in_t {
                        current_text.push('\n');
                    }
                }
                _ => {}
            }
            Ok(X::End(e)) => match e.name().as_ref() {
                b"w:p" => {
                    if in_p {
                        let trimmed = current_text.trim().to_string();
                        if !trimmed.is_empty() {
                            blocks.push(TextBlock { text: trimmed });
                        }
                    }
                    in_p = false;
                }
                b"w:t" => in_t = false,
                _ => {}
            },
            Ok(X::Text(t)) => {
                if in_t {
                    let s = t.unescape().unwrap_or_default();
                    current_text.push_str(&s);
                }
            }
            _ => {}
        }
        buf.clear();
    }
    let source_hash = blake3_like(bytes);
    Ok(ParsedDoc { blocks, source_hash })
}

// ────── 写入 ────────────────────────────────────────────────────────────

fn write_txt(
    original: &Path,
    output: &Path,
    blocks: &[TextBlock],
    translated: &[String],
) -> Result<()> {
    let orig_bytes = fs::read(original).with_context(|| format!("读 {}", original.display()))?;
    let (orig_text, _enc, _had_bom) = decode_bytes(&orig_bytes);
    let line_sep = if orig_text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::new();
    for (i, _b) in blocks.iter().enumerate() {
        let t = translated.get(i).map(String::as_str).unwrap_or("");
        out.push_str(t);
        out.push_str(line_sep);
    }
    fs::write(output, out).with_context(|| format!("写 {}", output.display()))?;
    Ok(())
}

fn write_md(output: &Path, blocks: &[TextBlock], translated: &[String]) -> Result<()> {
    // 一期：译文按段原样回写（保持 paragraph 边界），不做 markdown 重排版
    let mut final_text = String::new();
    for (i, _b) in blocks.iter().enumerate() {
        let t = translated.get(i).map(String::as_str).unwrap_or("");
        final_text.push_str(t);
        final_text.push_str("\n\n");
    }
    fs::write(output, final_text).with_context(|| format!("写 {}", output.display()))?;
    Ok(())
}

/// 把译文按原段落文本节点数均分回 `<w:t>` 节点
fn write_docx(
    original: &Path,
    output: &Path,
    translated: &[String],
    _expected_hash: &str,
) -> Result<()> {
    let bytes = fs::read(original).with_context(|| format!("读 {}", original.display()))?;
    let mut zip_in = zip::ZipArchive::new(Cursor::new(&bytes)).context("打开 docx (zip) 失败")?;
    let mut xml = String::new();
    {
        let mut f = zip_in.by_name("word/document.xml").context("docx 缺 word/document.xml")?;
        f.read_to_string(&mut xml).context("读 document.xml")?;
    }

    let paragraphs = collect_docx_paragraphs(&xml);

    let mut out_xml = String::with_capacity(xml.len() + 4096);
    let mut cursor = 0usize;
    for (i, para) in paragraphs.iter().enumerate() {
        out_xml.push_str(&xml[cursor..para.start]);
        let translated_text = translated.get(i).cloned().unwrap_or_default();
        if para.t_runs.is_empty() || translated_text.is_empty() {
            out_xml.push_str(&xml[para.start..para.end]);
        } else {
            let parts = split_evenly(&translated_text, para.t_runs.len());
            let mut sub_cursor = para.start;
            for (k, t_span) in para.t_runs.iter().enumerate() {
                out_xml.push_str(&xml[sub_cursor..t_span.0]);
                let part = parts.get(k).map(String::as_str).unwrap_or("");
                out_xml.push_str(&escape_xml(part));
                sub_cursor = t_span.1;
            }
            out_xml.push_str(&xml[sub_cursor..para.end]);
        }
        cursor = para.end;
    }
    out_xml.push_str(&xml[cursor..]);

    write_zip_with_replaced_xml(&bytes, output, "word/document.xml", out_xml.as_bytes())
}

#[derive(Debug)]
struct DocxParagraph {
    /// 段落 XML 在原文里的字节范围 [start, end)；end 是 `</w:p>` 标签结束之后的位置
    start: usize,
    end: usize,
    /// 段落内 `<w:t>` 文本节点在原文中的字节范围 [(start, end)]
    t_runs: Vec<(usize, usize)>,
}

fn collect_docx_paragraphs(xml: &str) -> Vec<DocxParagraph> {
    use quick_xml::events::Event as X;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out: Vec<DocxParagraph> = Vec::new();

    let mut current: Option<DocxParagraph> = None;
    let mut in_t = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Err(_) => break,
            Ok(X::Eof) => break,
            Ok(X::Start(e)) => {
                if e.name().as_ref() == b"w:p" && current.is_none() {
                    current = Some(DocxParagraph {
                        start: reader.buffer_position() as usize,
                        end: reader.buffer_position() as usize,
                        t_runs: Vec::new(),
                    });
                } else if e.name().as_ref() == b"w:t" && current.is_some() {
                    in_t = true;
                }
            }
            Ok(X::End(e)) => {
                if e.name().as_ref() == b"w:p" {
                    if let Some(mut p) = current.take() {
                        // end 设为当前 reader 位置之后（End 事件已消费完毕）
                        p.end = reader.buffer_position() as usize;
                        out.push(p);
                    }
                } else if e.name().as_ref() == b"w:t" && in_t {
                    in_t = false;
                }
            }
            Ok(X::Text(_)) => {
                if in_t {
                    if let Some(ref mut p) = current {
                        // buffer_position 是事件结束位置；text_start = 结束 - 文本字节数
                        let end_pos = reader.buffer_position() as usize;
                        let text_len = buf.len();
                        let start_pos = end_pos.saturating_sub(text_len);
                        p.t_runs.push((start_pos, end_pos));
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    out
}

fn write_zip_with_replaced_xml(
    original: &[u8],
    output: &Path,
    member_name: &str,
    new_bytes: &[u8],
) -> Result<()> {
    let mut zip_in = zip::ZipArchive::new(Cursor::new(original)).context("打开 docx (zip) 失败")?;
    let out_file = fs::File::create(output).with_context(|| format!("创建 {}", output.display()))?;
    let mut zip_out = zip::ZipWriter::new(out_file);
    let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut names: Vec<String> = (0..zip_in.len())
        .map(|i| zip_in.by_index(i).map(|f| f.name().to_string()).unwrap_or_default())
        .collect();
    names.sort();
    names.dedup();

    for name in names {
        let mut entry = zip_in.by_name(&name).with_context(|| format!("读 {name}"))?;
        if name == member_name {
            zip_out.start_file(&name, opts)?;
            zip_out.write_all(new_bytes)?;
        } else {
            zip_out.start_file(&name, opts)?;
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            zip_out.write_all(&buf)?;
        }
    }
    zip_out.finish()?;
    Ok(())
}

/// 把译文平均切成 n 段。中文算 1，ASCII 算 0.5。
/// 文本字符数少于 n 时，全部塞进最后一个桶，前面 n-1 个空字符串
/// （docx 写回时空 `<w:t>` 节点保留原结构，比写一个 char 进去再补 pad 更稳）。
fn split_evenly(text: &str, n: usize) -> Vec<String> {
    if n <= 1 {
        return vec![text.to_string()];
    }
    if text.chars().count() < n {
        let mut parts = vec![String::new(); n - 1];
        parts.push(text.to_string());
        return parts;
    }
    let total_width: f64 = text.chars().map(|c| if c.is_ascii() { 0.5 } else { 1.0 }).sum();
    let target = total_width / n as f64;
    let chars: Vec<char> = text.chars().collect();
    let mut parts: Vec<String> = Vec::with_capacity(n);
    let mut cur = String::new();
    let mut cur_w = 0.0f64;
    for (_i, &c) in chars.iter().enumerate() {
        let w = if c.is_ascii() { 0.5 } else { 1.0 };
        // 已经收齐 n-1 个桶时，剩余字符全归最后一个桶
        if parts.len() == n - 1 {
            cur.push(c);
            continue;
        }
        cur.push(c);
        cur_w += w;
        if cur_w >= target {
            parts.push(std::mem::take(&mut cur));
            cur_w = 0.0;
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    while parts.len() < n {
        parts.push(String::new());
    }
    parts
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

// ────── 工具 ────────────────────────────────────────────────────────────

/// 输出路径：foo.md → foo.zh.md；同名存在 → foo.zh (1).md，最多 99。
/// 输入已是 `foo.zh.md` 时同样视为「要再翻一次」，输出 `foo.zh (1).md`（不复用 stem 避免覆盖源）。
fn output_path(original: &Path) -> Result<PathBuf> {
    let parent = original.parent().ok_or_else(|| anyhow!("路径没有目录部分"))?;
    let stem = original
        .file_stem()
        .ok_or_else(|| anyhow!("路径没有文件名"))?
        .to_string_lossy()
        .to_string();
    let ext = original
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let already_zh = stem.to_ascii_lowercase().ends_with(".zh");
    // 已带 .zh 的输入：保留 .zh（避免覆盖原文件）；否则加 .zh
    let new_stem = if already_zh { stem } else { format!("{stem}.zh") };
    let candidate = if ext.is_empty() {
        parent.join(&new_stem)
    } else {
        parent.join(format!("{new_stem}.{ext}"))
    };
    if !candidate.exists() {
        return Ok(candidate);
    }
    for i in 1..=99 {
        let c = if ext.is_empty() {
            parent.join(format!("{new_stem} ({i})"))
        } else {
            parent.join(format!("{new_stem} ({i}).{ext}"))
        };
        if !c.exists() {
            return Ok(c);
        }
    }
    bail!("找不到可用的输出文件名（已尝试 99 次）")
}

/// 段间分隔：1 个空行 = 段落分隔（这是 markdown 的约定，txt 也沿用）；段内的多行用 `\n` 拼起来。
fn split_paragraphs(text: &str) -> Vec<TextBlock> {
    let mut out: Vec<TextBlock> = Vec::new();
    let mut current = String::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            // 空行 = 段落分隔
            let t = current.trim().to_string();
            if !t.is_empty() {
                out.push(TextBlock { text: t });
            }
            current.clear();
            continue;
        }
        if !current.is_empty() && !current.ends_with('\n') {
            current.push('\n');
        }
        current.push_str(line);
    }
    let t = current.trim().to_string();
    if !t.is_empty() {
        out.push(TextBlock { text: t });
    }
    out
}

fn decode_bytes(bytes: &[u8]) -> (String, &'static Encoding, bool) {
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        let (s, _enc) = enc.decode_without_bom_handling(&bytes[bom_len..]);
        return (s.into_owned(), enc, true);
    }
    let mut det = chardetng::EncodingDetector::new();
    det.feed(bytes, true);
    let enc: &'static Encoding = det.guess(None, true);
    let (s, used, _err) = enc.decode(bytes);
    (s.into_owned(), used, false)
}

fn blake3_like(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_detection() {
        assert_eq!(Kind::from_path(Path::new("a.docx")), Some(Kind::Docx));
        assert_eq!(Kind::from_path(Path::new("A.DOCX")), Some(Kind::Docx));
        assert_eq!(Kind::from_path(Path::new("a.md")), Some(Kind::Markdown));
        assert_eq!(Kind::from_path(Path::new("a.markdown")), Some(Kind::Markdown));
        assert_eq!(Kind::from_path(Path::new("a.txt")), Some(Kind::Text));
        assert_eq!(Kind::from_path(Path::new("a.pdf")), None);
        assert_eq!(Kind::from_path(Path::new("noext")), None);
    }

    #[test]
    fn parse_txt_basic() {
        let b = parse_txt(b"hello\nworld\n\nfoo bar").unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].text, "hello\nworld");
        assert_eq!(b[1].text, "foo bar");
    }

    #[test]
    fn parse_txt_crlf() {
        let b = parse_txt(b"line1\r\nline2\r\n\r\nline3").unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].text, "line1\nline2");
        assert_eq!(b[1].text, "line3");
    }

    #[test]
    fn parse_md_skips_code() {
        let md = b"# Title\n\nHello world.\n\n```rust\nfn x() {}\n```\n\nMore text.";
        let b = parse_md(md).unwrap();
        let texts: Vec<&str> = b.iter().map(|x| x.text.as_str()).collect();
        assert!(texts.contains(&"Title"), "标题应该翻");
        assert!(texts.contains(&"Hello world."), "段落应该翻");
        assert!(texts.contains(&"More text."), "段后段落应该翻");
        assert!(!texts.iter().any(|t| t.contains("fn x()")), "代码块不翻");
    }

    #[test]
    fn parse_md_skips_inline_code_and_links() {
        let md = b"Use `foo()` to [click](https://example.com) here.";
        let b = parse_md(md).unwrap();
        assert_eq!(b.len(), 1);
        assert!(!b[0].text.contains("https://example.com"));
    }

    #[test]
    fn parse_md_heading_and_list() {
        let md = b"## Section\n\n- item one\n- item two\n";
        let b = parse_md(md).unwrap();
        let texts: Vec<&str> = b.iter().map(|x| x.text.as_str()).collect();
        assert!(texts.contains(&"Section"));
        assert!(texts.iter().any(|t| t.contains("item one")));
        assert!(texts.iter().any(|t| t.contains("item two")));
    }

    #[test]
    fn split_evenly_works_for_cjk() {
        let parts = split_evenly("一二三四五六七八九十", 3);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| !p.is_empty()));
    }

    #[test]
    fn split_evenly_handles_short_input() {
        assert_eq!(split_evenly("hi", 5), vec!["", "", "", "", "hi"]);
    }

    #[test]
    fn output_path_avoids_collision() {
        let tmp = std::env::temp_dir().join("tt_ft_test_output_path");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let a = tmp.join("foo.md");
        fs::write(&a, "x").unwrap();
        let p1 = output_path(&a).unwrap();
        assert_eq!(p1, tmp.join("foo.zh.md"));
        fs::write(&p1, "y").unwrap();
        let p2 = output_path(&a).unwrap();
        assert_eq!(p2, tmp.join("foo.zh (1).md"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn output_path_handles_already_zh() {
        let tmp = std::env::temp_dir().join("tt_ft_test_already_zh");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let a = tmp.join("foo.zh.md");
        fs::write(&a, "x").unwrap();
        let p = output_path(&a).unwrap();
        // foo.zh.md 已存在，输出加序号；不复用 .zh（避免覆盖源文件）
        assert_eq!(p, tmp.join("foo.zh (1).md"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn decode_bytes_handles_bom() {
        let (s, _enc, had_bom) = decode_bytes(&[0xEF, 0xBB, 0xBF, b'h', b'i']);
        assert!(had_bom);
        assert_eq!(s, "hi");
    }

    /// 调试：跑一次 zip 往返后的 docx XML，看看到底是什么字符让 quick-xml 报错
    #[test]
    #[ignore] // 仅排查时手动跑，平时 `cargo test` 跳过
    fn debug_docx_after_zip() {
        use quick_xml::events::Event as X;
        use quick_xml::Reader;
        use std::io::Write;
        let doc_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:r><w:t>Hello world.</w:t></w:r></w:p>
<w:p><w:r><w:t>Second.</w:t></w:r></w:p>
</w:body>
</w:document>"#;
        let buf = std::io::Cursor::new(Vec::<u8>::new());
        let mut z = zip::ZipWriter::new(buf);
        let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("word/document.xml", opts).unwrap();
        z.write_all(doc_xml.as_bytes()).unwrap();
        let bytes = z.finish().unwrap().into_inner();
        let mut zip_in = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        use std::io::Read;
        zip_in.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
        eprintln!("xml first 80: {:?}", &xml.as_bytes()[..80.min(xml.len())]);
        let mut reader = Reader::from_str(&xml);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Err(e) => { eprintln!("ERR: {e} at pos {} buf.len={} buf={:?}", reader.buffer_position(), buf.len(), String::from_utf8_lossy(&buf)); break; }
                Ok(X::Eof) => break,
                Ok(X::Start(s)) => eprintln!("Start: {:?}", std::str::from_utf8(s.name().as_ref()).ok()),
                Ok(X::End(e)) => eprintln!("End:   {:?}", std::str::from_utf8(e.name().as_ref()).ok()),
                Ok(X::Text(_)) => eprintln!("Text event at pos {}", reader.buffer_position()),
                _ => {}
            }
            buf.clear();
        }
    }

    /// 现造一个最小 docx（zip 含 [Content_Types].xml / _rels/.rels / word/document.xml），
    /// 验 parse → write → 再 parse 文本保持一致，且翻译文本被均分到所有 `<w:t>` 节点
    #[test]
    fn docx_round_trip_preserves_paragraphs() {
        let tmp = std::env::temp_dir().join("tt_ft_test_docx_round_trip");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("src.docx");
        let dst = tmp.join("src.zh.docx");

        // 构造 document.xml：两段，每段一个 `<w:r><w:t>` 文本节点
        let doc_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:r><w:t>Hello world.</w:t></w:r></w:p>
<w:p><w:r><w:t>Second paragraph.</w:t></w:r></w:p>
</w:body>
</w:document>"#;
        let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

        {
            let file = fs::File::create(&src).unwrap();
            let mut z = zip::ZipWriter::new(file);
            let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            z.start_file("[Content_Types].xml", opts).unwrap();
            z.write_all(content_types.as_bytes()).unwrap();
            z.start_file("_rels/.rels", opts).unwrap();
            z.write_all(rels.as_bytes()).unwrap();
            z.start_file("word/document.xml", opts).unwrap();
            z.write_all(doc_xml.as_bytes()).unwrap();
            z.finish().unwrap();
        }

        let parsed = parse(&src, Kind::Docx).unwrap();
        assert_eq!(parsed.blocks.len(), 2);
        assert_eq!(parsed.blocks[0].text, "Hello world.");
        assert_eq!(parsed.blocks[1].text, "Second paragraph.");

        // 模拟翻译：第一段长一些，第二段短一些
        let translated = vec!["你好世界".to_string(), "第二段".to_string()];
        write_docx(&src, &dst, &translated, &parsed.source_hash).unwrap();

        let reparsed = parse(&dst, Kind::Docx).unwrap();
        assert_eq!(reparsed.blocks.len(), 2);
        assert_eq!(reparsed.blocks[0].text, translated[0]);
        assert_eq!(reparsed.blocks[1].text, translated[1]);

        let _ = fs::remove_dir_all(&tmp);
    }

    /// 一个段落里有多个 `<w:t>` 节点时（典型情况：原文带格式标记切开的文本），
    /// 译文应被均分到这些节点上；段落级重 parse 后拼回去等于原文
    #[test]
    fn docx_round_trip_splits_across_runs() {
        let tmp = std::env::temp_dir().join("tt_ft_test_docx_runs");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("multi.docx");
        let dst = tmp.join("multi.zh.docx");

        // 一段内三个 `<w:t>` 节点：模拟带粗体 / 斜体 / 普通 三段切开的文本
        let doc_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:r><w:t xml:space="preserve">Hello </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r><w:r><w:t xml:space="preserve"> world.</w:t></w:r></w:p>
</w:body>
</w:document>"#;
        let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
        {
            let file = fs::File::create(&src).unwrap();
            let mut z = zip::ZipWriter::new(file);
            let opts = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            z.start_file("[Content_Types].xml", opts).unwrap();
            z.write_all(content_types.as_bytes()).unwrap();
            z.start_file("_rels/.rels", opts).unwrap();
            z.write_all(rels.as_bytes()).unwrap();
            z.start_file("word/document.xml", opts).unwrap();
            z.write_all(doc_xml.as_bytes()).unwrap();
            z.finish().unwrap();
        }

        let parsed = parse(&src, Kind::Docx).unwrap();
        assert_eq!(parsed.blocks.len(), 1);
        assert_eq!(parsed.blocks[0].text, "Hello bold world.");

        // 译文 7 字，平分到 3 个 `<w:t>` 节点
        let translated = vec!["一二三四五六七".to_string()];
        write_docx(&src, &dst, &translated, &parsed.source_hash).unwrap();

        let reparsed = parse(&dst, Kind::Docx).unwrap();
        assert_eq!(reparsed.blocks.len(), 1);
        assert_eq!(reparsed.blocks[0].text, "一二三四五六七");

        let _ = fs::remove_dir_all(&tmp);
    }
}