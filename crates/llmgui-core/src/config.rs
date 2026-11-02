use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{LlmguiError, Result};

/// Top-level config file (`<LLMGUI_HOME>/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub download: DownloadConfig,
    pub model: ModelConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    pub max_concurrent: usize,
    pub allow_remote: bool,
    pub gui_dir: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 11435,
            api_key: None,
            max_concurrent: 2,
            allow_remote: false,
            gui_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadConfig {
    /// Ordered source priority. Supported: `modelscope`, `hf-mirror`, `gitee-ai`.
    pub sources: Vec<String>,
    pub concurrency: usize,
    pub chunk_size_mb: u64,
    pub cache_dir: Option<PathBuf>,
    pub timeout_secs: u64,
    pub proxy: Option<String>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            sources: vec![
                "modelscope".to_string(),
                "hf-mirror".to_string(),
                "gitee-ai".to_string(),
            ],
            concurrency: 8,
            chunk_size_mb: 16,
            cache_dir: None,
            timeout_secs: 60,
            proxy: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub ctx: u32,
    pub kv_type: String,
    pub gpu_layers: i32,
    pub n_threads: Option<i32>,
    pub n_batch: u32,
    /// Additional directories scanned for models (external libraries).
    #[serde(default)]
    pub extra_dirs: Vec<PathBuf>,
    /// Unload an idle model after this many seconds (0 = never).
    #[serde(default)]
    pub idle_ttl_secs: u64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            ctx: 32768,
            kv_type: "q8_0".to_string(),
            gpu_layers: -1,
            n_threads: None,
            n_batch: 512,
            extra_dirs: Vec::new(),
            idle_ttl_secs: 0,
        }
    }
}

impl AppConfig {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| LlmguiError::Config(format!("serialize config: {e}")))?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Reject remote binds unless the operator explicitly enabled them.
    pub fn validate_bind(&self) -> Result<()> {
        if !self.server.allow_remote
            && self.server.host != "127.0.0.1"
            && self.server.host != "localhost"
            && self.server.host != "::1"
        {
            return Err(LlmguiError::Config(
                "remote bind requires server.allow_remote = true".to_string(),
            ));
        }
        Ok(())
    }
}
