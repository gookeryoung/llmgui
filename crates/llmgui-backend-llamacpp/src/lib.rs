//! llama.cpp backend. The `llama` feature is optional so that CI and the
//! mock-only path do not require a CUDA/MSVC toolchain.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use llmgui_backend_api::{
    Backend, BackendInfo, GenerateRequest, GenerateStats, LoadConfig, PrefixCacheStats, TokenEvent,
};
use llmgui_core::Result;
use tokio::sync::mpsc::UnboundedSender;

#[cfg(feature = "llama")]
mod real;

pub struct LlamaCppBackend {
    model_id: String,
    model_path: std::sync::Mutex<Option<PathBuf>>,
    #[cfg(feature = "llama")]
    inner: real::RealBackend,
}

impl LlamaCppBackend {
    pub fn new() -> Self {
        Self {
            model_id: "llama.cpp".to_string(),
            model_path: std::sync::Mutex::new(None),
            #[cfg(feature = "llama")]
            inner: real::RealBackend::new(),
        }
    }
}

impl Default for LlamaCppBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for LlamaCppBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            name: "llama.cpp".to_string(),
            model_id: self.model_id.clone(),
            model_path: self.model_path.lock().unwrap().clone(),
            n_ctx: 0,
            n_params: None,
            slots: 1,
        }
    }

    fn load(&self, model_path: &Path, config: &LoadConfig) -> Result<()> {
        *self.model_path.lock().unwrap() = Some(model_path.to_path_buf());
        #[cfg(feature = "llama")]
        {
            eprintln!("[llama] entering backend load for {}", model_path.display());
            self.inner.load(model_path, config)?;
            eprintln!("[llama] backend load returned");
            return Ok(());
        }
        #[cfg(not(feature = "llama"))]
        {
            let _ = config;
            Err(llmgui_core::LlmguiError::Unsupported(
                "llama.cpp backend is not enabled; rebuild with --features llama".to_string(),
            ))
        }
    }

    fn unload(&self) -> Result<()> {
        *self.model_path.lock().unwrap() = None;
        #[cfg(feature = "llama")]
        {
            self.inner.unload()?;
        }
        Ok(())
    }

    fn tokenize(&self, text: &str, add_special: bool) -> Result<Vec<u32>> {
        #[cfg(feature = "llama")]
        {
            return self.inner.tokenize(text, add_special);
        }
        #[cfg(not(feature = "llama"))]
        {
            let _ = (text, add_special);
            Err(llmgui_core::LlmguiError::Unsupported(
                "llama.cpp backend is not enabled".to_string(),
            ))
        }
    }

    fn generate(
        &self,
        request: GenerateRequest,
        cancel: Arc<AtomicBool>,
        tx: UnboundedSender<TokenEvent>,
    ) -> Result<()> {
        #[cfg(feature = "llama")]
        {
            return self.inner.generate(request, cancel, tx);
        }
        #[cfg(not(feature = "llama"))]
        {
            let _ = (request, cancel, tx);
            Err(llmgui_core::LlmguiError::Unsupported(
                "llama.cpp backend is not enabled".to_string(),
            ))
        }
    }

    fn prefix_cache_stats(&self) -> PrefixCacheStats {
        #[cfg(feature = "llama")]
        {
            return self.inner.prefix_cache_stats();
        }
        #[cfg(not(feature = "llama"))]
        PrefixCacheStats::default()
    }
}

// Keep the type usable in tests without the feature.
#[allow(dead_code)]
fn _assert_stats(_: GenerateStats) {}
