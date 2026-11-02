//! Deterministic mock backend used by CI, `llmgui serve --backend mock`, and
//! tests that must not depend on a GPU or model download.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use llmgui_backend_api::{
    Backend, BackendInfo, GenerateRequest, GenerateStats, LoadConfig, TokenEvent,
    default_chat_prompt,
};
use llmgui_core::api::ChatMessage;
use llmgui_core::{LlmguiError, Result};
use tokio::sync::mpsc::UnboundedSender;

pub struct MockBackend {
    model_id: String,
    model_path: Mutex<Option<PathBuf>>,
    n_ctx: AtomicU64,
    calls: AtomicU64,
    delay_ms: u64,
}

impl MockBackend {
    pub fn new(delay_ms: u64) -> Self {
        Self {
            model_id: "mock".to_string(),
            model_path: Mutex::new(None),
            n_ctx: AtomicU64::new(32768),
            calls: AtomicU64::new(0),
            delay_ms,
        }
    }

    pub fn with_model_id(mut self, id: impl Into<String>) -> Self {
        self.model_id = id.into();
        self
    }

    fn response_text(&self, prompt: &str) -> String {
        let tail: String = prompt
            .chars()
            .rev()
            .take(40)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!(
            "这是一条来自 llmgui mock 后端的演示回答。当前提示词末尾是：{tail}。请配置真实 GGUF 模型以获得实际模型输出。"
        )
    }
}

impl Backend for MockBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "mock".to_string(),
            model_id: self.model_id.clone(),
            model_path: self.model_path.lock().unwrap().clone(),
            n_ctx: self.n_ctx.load(Ordering::Relaxed) as u32,
            n_params: Some(0),
            slots: 1,
        }
    }

    fn load(&self, model_path: &Path, config: &LoadConfig) -> Result<()> {
        *self.model_path.lock().unwrap() = Some(model_path.to_path_buf());
        self.n_ctx.store(u64::from(config.n_ctx), Ordering::Relaxed);
        Ok(())
    }

    fn unload(&self) -> Result<()> {
        *self.model_path.lock().unwrap() = None;
        Ok(())
    }

    fn tokenize(&self, text: &str, _add_special: bool) -> Result<Vec<u32>> {
        Ok(text.chars().map(|c| c as u32).collect())
    }

    fn generate(
        &self,
        request: GenerateRequest,
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<TokenEvent>,
    ) -> Result<()> {
        let started = Instant::now();
        self.calls.fetch_add(1, Ordering::Relaxed);
        let prompt_tokens = self.tokenize(&request.prompt, false)?.len() as u32;
        let text = if request.prompt.contains("<|im_start|>") {
            self.response_text(&request.prompt)
        } else {
            let msg = ChatMessage::new("user", &request.prompt);
            self.response_text(&default_chat_prompt(&[msg]))
        };
        let mut completion_tokens = 0u32;
        let mut ttft = None;
        for ch in text.chars() {
            if cancel.load(Ordering::Relaxed) {
                let stats = GenerateStats::finish(
                    prompt_tokens,
                    completion_tokens,
                    started,
                    ttft,
                    false,
                    "cancelled",
                );
                let _ = tx.send(TokenEvent::Done { stats });
                return Ok(());
            }
            if completion_tokens >= request.max_tokens {
                break;
            }
            if ttft.is_none() {
                ttft = Some(started.elapsed());
            }
            let _ = tx.send(TokenEvent::Token {
                id: ch as u32,
                text: ch.to_string(),
            });
            completion_tokens += 1;
            if self.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(self.delay_ms));
            }
        }
        let stats = GenerateStats::finish(
            prompt_tokens,
            completion_tokens,
            started,
            ttft,
            false,
            "stop",
        );
        let _ = tx.send(TokenEvent::Done { stats });
        Ok(())
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new(2)
    }
}

/// Fail loudly when a caller accidentally uses mock for production commands.
pub fn require_real_backend() -> Result<()> {
    Err(LlmguiError::Unsupported(
        "this build has no llama.cpp backend; rebuild with --features llama".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmgui_backend_api::TokenEvent;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn mock_streams_and_finishes() {
        let backend = MockBackend::new(0);
        backend
            .load(Path::new("x.gguf"), &LoadConfig::default())
            .unwrap();
        let (tx, mut rx) = unbounded_channel();
        let req = GenerateRequest {
            request_id: "r1".into(),
            prompt: "hello".into(),
            max_tokens: 8,
            temperature: 0.0,
            top_p: 1.0,
            stop: vec![],
            seed: Some(1),
            cache_key: "k".into(),
        };
        backend
            .generate(req, Arc::new(AtomicBool::new(false)), tx)
            .unwrap();
        let mut tokens = 0;
        let mut done = false;
        while let Some(event) = rx.blocking_recv() {
            match event {
                TokenEvent::Token { .. } => tokens += 1,
                TokenEvent::Done { .. } => {
                    done = true;
                    break;
                }
                TokenEvent::Error { message, .. } => panic!("{message}"),
            }
        }
        assert!(tokens > 0 && tokens <= 8);
        assert!(done);
    }
}
