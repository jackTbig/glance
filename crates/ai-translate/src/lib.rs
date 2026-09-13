//! ai-translate：共享翻译核心（live-caption 与翻译工具共用）
//!
//! 底层 [`Engine`] 封装三后端（claude CLI / codex CLI / OpenAI 兼容），系统提示由调用方给；
//! 上层 [`Translator`] 按场景路由后端、限并发、批量打包、重试。
pub mod backend;
pub mod batch;
pub mod claude;
pub mod direction;
pub mod codex;
pub mod engine;
pub mod glossary;
pub mod html_guard;
pub mod openai;
pub mod prompts;
pub mod retry;
pub mod scene;
pub mod semaphore;
pub mod translator;
pub mod util;

pub use backend::{Backend, BackendConfig};
pub use direction::Direction;
pub use engine::{test_translate, Engine};
pub use glossary::{Glossary, GlossaryEntry};
pub use retry::RetryPolicy;
pub use scene::Scene;
pub use translator::{Item, ItemResult, SceneRoute, Translator, TranslatorConfig};
pub use util::resolve_bin;
