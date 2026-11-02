//! Real llama.cpp backend, enabled with `--features llama`.
//!
//! The model is loaded once and leaked to obtain a `'static` borrow so that
//! per-request contexts can be created without self-referential structs. Model
//! unloading is therefore deferred to process exit; the CLI supports one model
//! per process today (`hot-swap` is a P3 item).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use llama_cpp_2::LlamaStateSeqFlags;
use llama_cpp_2::SeqState;
use llama_cpp_2::context::params::{KvCacheType as LlamaKvType, LlamaContextParams};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llmgui_backend_api::{
    GenerateRequest, GenerateStats, LoadConfig, PrefixCacheStats, TokenEvent,
};
use llmgui_core::{LlmguiError, Result};
use tokio::sync::mpsc::UnboundedSender;

fn backend() -> &'static LlamaBackend {
    static BACKEND: OnceLock<&'static LlamaBackend> = OnceLock::new();
    BACKEND.get_or_init(|| {
        let backend = LlamaBackend::init().expect("failed to initialize llama.cpp backend");
        Box::leak(Box::new(backend))
    })
}

pub struct RealBackend {
    model: Mutex<Option<&'static LlamaModel>>,
    model_path: Mutex<Option<std::path::PathBuf>>,
    load_config: Mutex<LoadConfig>,
    prefix_cache: Mutex<HashMap<u64, SeqState>>,
    prefix_hits: AtomicU64,
    prefix_misses: AtomicU64,
    prefix_saved_tokens: AtomicU64,
}

impl RealBackend {
    pub fn new() -> Self {
        Self {
            model: Mutex::new(None),
            model_path: Mutex::new(None),
            load_config: Mutex::new(LoadConfig::default()),
            prefix_cache: Mutex::new(HashMap::new()),
            prefix_hits: AtomicU64::new(0),
            prefix_misses: AtomicU64::new(0),
            prefix_saved_tokens: AtomicU64::new(0),
        }
    }

    pub fn load(&self, model_path: &Path, config: &LoadConfig) -> Result<()> {
        if !model_path.exists() {
            return Err(LlmguiError::NotFound(model_path.display().to_string()));
        }
        let n_gpu_layers = if config.n_gpu_layers < 0 {
            // CPU-safe default. A CUDA-enabled build can opt into GPU layers
            // explicitly with `--gpu-layers` in a later phase.
            0
        } else {
            config.n_gpu_layers as u32
        };
        let params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);
        let model = LlamaModel::load_from_file(backend(), model_path, &params)
            .map_err(|e| LlmguiError::Model(format!("load {}: {e}", model_path.display())))?;
        let leaked: &'static LlamaModel = Box::leak(Box::new(model));
        *self.model.lock().unwrap() = Some(leaked);
        *self.model_path.lock().unwrap() = Some(model_path.to_path_buf());
        *self.load_config.lock().unwrap() = config.clone();
        self.prefix_cache.lock().unwrap().clear();
        Ok(())
    }

    pub fn unload(&self) -> Result<()> {
        *self.model.lock().unwrap() = None;
        *self.model_path.lock().unwrap() = None;
        self.prefix_cache.lock().unwrap().clear();
        Ok(())
    }

    fn context_params(config: &LoadConfig) -> LlamaContextParams {
        let kv = match config.kv_type.to_ascii_lowercase().as_str() {
            "q4_0" => LlamaKvType::Q4_0,
            "q8_0" | "q8" => LlamaKvType::Q8_0,
            _ => LlamaKvType::F16,
        };
        let mut params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(config.n_ctx.max(512)))
            .with_n_batch(config.n_batch.max(32))
            .with_n_ubatch(config.n_batch.max(32))
            .with_n_seq_max(1)
            .with_type_k(kv)
            .with_type_v(kv);
        if let Some(threads) = config.n_threads {
            params = params.with_n_threads(threads);
        }
        params
    }

    pub fn tokenize(&self, text: &str, add_special: bool) -> Result<Vec<u32>> {
        let guard = self.model.lock().unwrap();
        let model = guard
            .as_ref()
            .ok_or_else(|| LlmguiError::Backend("model not loaded".to_string()))?;
        Ok(model
            .vocab()
            .tokenize(text.as_bytes(), add_special, true)
            .into_iter()
            .map(|t| t.0 as u32)
            .collect())
    }

    pub fn generate(
        &self,
        request: GenerateRequest,
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<TokenEvent>,
    ) -> Result<()> {
        let model = {
            let guard = self.model.lock().unwrap();
            *guard
        };
        let model = model.ok_or_else(|| LlmguiError::Backend("model not loaded".to_string()))?;
        let config = self.load_config.lock().unwrap().clone();
        let mut ctx = model
            .new_context(backend(), Self::context_params(&config))
            .map_err(|e| LlmguiError::Backend(format!("create context: {e}")))?;

        let started = Instant::now();
        let tokens = model
            .vocab()
            .tokenize(request.prompt.as_bytes(), true, true);
        if tokens.is_empty() {
            return Err(LlmguiError::Config("prompt tokenized to empty".to_string()));
        }
        let n_ctx = ctx.n_ctx() as usize;
        if tokens.len() >= n_ctx {
            return Err(LlmguiError::Config(format!(
                "prompt has {} tokens but n_ctx is {n_ctx}",
                tokens.len()
            )));
        }
        let prompt_tokens = tokens.len() as u32;
        let mut batch = LlamaBatch::new(ctx.n_batch() as usize, 1);
        let cache_key = hash_tokens(&tokens);
        let cached = {
            let cache = self.prefix_cache.lock().unwrap();
            cache.get(&cache_key).cloned()
        };
        let mut prefix_hit = false;
        let mut position = 0i32;
        if let Some(state) = cached
            && ctx.state_seq_set(&state, 0).is_ok()
        {
            prefix_hit = true;
            position = tokens.len() as i32;
            self.prefix_hits.fetch_add(1, Ordering::Relaxed);
            self.prefix_saved_tokens
                .fetch_add(u64::from(prompt_tokens), Ordering::Relaxed);
        }
        if !prefix_hit {
            let mut offset = 0usize;
            while offset < tokens.len() {
                let end = (offset + ctx.n_batch() as usize).min(tokens.len());
                batch.clear();
                for (index, token) in tokens[offset..end].iter().enumerate() {
                    batch
                        .add(
                            *token,
                            position + index as i32,
                            &[0],
                            end == tokens.len() && index == end - offset - 1,
                        )
                        .map_err(|e| LlmguiError::Backend(format!("batch add: {e}")))?;
                }
                ctx.decode(&mut batch)
                    .map_err(|e| LlmguiError::Backend(format!("prefill decode: {e}")))?;
                position += (end - offset) as i32;
                offset = end;
            }
            if let Ok(state) = ctx.state_seq_get(0, LlamaStateSeqFlags::empty()) {
                let mut cache = self.prefix_cache.lock().unwrap();
                if cache.len() >= 8 {
                    cache.clear();
                }
                cache.insert(cache_key, state);
            }
            self.prefix_misses.fetch_add(1, Ordering::Relaxed);
        }

        let mut sampler = if request.temperature <= 0.0 {
            LlamaSampler::greedy()
        } else {
            LlamaSampler::chain_simple([
                LlamaSampler::temp(request.temperature.max(0.01)),
                LlamaSampler::top_p(request.top_p.clamp(0.01, 1.0), 1),
                LlamaSampler::dist(request.seed.unwrap_or(42) as u32),
            ])
        };

        let mut token = sampler.sample(&ctx, -1);
        let mut decoder = Utf8Decoder::default();
        let mut completion_tokens = 0u32;
        let mut ttft = None;
        let finish_reason = loop {
            if cancel.load(Ordering::Relaxed) {
                break "cancelled".to_string();
            }
            if model.vocab().is_eog(token) {
                break "stop".to_string();
            }
            if completion_tokens >= request.max_tokens {
                break "length".to_string();
            }
            let piece = model.vocab().token_to_piece(token, false, None);
            if let Some(text) = decoder.push(&piece) {
                if !text.is_empty() {
                    if ttft.is_none() {
                        ttft = Some(started.elapsed());
                    }
                    let _ = tx.send(TokenEvent::Token {
                        id: token.0 as u32,
                        text,
                    });
                }
            }
            completion_tokens += 1;
            if position as usize >= n_ctx.saturating_sub(1) {
                break "length".to_string();
            }
            batch.clear();
            batch
                .add(token, position, &[0], true)
                .map_err(|e| LlmguiError::Backend(format!("batch add: {e}")))?;
            ctx.decode(&mut batch)
                .map_err(|e| LlmguiError::Backend(format!("decode: {e}")))?;
            position += 1;
            token = sampler.sample(&ctx, -1);
        };
        let _ = tx.send(TokenEvent::Done {
            stats: GenerateStats::finish(
                prompt_tokens,
                completion_tokens,
                started,
                ttft,
                prefix_hit,
                finish_reason,
            ),
        });
        Ok(())
    }

    pub fn prefix_cache_stats(&self) -> PrefixCacheStats {
        let entries = self.prefix_cache.lock().unwrap().len();
        PrefixCacheStats {
            hits: self.prefix_hits.load(Ordering::Relaxed),
            misses: self.prefix_misses.load(Ordering::Relaxed),
            saved_prompt_tokens: self.prefix_saved_tokens.load(Ordering::Relaxed),
            entries,
        }
    }
}

impl Default for RealBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_tokens(tokens: &[LlamaToken]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for token in tokens {
        token.0.hash(&mut hasher);
    }
    hasher.finish()
}

/// Decode a byte stream that may split UTF-8 code points across tokens.
#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(text) => {
                let text = text.to_string();
                self.pending.clear();
                Some(text)
            }
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                let text = String::from_utf8_lossy(&self.pending[..valid]).to_string();
                self.pending.drain(..valid);
                if text.is_empty() { None } else { Some(text) }
            }
            Err(_) => {
                let text = String::from_utf8_lossy(&self.pending).to_string();
                self.pending.clear();
                Some(text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Utf8Decoder, hash_tokens};
    use llama_cpp_2::token::LlamaToken;

    #[test]
    fn prefix_hash_is_stable_and_order_sensitive() {
        let a = [LlamaToken(1), LlamaToken(2), LlamaToken(3)];
        let b = [LlamaToken(1), LlamaToken(2), LlamaToken(4)];
        assert_eq!(hash_tokens(&a), hash_tokens(&a));
        assert_ne!(hash_tokens(&a), hash_tokens(&b));
    }

    #[test]
    fn utf8_split_is_reassembled() {
        let mut decoder = Utf8Decoder::default();
        let bytes = "你好".as_bytes();
        assert!(decoder.push(&bytes[..1]).is_none());
        assert_eq!(decoder.push(&bytes[1..4]).as_deref(), Some("你"));
        assert_eq!(decoder.push(&bytes[4..]).as_deref(), Some("好"));
    }
}
