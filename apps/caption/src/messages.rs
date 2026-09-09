use chrono::{DateTime, Local};

pub type SegId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Capture,
    Asr,
    Translate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StageState {
    Ok,
    Warn(String),
    Err(String),
}

#[derive(Debug, Clone)]
pub enum UiMsg {
    /// 正在说的这句的中间识别结果（会被后续 Partial 或 English 替换）；id 与之后的定稿相同
    EnglishPartial { id: SegId, text: String },
    English { id: SegId, at: DateTime<Local>, text: String },
    /// 说话过程中按半句翻出来的临时中文（灰色），定稿译文开始后不再采纳
    ChineseProvisional { id: SegId, text: String },
    /// 流式译文（累积到目前为止的全文）
    ChinesePartial { id: SegId, text: String },
    Chinese { id: SegId, text: String },
    Status { stage: Stage, state: StageState },
}
