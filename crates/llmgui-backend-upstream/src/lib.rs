//! OpenAI-compatible upstream backend.
//!
//! This is a pragmatic fallback for environments where the in-process
//! llama.cpp FFI cannot run (for example Windows 11 build 26300 + Arrow Lake).
//! It forwards `/v1/completions` requests to a local `llama-server` (or any
//! OpenAI-compatible server) and streams tokens back through the same
//! `Backend` trait used by the in-process backend.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use llmgui_backend_api::{
    GenerateRequest, GenerateStats, LoadConfig, PrefixCacheStats, TokenEvent,
};
use llmgui_core::{LlmguiError, Result};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum UpstreamKind {
    #[default]
    Generic,
    LlamaServer,
    TabbyApi,
}

impl UpstreamKind {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("llama-server") | Some("llama_server") => Self::LlamaServer,
            Some("tabbyapi") | Some("tabby") => Self::TabbyApi,
            _ => Self::Generic,
        }
    }

    /// `cache_prompt` is a llama-server extension; TabbyAPI ignores or rejects
    /// unknown fields depending on version, so only send it when requested.
    fn sends_cache_prompt(self) -> bool {
        matches!(self, Self::LlamaServer)
    }
}

pub struct UpstreamBackend {
    base_url: String,
    model: String,
    kind: UpstreamKind,
    http: ureq::Agent,
    n_ctx: AtomicU64,
    requests: AtomicU64,
}

impl UpstreamBackend {
    /// `base_url` should be e.g. `http://127.0.0.1:18080` (no `/v1` suffix).
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let http = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(300))
            .build();
        Ok(Self {
            base_url,
            model: "local".to_string(),
            kind: UpstreamKind::Generic,
            http,
            n_ctx: AtomicU64::new(32768),
            requests: AtomicU64::new(0),
        })
    }

    pub fn from_env() -> Result<Self> {
        let url = std::env::var("LLMGUI_UPSTREAM_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:18080".to_string());
        let mut backend = Self::new(url)?;
        if let Ok(model) = std::env::var("LLMGUI_UPSTREAM_MODEL")
            && !model.trim().is_empty()
        {
            backend.model = model;
        }
        backend.kind = UpstreamKind::parse(std::env::var("LLMGUI_UPSTREAM_KIND").ok().as_deref());
        Ok(backend)
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/completions", self.base_url)
    }

    fn health(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);
        match self.http.get(&url).call() {
            Ok(response) if response.status() < 400 => Ok(()),
            Ok(response) => Err(LlmguiError::Backend(format!(
                "upstream health returned {}; check that llama-server/TabbyAPI is ready",
                response.status()
            ))),
            Err(error) => Err(LlmguiError::Backend(format!(
                "upstream health failed ({error}); start llama-server/TabbyAPI first, or set LLMGUI_UPSTREAM_URL"
            ))),
        }
    }
}

impl Default for UpstreamBackend {
    fn default() -> Self {
        Self::from_env().expect("LLMGUI_UPSTREAM_URL invalid")
    }
}

impl llmgui_backend_api::Backend for UpstreamBackend {
    fn info(&self) -> llmgui_backend_api::BackendInfo {
        llmgui_backend_api::BackendInfo {
            name: "upstream".to_string(),
            model_id: self.model.clone(),
            model_path: None,
            n_ctx: self.n_ctx.load(Ordering::Relaxed) as u32,
            n_params: None,
            slots: 1,
        }
    }

    fn load(&self, model_path: &Path, config: &LoadConfig) -> Result<()> {
        self.health()?;
        self.n_ctx.store(u64::from(config.n_ctx), Ordering::Relaxed);
        let _ = model_path;
        Ok(())
    }

    fn unload(&self) -> Result<()> {
        Ok(())
    }

    fn tokenize(&self, text: &str, _add_special: bool) -> Result<Vec<u32>> {
        // Approximate token count for stats when the upstream server does not
        // expose tokenize. The real prompt token count is reported in usage.
        let estimate = (text.chars().count() / 4).max(1);
        Ok((0..estimate).map(|i| i as u32).collect())
    }

    fn generate(
        &self,
        request: GenerateRequest,
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<TokenEvent>,
    ) -> Result<()> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let prompt_tokens = (request.prompt.chars().count() / 4).max(1) as u32;
        let mut body = serde_json::json!({
            "model": self.model,
            "prompt": request.prompt,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "top_p": request.top_p,
            "stream": true,
            "stop": request.stop,
            "seed": request.seed,
        });
        if self.kind.sends_cache_prompt() {
            body["cache_prompt"] = serde_json::Value::Bool(true);
        }
        let response = match self.http.post(&self.endpoint()).send_json(&body) {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                let text = response.into_string().unwrap_or_default();
                return Err(LlmguiError::Backend(format!(
                    "upstream returned {status}: {}",
                    text.chars().take(500).collect::<String>()
                )));
            }
            Err(error) => {
                return Err(LlmguiError::Backend(format!("upstream request: {error}")));
            }
        };
        let mut reader = std::io::BufReader::new(response.into_reader());
        let mut line = String::new();
        let mut completion_tokens = 0u32;
        let mut ttft = None;
        let mut finish_reason = "stop".to_string();
        loop {
            if cancel.load(Ordering::Relaxed) {
                finish_reason = "cancelled".to_string();
                break;
            }
            line.clear();
            use std::io::BufRead;
            let read = reader
                .read_line(&mut line)
                .map_err(|e| LlmguiError::Backend(format!("read upstream: {e}")))?;
            if read == 0 {
                break;
            }
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                break;
            }
            let Ok(value) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            if let Some(error) = value.get("error") {
                return Err(LlmguiError::Backend(format!("upstream error: {error}")));
            }
            if let Some(usage) = value.get("usage").and_then(Value::as_object)
                && let Some(tokens) = usage.get("completion_tokens").and_then(Value::as_u64)
            {
                completion_tokens = completion_tokens.max(tokens as u32);
            }
            if let Some(choice) = value
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|c| c.first())
            {
                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    finish_reason = reason.to_string();
                }
                if let Some(text) = choice.get("text").and_then(Value::as_str).or_else(|| {
                    choice
                        .get("delta")
                        .and_then(|d| d.get("content"))
                        .and_then(Value::as_str)
                }) && !text.is_empty()
                {
                    if ttft.is_none() {
                        ttft = Some(started.elapsed());
                    }
                    completion_tokens += 1;
                    let _ = tx.send(TokenEvent::Token {
                        id: completion_tokens,
                        text: text.to_string(),
                    });
                }
            }
        }
        let stats = GenerateStats::finish(
            prompt_tokens,
            completion_tokens,
            started,
            ttft,
            false,
            finish_reason,
        );
        let _ = tx.send(TokenEvent::Done { stats });
        Ok(())
    }

    fn prefix_cache_stats(&self) -> PrefixCacheStats {
        PrefixCacheStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_has_no_double_slash() {
        let backend = UpstreamBackend::new("http://127.0.0.1:18080/").unwrap();
        assert_eq!(backend.endpoint(), "http://127.0.0.1:18080/v1/completions");
        assert_eq!(backend.model, "local");
    }

    #[test]
    fn upstream_kind_controls_cache_prompt() {
        assert!(UpstreamKind::parse(Some("llama-server")).sends_cache_prompt());
        assert!(!UpstreamKind::parse(Some("tabbyapi")).sends_cache_prompt());
        assert!(!UpstreamKind::parse(Some("generic")).sends_cache_prompt());
        assert!(!UpstreamKind::parse(None).sends_cache_prompt());
        assert_eq!(UpstreamKind::parse(Some("TABBY")), UpstreamKind::TabbyApi);
    }
}
