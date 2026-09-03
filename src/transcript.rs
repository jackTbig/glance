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

/// 落盘：transcripts/2026-09-03_2055.md
pub struct TranscriptFile {
    pub path: PathBuf,
    file: std::fs::File,
}

impl TranscriptFile {
    pub fn create(dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = PathBuf::from(dir).join(format!("{}.md", Local::now().format("%Y-%m-%d_%H%M")));
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "# 会议字幕 {}\n", Local::now().format("%Y-%m-%d %H:%M"))?;
        Ok(Self { path, file })
    }
    pub fn append(&mut self, line: &str) {
        if let Err(e) = self.file.write_all(line.as_bytes()) {
            tracing::warn!("写记录失败：{e}");
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
}
