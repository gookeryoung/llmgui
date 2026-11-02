use std::path::PathBuf;

/// Errors exposed by every llmgui crate. `code()` is a stable machine-readable
/// identifier used in OpenAI-style error bodies and CLI diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum LlmguiError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("http error: {0}")]
    Http(String),
    #[error("invalid model: {0}")]
    Model(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("busy: {0}")]
    Busy(String),
    #[error("insufficient memory: {0}")]
    InsufficientMemory(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("cancelled")]
    Cancelled,
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    Checksum {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("path is outside the allowed root: {0}")]
    PathEscape(PathBuf),
}

impl LlmguiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io_error",
            Self::Json(_) => "json_error",
            Self::Toml(_) => "config_error",
            Self::Http(_) => "http_error",
            Self::Model(_) => "invalid_model",
            Self::Download(_) => "download_failed",
            Self::Backend(_) => "backend_error",
            Self::Config(_) => "config_error",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Busy(_) => "busy",
            Self::InsufficientMemory(_) => "insufficient_memory",
            Self::Unsupported(_) => "unsupported",
            Self::Cancelled => "cancelled",
            Self::Checksum { .. } => "checksum_mismatch",
            Self::PathEscape(_) => "path_escape",
        }
    }
}

pub type Result<T> = std::result::Result<T, LlmguiError>;
