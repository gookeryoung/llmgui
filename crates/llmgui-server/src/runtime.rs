//! Runtime model lifecycle: one active model, drained switches, JIT loading and
//! idle unload. The backend object stays alive across switches; `load` performs
//! the actual weight swap inside the backend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use llmgui_backend_api::{Backend, LoadConfig};
use llmgui_core::config::RuntimeProfile;
use llmgui_core::estimate::KvCacheType;
use llmgui_core::{LlmguiError, Result};
use llmgui_models::GgufInfo;
use llmgui_models::library::{LoadDefaults, ModelEntry};
use tokio::sync::{Mutex, Notify, RwLock};

fn now() -> u64 {
    llmgui_core::now_unix().max(0) as u64
}

#[derive(Debug, Clone)]
pub struct LoadedModel {
    pub id: String,
    pub alias: Option<String>,
    pub display_name: String,
    pub path: std::path::PathBuf,
    pub profile: RuntimeProfile,
    pub ctx: u32,
    pub kv: String,
    pub slots: usize,
    pub gpu_layers: i32,
    pub ttl_secs: u64,
    pub loaded_at: u64,
    pub last_used: Arc<AtomicU64>,
    load_config: LoadConfig,
}

impl LoadedModel {
    pub fn touch(&self) {
        self.last_used.store(now(), Ordering::Relaxed);
    }
}

pub struct RequestGuard {
    runtime: Arc<Runtime>,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.runtime.active.fetch_sub(1, Ordering::SeqCst);
        self.runtime.notify.notify_waiters();
    }
}

pub struct Runtime {
    backend: Arc<dyn Backend>,
    backend_name: String,
    loaded: RwLock<Option<Arc<LoadedModel>>>,
    active: AtomicUsize,
    draining: AtomicBool,
    notify: Notify,
    op_lock: Mutex<()>,
    vram_budget_bytes: u64,
}

impl Runtime {
    pub fn new(backend: Arc<dyn Backend>, backend_name: impl Into<String>) -> Self {
        let budget = std::env::var("LLMGUI_VRAM_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(32 * 1_000_000_000);
        Self {
            backend,
            backend_name: backend_name.into(),
            loaded: RwLock::new(None),
            active: AtomicUsize::new(0),
            draining: AtomicBool::new(false),
            notify: Notify::new(),
            op_lock: Mutex::new(()),
            vram_budget_bytes: budget,
        }
    }

    pub fn backend(&self) -> Arc<dyn Backend> {
        self.backend.clone()
    }

    pub fn backend_name(&self) -> &str {
        &self.backend_name
    }

    pub fn vram_budget_bytes(&self) -> u64 {
        self.vram_budget_bytes
    }

    /// Register weights that the backend already loaded at startup.
    pub async fn adopt_loaded(
        &self,
        id: impl Into<String>,
        display_name: impl Into<String>,
        path: std::path::PathBuf,
        defaults: &LoadDefaults,
    ) {
        let load_config = LoadConfig::from(defaults.profile_params());
        let loaded = Arc::new(LoadedModel {
            id: id.into(),
            alias: None,
            display_name: display_name.into(),
            path,
            profile: defaults.profile,
            ctx: load_config.n_ctx,
            kv: load_config.kv_type.clone(),
            slots: load_config.slots,
            gpu_layers: load_config.n_gpu_layers,
            ttl_secs: defaults.ttl_secs,
            loaded_at: now(),
            last_used: Arc::new(AtomicU64::new(now())),
            load_config,
        });
        *self.loaded.write().await = Some(loaded);
    }

    pub async fn loaded(&self) -> Option<Arc<LoadedModel>> {
        self.loaded.read().await.clone()
    }

    pub async fn loaded_id(&self) -> Option<String> {
        self.loaded().await.map(|model| model.id.clone())
    }

    /// Enter a generation request. Fails fast while a model swap is draining.
    pub fn begin_request(self: &Arc<Self>) -> Result<RequestGuard> {
        if self.draining.load(Ordering::SeqCst) {
            return Err(LlmguiError::Busy(
                "model is being loaded or unloaded".to_string(),
            ));
        }
        self.active.fetch_add(1, Ordering::SeqCst);
        if self.draining.load(Ordering::SeqCst) {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return Err(LlmguiError::Busy(
                "model is being loaded or unloaded".to_string(),
            ));
        }
        Ok(RequestGuard {
            runtime: self.clone(),
        })
    }

    pub fn active_requests(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Resolve a request's model to a loaded model, JIT-loading when installed.
    pub async fn ensure_loaded(
        self: &Arc<Self>,
        entry: &ModelEntry,
        defaults: LoadDefaults,
        vram_budget_bytes: u64,
    ) -> Result<Arc<LoadedModel>> {
        if let Some(loaded) = self.loaded().await
            && loaded.id == entry.id
        {
            loaded.touch();
            return Ok(loaded);
        }
        self.load(entry, defaults, vram_budget_bytes).await
    }

    pub async fn load(
        self: &Arc<Self>,
        entry: &ModelEntry,
        defaults: LoadDefaults,
        vram_budget_bytes: u64,
    ) -> Result<Arc<LoadedModel>> {
        let _op = self.op_lock.lock().await;
        let previous = self.loaded().await;
        if let Some(previous) = &previous
            && previous.id == entry.id
        {
            previous.touch();
            return Ok(previous.clone());
        }
        self.check_memory(entry, &defaults, vram_budget_bytes)?;
        self.begin_drain().await?;
        let result = self.load_backend(entry, &defaults, vram_budget_bytes).await;
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(error) => {
                // Best effort rollback to the previous weights.
                if let Some(previous) = &previous {
                    let rollback = self.load_backend_from(previous, vram_budget_bytes).await;
                    if let Err(rollback) = rollback {
                        tracing::error!(%rollback, "failed to restore previous model");
                    }
                }
                self.finish_drain();
                return Err(error);
            }
        };
        *self.loaded.write().await = Some(loaded.clone());
        self.finish_drain();
        tracing::info!(model = %entry.id, "model loaded");
        Ok(loaded)
    }

    async fn load_backend(
        &self,
        entry: &ModelEntry,
        defaults: &LoadDefaults,
        vram_budget_bytes: u64,
    ) -> Result<Arc<LoadedModel>> {
        let gguf = entry
            .files
            .iter()
            .find(|file| file.path.ends_with(".gguf"))
            .map(|file| entry.path.join(&file.path))
            .ok_or_else(|| LlmguiError::Model("model has no GGUF file".to_string()))?;
        let info = GgufInfo::open(&gguf)?;
        let config = LoadConfig::from(defaults.profile_params());
        let kv = KvCacheType::parse(&config.kv_type);
        let estimate = info.estimate(config.n_ctx, kv, config.slots as u32);
        if estimate.total_bytes > vram_budget_bytes {
            return Err(LlmguiError::InsufficientMemory(format!(
                "{} needs {} but the budget is {}; lower ctx, use q8_0/q4_0 KV or a smaller quant",
                entry.id,
                llmgui_core::estimate::human_bytes(estimate.total_bytes),
                llmgui_core::estimate::human_bytes(vram_budget_bytes)
            )));
        }
        tracing::info!(
            model = %entry.id,
            profile = defaults.profile.as_str(),
            n_ctx = config.n_ctx,
            kv = %config.kv_type,
            n_gpu_layers = config.n_gpu_layers,
            n_batch = config.n_batch,
            n_ubatch = config.n_ubatch,
            n_threads = ?config.n_threads,
            n_threads_batch = ?config.n_threads_batch,
            flash_attn = ?config.flash_attn,
            "loading model with effective parameters"
        );
        let backend = self.backend.clone();
        let path = gguf.clone();
        let backend_config = config.clone();
        tokio::task::spawn_blocking(move || backend.load(&path, &backend_config))
            .await
            .map_err(|error| LlmguiError::Backend(format!("load task failed: {error}")))??;
        Ok(Arc::new(LoadedModel {
            id: entry.id.clone(),
            alias: None,
            display_name: entry.display_name.clone(),
            path: gguf,
            profile: defaults.profile,
            ctx: config.n_ctx,
            kv: config.kv_type.clone(),
            slots: config.slots,
            gpu_layers: config.n_gpu_layers,
            ttl_secs: defaults.ttl_secs,
            loaded_at: now(),
            last_used: Arc::new(AtomicU64::new(now())),
            load_config: config,
        }))
    }

    async fn load_backend_from(&self, model: &LoadedModel, budget: u64) -> Result<()> {
        let config = model.load_config.clone();
        let backend = self.backend.clone();
        let path = model.path.clone();
        let _ = budget;
        tokio::task::spawn_blocking(move || backend.load(&path, &config))
            .await
            .map_err(|error| LlmguiError::Backend(format!("load task failed: {error}")))??;
        *self.loaded.write().await = Some(Arc::new(model.clone()));
        Ok(())
    }

    fn check_memory(&self, entry: &ModelEntry, defaults: &LoadDefaults, budget: u64) -> Result<()> {
        let gguf = entry
            .files
            .iter()
            .find(|file| file.path.ends_with(".gguf"))
            .map(|file| entry.path.join(&file.path))
            .ok_or_else(|| LlmguiError::Model("model has no GGUF file".to_string()))?;
        let info = GgufInfo::open(&gguf)?;
        let config = LoadConfig::from(defaults.profile_params());
        let estimate = info.estimate(
            config.n_ctx,
            KvCacheType::parse(&config.kv_type),
            config.slots as u32,
        );
        if estimate.total_bytes > budget {
            return Err(LlmguiError::InsufficientMemory(format!(
                "{} needs {} at ctx {} / {} KV / {} slot(s), budget {}",
                entry.id,
                llmgui_core::estimate::human_bytes(estimate.total_bytes),
                config.n_ctx,
                config.kv_type,
                config.slots,
                llmgui_core::estimate::human_bytes(budget)
            )));
        }
        Ok(())
    }

    async fn begin_drain(&self) -> Result<()> {
        self.draining.store(true, Ordering::SeqCst);
        let timeout = std::env::var("LLMGUI_DRAIN_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(30);
        let deadline = std::time::Instant::now() + Duration::from_secs(timeout);
        while self.active.load(Ordering::SeqCst) > 0 {
            if std::time::Instant::now() >= deadline {
                self.draining.store(false, Ordering::SeqCst);
                return Err(LlmguiError::Busy(format!(
                    "timed out waiting for {} in-flight request(s); retry or raise LLMGUI_DRAIN_TIMEOUT_SECS",
                    self.active.load(Ordering::SeqCst)
                )));
            }
            let _ = tokio::time::timeout(Duration::from_millis(100), self.notify.notified()).await;
        }
        Ok(())
    }

    fn finish_drain(&self) {
        self.draining.store(false, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub async fn unload(&self) -> Result<Option<String>> {
        let _op = self.op_lock.lock().await;
        let current = self.loaded().await;
        let Some(current) = current else {
            return Ok(None);
        };
        self.begin_drain().await?;
        let backend = self.backend.clone();
        let result = tokio::task::spawn_blocking(move || backend.unload())
            .await
            .map_err(|error| LlmguiError::Backend(format!("unload task failed: {error}")))?;
        self.finish_drain();
        result?;
        *self.loaded.write().await = None;
        tracing::info!(model = %current.id, "model unloaded");
        Ok(Some(current.id.clone()))
    }

    /// Unload when idle and the TTL has elapsed.
    pub async fn maybe_unload_idle(&self) -> Result<Option<String>> {
        let Some(model) = self.loaded().await else {
            return Ok(None);
        };
        if model.ttl_secs == 0 || self.active.load(Ordering::SeqCst) > 0 {
            return Ok(None);
        }
        let idle = now().saturating_sub(model.last_used.load(Ordering::Relaxed));
        if idle < model.ttl_secs {
            return Ok(None);
        }
        self.unload().await
    }
}

/// Convenience for tests: a runtime with a mock backend.
#[cfg(test)]
pub fn mock_runtime() -> Arc<Runtime> {
    Arc::new(Runtime::new(
        Arc::new(llmgui_backend_mock::MockBackend::default()),
        "mock",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmgui_models::library::{LibraryFile, ModelEntry, ModelKind};

    fn temp_entry() -> (tempfile::TempDir, ModelEntry) {
        let dir = tempfile::tempdir().unwrap();
        let gguf = dir.path().join("tiny.gguf");
        llmgui_models::write_test_gguf(&gguf, "qwen2", 4, 4, 2, 512).unwrap();
        let entry = ModelEntry {
            id: "tiny".into(),
            display_name: "tiny".into(),
            kind: ModelKind::Inference,
            path: dir.path().to_path_buf(),
            manifest_path: None,
            location: Default::default(),
            repo: None,
            source_kind: None,
            revision: None,
            quant: "Q4_K_M".into(),
            bits_per_weight: None,
            architecture: Some("qwen2".into()),
            context: Default::default(),
            license: None,
            size_bytes: std::fs::metadata(&gguf).map(|m| m.len()).unwrap_or(0),
            files: vec![LibraryFile {
                path: "tiny.gguf".into(),
                size: std::fs::metadata(&gguf).map(|m| m.len()).unwrap_or(0),
                sha256: None,
            }],
            installed_at: 0,
            verified: false,
            fit_24gb: true,
            fit_32gb: true,
            vram_bytes_32k_q8: None,
            derived_from: None,
            adapter_id: None,
        };
        (dir, entry)
    }

    #[tokio::test]
    async fn load_unload_and_ttl() {
        let runtime = mock_runtime();
        let (_dir, entry) = temp_entry();
        let defaults = LoadDefaults {
            ctx: 512,
            slots: 1,
            ..Default::default()
        };
        let loaded = runtime
            .load(&entry, defaults.clone(), 32_000_000_000)
            .await
            .unwrap();
        assert_eq!(loaded.id, "tiny");
        assert!(runtime.loaded_id().await.is_some());
        let requests = runtime.begin_request().unwrap();
        assert_eq!(runtime.active_requests(), 1);
        drop(requests);
        assert_eq!(runtime.active_requests(), 0);
        runtime.unload().await.unwrap();
        assert!(runtime.loaded_id().await.is_none());
    }

    #[tokio::test]
    async fn turbo_defaults_reach_backend() {
        let runtime = mock_runtime();
        let (_dir, entry) = temp_entry();
        let defaults = LoadDefaults {
            profile: RuntimeProfile::Turbo,
            ..LoadDefaults::default()
        };
        let loaded = runtime
            .load(&entry, defaults, 32_000_000_000)
            .await
            .unwrap();
        assert_eq!(loaded.profile, RuntimeProfile::Turbo);
        assert_eq!(loaded.ctx, 8192);
        assert_eq!(loaded.kv, "q4_0");
        assert_eq!(loaded.gpu_layers, 999);
        assert_eq!(loaded.load_config.n_batch, 2048);
        assert_eq!(loaded.load_config.n_ubatch, 512);
    }

    #[tokio::test]
    async fn memory_gate_rejects_oversized_model() {
        let runtime = mock_runtime();
        let (_dir, entry) = temp_entry();
        let error = runtime
            .load(&entry, LoadDefaults::default(), 1_000)
            .await
            .unwrap_err();
        assert!(matches!(error, LlmguiError::InsufficientMemory(_)));
        assert!(runtime.loaded_id().await.is_none());
    }
}
