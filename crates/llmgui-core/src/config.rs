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
    pub catalog: CatalogConfig,
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

/// Remote curated catalog refresh policy (ADR 0005).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogConfig {
    /// Check the remote catalog automatically; `false` disables network access.
    pub auto_update: bool,
    /// Versioned TOML catalog URL; empty disables the remote source.
    pub url: String,
    /// Minimum interval between successful checks, in hours (1–168).
    pub ttl_hours: u64,
    pub timeout_secs: u64,
    pub max_bytes: u64,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            auto_update: true,
            url: "https://raw.githubusercontent.com/gookeryoung/llmgui/main/models/v0.toml"
                .to_string(),
            ttl_hours: 24,
            timeout_secs: 10,
            max_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Named load-profile preset. `Custom` keeps the explicit fields below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfile {
    #[default]
    Custom,
    Balanced,
    Turbo,
}

impl RuntimeProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "custom" => Some(Self::Custom),
            "balanced" => Some(Self::Balanced),
            "turbo" => Some(Self::Turbo),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Balanced => "balanced",
            Self::Turbo => "turbo",
        }
    }
}

/// Flash-attention policy. `Auto` lets llama.cpp enable FA when the KV cache
/// type requires it, which preserves the pre-profile behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FlashAttention {
    #[default]
    Auto,
    Enabled,
    Disabled,
}

/// Fully resolved load parameters after applying a [`RuntimeProfile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileParams {
    pub profile: RuntimeProfile,
    pub ctx: u32,
    pub kv_type: String,
    pub gpu_layers: i32,
    pub n_batch: u32,
    pub n_ubatch: u32,
    pub n_threads: Option<i32>,
    pub n_threads_batch: Option<i32>,
    pub flash_attn: FlashAttention,
    pub slots: usize,
    pub no_context_shift: bool,
}

impl RuntimeProfile {
    /// Expand this profile over `base`. Profiles override the explicit fields;
    /// callers that need per-request overrides apply them afterwards.
    pub fn expand(self, base: ProfileParams) -> ProfileParams {
        match self {
            Self::Custom => ProfileParams {
                profile: self,
                ..base
            },
            Self::Balanced => ProfileParams {
                profile: self,
                ctx: 32768,
                kv_type: "q8_0".to_string(),
                gpu_layers: -1,
                n_batch: 512,
                n_ubatch: 512,
                n_threads: None,
                n_threads_batch: None,
                flash_attn: FlashAttention::Auto,
                slots: base.slots,
                no_context_shift: true,
            },
            Self::Turbo => ProfileParams {
                profile: self,
                ctx: 8192,
                kv_type: "q4_0".to_string(),
                gpu_layers: 999,
                n_batch: 2048,
                n_ubatch: 512,
                n_threads: Some(8),
                n_threads_batch: Some(8),
                flash_attn: FlashAttention::Enabled,
                slots: base.slots,
                no_context_shift: true,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// Named load preset; `custom` keeps the explicit fields below.
    pub profile: RuntimeProfile,
    pub ctx: u32,
    pub kv_type: String,
    pub gpu_layers: i32,
    pub n_threads: Option<i32>,
    pub n_batch: u32,
    /// Micro-batch size; `None` keeps the historical `n_ubatch = n_batch`.
    pub n_ubatch: Option<u32>,
    pub n_threads_batch: Option<i32>,
    pub flash_attn: FlashAttention,
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
            profile: RuntimeProfile::Custom,
            ctx: 32768,
            kv_type: "q8_0".to_string(),
            gpu_layers: -1,
            n_threads: None,
            n_batch: 512,
            n_ubatch: None,
            n_threads_batch: None,
            flash_attn: FlashAttention::Auto,
            extra_dirs: Vec::new(),
            idle_ttl_secs: 0,
        }
    }
}

impl ModelConfig {
    /// Resolve the configured profile into concrete load parameters.
    pub fn profile_params(&self, slots: usize) -> ProfileParams {
        let base = ProfileParams {
            profile: self.profile,
            ctx: self.ctx,
            kv_type: self.kv_type.clone(),
            gpu_layers: self.gpu_layers,
            n_batch: self.n_batch,
            n_ubatch: self.n_ubatch.unwrap_or(self.n_batch),
            n_threads: self.n_threads,
            n_threads_batch: self.n_threads_batch,
            flash_attn: self.flash_attn,
            slots,
            no_context_shift: true,
        };
        self.profile.expand(base)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_parse_accepts_known_names() {
        assert_eq!(RuntimeProfile::parse("turbo"), Some(RuntimeProfile::Turbo));
        assert_eq!(
            RuntimeProfile::parse(" Balanced "),
            Some(RuntimeProfile::Balanced)
        );
        assert_eq!(
            RuntimeProfile::parse("custom"),
            Some(RuntimeProfile::Custom)
        );
        assert_eq!(RuntimeProfile::parse("fast"), None);
        assert_eq!(RuntimeProfile::default(), RuntimeProfile::Custom);
    }

    #[test]
    fn turbo_profile_expands_to_target_parameters() {
        let config = ModelConfig {
            profile: RuntimeProfile::Turbo,
            // Preset wins over explicit config-file fields.
            ctx: 16384,
            kv_type: "q8_0".to_string(),
            gpu_layers: 0,
            ..ModelConfig::default()
        };
        let params = config.profile_params(4);
        assert_eq!(params.ctx, 8192);
        assert_eq!(params.kv_type, "q4_0");
        assert_eq!(params.gpu_layers, 999);
        assert_eq!(params.n_batch, 2048);
        assert_eq!(params.n_ubatch, 512);
        assert_eq!(params.n_threads, Some(8));
        assert_eq!(params.n_threads_batch, Some(8));
        assert_eq!(params.flash_attn, FlashAttention::Enabled);
        assert_eq!(params.slots, 4);
        assert!(params.no_context_shift);
    }

    #[test]
    fn custom_profile_keeps_explicit_fields() {
        let config = ModelConfig {
            ctx: 4096,
            kv_type: "f16".to_string(),
            gpu_layers: 12,
            n_batch: 1024,
            n_ubatch: Some(256),
            n_threads_batch: Some(6),
            flash_attn: FlashAttention::Disabled,
            ..ModelConfig::default()
        };
        let params = config.profile_params(2);
        assert_eq!(params.ctx, 4096);
        assert_eq!(params.kv_type, "f16");
        assert_eq!(params.gpu_layers, 12);
        assert_eq!(params.n_batch, 1024);
        assert_eq!(params.n_ubatch, 256);
        assert_eq!(params.n_threads_batch, Some(6));
        assert_eq!(params.flash_attn, FlashAttention::Disabled);
        assert_eq!(params.slots, 2);
    }

    #[test]
    fn ubatch_defaults_to_batch_for_legacy_config() {
        let config = ModelConfig {
            n_batch: 768,
            ..ModelConfig::default()
        };
        assert_eq!(config.profile_params(1).n_ubatch, 768);
    }

    #[test]
    fn config_parses_profile_and_flash_attention() {
        let config: AppConfig = toml::from_str(
            r#"
            [model]
            profile = "turbo"
            flash_attn = "disabled"
            "#,
        )
        .unwrap();
        assert_eq!(config.model.profile, RuntimeProfile::Turbo);
        assert_eq!(config.model.flash_attn, FlashAttention::Disabled);

        let legacy: AppConfig = toml::from_str("[model]\nctx = 4096\n").unwrap();
        assert_eq!(legacy.model.profile, RuntimeProfile::Custom);
        assert_eq!(legacy.model.flash_attn, FlashAttention::Auto);
    }
}
