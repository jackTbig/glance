//! Engine：三后端的统一入口（一个 Engine = 一个系统提示 + 一个后端实例）
use crate::backend::{Backend, BackendConfig};
use crate::claude::ClaudeSession;
use crate::codex::codex_translate;
use crate::openai::OpenAi;
use std::time::{Duration, Instant};

enum Kind {
    Claude(ClaudeSession),
    Codex,
    OpenAi(OpenAi),
}

pub struct Engine {
    kind: Kind,
    cfg: BackendConfig,
    system_prompt: String,
}

impl Engine {
    pub fn new(cfg: &BackendConfig, system_prompt: &str) -> anyhow::Result<Engine> {
        let kind = match cfg.backend {
            Backend::Claude => Kind::Claude(ClaudeSession::spawn(cfg, system_prompt)?),
            Backend::Codex => Kind::Codex,
            Backend::OpenAi => Kind::OpenAi(OpenAi::new(cfg)?),
        };
        Ok(Engine { kind, cfg: cfg.clone(), system_prompt: system_prompt.to_string() })
    }

    pub fn backend(&self) -> Backend {
        self.cfg.backend
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn translate(&mut self, user_message: &str, on_delta: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
        match &mut self.kind {
            Kind::Claude(s) => s.translate(user_message, self.cfg.timeout, on_delta),
            Kind::Codex => codex_translate(&self.cfg, &self.system_prompt, user_message),
            Kind::OpenAi(o) => o.chat(&self.system_prompt, user_message, self.cfg.timeout, on_delta),
        }
    }

    pub fn turns(&self) -> u32 {
        match &self.kind {
            Kind::Claude(s) => s.turns(),
            _ => 0,
        }
    }

    /// claude 常驻会话每 `every` 句轮换一次；其他后端不轮换
    pub fn needs_rotation(&self, every: u32) -> bool {
        matches!(self.kind, Kind::Claude(_)) && self.turns() >= every
    }
}

/// 设置窗「测试翻译」：按给定配置起一次引擎，翻一句，返回译文与耗时（含冷启动）
pub fn test_translate(cfg: &BackendConfig, system_prompt: &str, text: &str) -> anyhow::Result<(String, Duration)> {
    let mut engine = Engine::new(cfg, system_prompt)?;
    let t0 = Instant::now();
    let zh = engine.translate(text, &mut |_| {})?;
    Ok((zh, t0.elapsed()))
}
