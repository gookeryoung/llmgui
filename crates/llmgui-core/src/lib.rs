//! Core types shared by the llmgui runtime, downloader, server and CLI.

pub mod api;
pub mod config;
pub mod error;
pub mod estimate;
pub mod manifest;
pub mod paths;

pub use error::{LlmguiError, Result};

/// Runtime version reported by `/healthz` and CLI `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Return the current Unix timestamp in seconds.
pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Return the current RFC3339 timestamp.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
