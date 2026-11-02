//! Backend abstraction shared by the mock backend and llama.cpp backend.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use llmgui_core::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: String,
    pub model_id: String,
    pub model_path: Option<PathBuf>,
    pub n_ctx: u32,
    pub n_params: Option<u64>,
    pub slots: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadConfig {
    pub n_ctx: u32,
    pub n_batch: u32,
    pub n_gpu_layers: i32,
    pub n_threads: Option<i32>,
    pub kv_type: String,
    pub slots: usize,
}

impl Default for LoadConfig {
    fn default() -> Self {
        Self {
            n_ctx: 32768,
            n_batch: 512,
            n_gpu_layers: -1,
            n_threads: None,
            kv_type: "q8_0".to_string(),
            slots: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub request_id: String,
    pub prompt: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub stop: Vec<String>,
    pub seed: Option<u64>,
    /// Stable hash of the prompt prefix, used by prefix caches.
    pub cache_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateStats {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub ttft_ms: u64,
    pub total_ms: u64,
    pub tokens_per_second: f64,
    pub prefix_cache_hit: bool,
    pub finish_reason: String,
}

impl GenerateStats {
    pub fn finish(
        prompt_tokens: u32,
        completion_tokens: u32,
        started: Instant,
        ttft: Option<Duration>,
        prefix_cache_hit: bool,
        finish_reason: impl Into<String>,
    ) -> Self {
        let total = started.elapsed();
        let total_ms = total.as_millis() as u64;
        let tokens_per_second = if total_ms == 0 {
            0.0
        } else {
            f64::from(completion_tokens) * 1000.0 / total_ms as f64
        };
        Self {
            prompt_tokens,
            completion_tokens,
            ttft_ms: ttft.map(|d| d.as_millis() as u64).unwrap_or(total_ms),
            total_ms,
            tokens_per_second,
            prefix_cache_hit,
            finish_reason: finish_reason.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TokenEvent {
    Token { id: u32, text: String },
    Done { stats: GenerateStats },
    Error { message: String, code: String },
}

/// A blocking inference backend. The scheduler calls `generate` from a worker
/// thread; implementations must honour `cancel` at token boundaries.
pub trait Backend: Send + Sync {
    fn info(&self) -> BackendInfo;
    fn load(&self, model_path: &Path, config: &LoadConfig) -> Result<()>;
    fn unload(&self) -> Result<()>;
    fn tokenize(&self, text: &str, add_special: bool) -> Result<Vec<u32>>;
    fn generate(
        &self,
        request: GenerateRequest,
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<TokenEvent>,
    ) -> Result<()>;
    fn prefix_cache_stats(&self) -> PrefixCacheStats {
        PrefixCacheStats::default()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PrefixCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub saved_prompt_tokens: u64,
    pub entries: usize,
}

pub fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

/// Build a simple ChatML prompt for models without a runtime chat template.
pub fn default_chat_prompt(messages: &[llmgui_core::api::ChatMessage]) -> String {
    let mut out = String::new();
    for message in messages {
        out.push_str("<|im_start|>");
        out.push_str(&message.role);
        out.push('\n');
        out.push_str(&message.text());
        out.push_str("<|im_end|>\n");
    }
    out.push_str("<|im_start|>assistant\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatml_contains_roles() {
        let messages = vec![
            llmgui_core::api::ChatMessage::new("system", "be nice"),
            llmgui_core::api::ChatMessage::new("user", "hello"),
        ];
        let prompt = default_chat_prompt(&messages);
        assert!(prompt.contains("<|im_start|>user"));
        assert!(prompt.ends_with("<|im_start|>assistant\n"));
    }
}
