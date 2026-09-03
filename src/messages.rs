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
    English { id: SegId, at: DateTime<Local>, text: String },
    Chinese { id: SegId, text: String },
    Status { stage: Stage, state: StageState },
}
