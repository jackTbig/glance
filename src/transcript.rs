use crate::messages::SegId;
use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

pub fn format_line(at: DateTime<Local>, en: &str, zh: &str) -> String {
    format!("[{}] EN: {en}\n           ZH: {zh}\n", at.format("%H:%M:%S"))
}

/// 内存配对：英文先到，中文后到时产出一行
#[derive(Default)]
pub struct Transcript {
    pending: HashMap<SegId, (DateTime<Local>, String)>,
}

impl Transcript {
    pub fn push_en(&mut self, id: SegId, at: DateTime<Local>, en: &str) -> Option<String> {
        self.pending.insert(id, (at, en.to_string()));
        None
    }
    pub fn push_zh(&mut self, id: SegId, zh: &str) -> Option<String> {
        let (at, en) = self.pending.remove(&id)?;
        Some(format_line(at, &en, zh))
    }
}

/// 落盘：transcripts/2026-09-03_2055.md。
/// 文件在写入第一行时才真正创建（否则每次启动都会留下只有标题的空文件）。
pub struct TranscriptFile {
    pub path: PathBuf,
    file: Option<std::fs::File>,
}

impl TranscriptFile {
    pub fn create(dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = PathBuf::from(dir).join(format!("{}.md", Local::now().format("%Y-%m-%d_%H%M")));
        Ok(Self { path, file: None })
    }

    /// 确保文件存在（带标题行）；打开文件给外部程序看之前调用
    pub fn ensure_created(&mut self) -> bool {
        if self.file.is_some() {
            return true;
        }
        match std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(mut f) => {
                if f.metadata().map_or(0, |m| m.len()) == 0 {
                    let _ = writeln!(f, "# 会议字幕 {}\n", Local::now().format("%Y-%m-%d %H:%M"));
                }
                self.file = Some(f);
                true
            }
            Err(e) => {
                tracing::warn!("建记录文件失败：{e}");
                false
            }
        }
    }

    pub fn append(&mut self, line: &str) {
        if !self.ensure_created() {
            return;
        }
        if let Some(f) = self.file.as_mut() {
            if let Err(e) = f.write_all(line.as_bytes()) {
                tracing::warn!("写记录失败：{e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn line_format() {
        let at = chrono::Local.with_ymd_and_hms(2026, 9, 3, 20, 55, 12).unwrap();
        let l = format_line(at, "Hello team.", "大家好。");
        assert_eq!(l, "[20:55:12] EN: Hello team.\n           ZH: 大家好。\n");
    }

    #[test]
    fn pairs_english_with_later_chinese() {
        let at = chrono::Local::now();
        let mut t = Transcript::default();
        assert!(t.push_en(1, at, "a").is_none());
        assert!(t.push_en(2, at, "b").is_none());
        assert!(t.push_zh(2, "乙").unwrap().contains("EN: b"));
        assert!(t.push_zh(1, "甲").unwrap().contains("ZH: 甲"));
        assert!(t.push_zh(9, "无").is_none(), "未知段号忽略");
    }

    #[test]
    fn file_is_created_lazily_with_header() {
        let dir = std::env::temp_dir().join(format!("lc-transcript-test-{}", std::process::id()));
        let mut f = TranscriptFile::create(&dir.to_string_lossy()).unwrap();
        assert!(!f.path.exists(), "创建对象时不应落盘");
        f.append("[00:00:00] EN: x\n           ZH: y\n");
        let s = std::fs::read_to_string(&f.path).unwrap();
        assert!(s.starts_with("# 会议字幕 "), "{s}");
        assert!(s.contains("EN: x"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
