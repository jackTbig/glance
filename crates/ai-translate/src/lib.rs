//! ai-translate：共享翻译核心（live-caption 与翻译工具共用）
//!
//! 底层 [`Engine`] 封装三后端（claude CLI / codex CLI / OpenAI 兼容），系统提示由调用方给；
//! 上层 [`Translator`] 按场景路由后端、限并发、批量打包、重试。
pub mod backend;
pub mod claude;
pub mod codex;
pub mod engine;
pub mod openai;
pub mod util;

pub use backend::{Backend, BackendConfig};
pub use engine::{test_translate, Engine};
pub use util::resolve_bin;
