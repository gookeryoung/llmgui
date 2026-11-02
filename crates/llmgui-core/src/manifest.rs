use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{LlmguiError, Result};

pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    pub schema: u32,
    pub id: String,
    pub source: ModelSource,
    #[serde(default)]
    pub architecture: Option<String>,
    pub quant: QuantInfo,
    #[serde(default)]
    pub context: ContextInfo,
    #[serde(default)]
    pub files: Vec<ModelFile>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub chat_template: Option<ChatTemplateInfo>,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelSource {
    #[serde(rename = "type")]
    pub kind: String,
    pub repo: String,
    #[serde(default = "default_revision")]
    pub revision: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantInfo {
    pub format: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub bits_per_weight: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextInfo {
    #[serde(default = "default_context")]
    pub train: u32,
    #[serde(default = "default_context")]
    pub max: u32,
}

impl Default for ContextInfo {
    fn default() -> Self {
        Self {
            train: default_context(),
            max: default_context(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatTemplateInfo {
    pub source: String,
    #[serde(default)]
    pub version: u32,
}

fn default_revision() -> String {
    "master".to_string()
}

fn default_context() -> u32 {
    32768
}

impl ModelManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(LlmguiError::Model(format!(
                "unsupported manifest schema {}, expected {}",
                self.schema, MANIFEST_SCHEMA
            )));
        }
        if self.id.trim().is_empty() {
            return Err(LlmguiError::Model("manifest id is empty".to_string()));
        }
        for file in &self.files {
            if file.path.starts_with('/')
                || file.path.contains("..")
                || file.path.contains('\\')
                || file.path.contains(':')
            {
                return Err(LlmguiError::Model(format!(
                    "unsafe model file path: {}",
                    file.path
                )));
            }
        }
        Ok(())
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ModelManifest {
        ModelManifest {
            schema: 1,
            id: "qwen".into(),
            source: ModelSource::default(),
            architecture: Some("qwen2".into()),
            quant: QuantInfo {
                format: "gguf".into(),
                kind: "Q4_K_M".into(),
                bits_per_weight: Some(4.89),
            },
            context: ContextInfo::default(),
            files: vec![ModelFile {
                path: "a.gguf".into(),
                size: 10,
                sha256: "0".repeat(64),
            }],
            license: Some("Apache-2.0".into()),
            chat_template: None,
            created_at: "2026-11-02T00:00:00Z".into(),
        }
    }

    #[test]
    fn rejects_unsafe_paths() {
        let mut manifest = sample();
        manifest.files[0].path = "../escape.gguf".into();
        assert!(manifest.validate().is_err());
        manifest.files[0].path = "C:\\escape.gguf".into();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn roundtrips_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let manifest = sample();
        manifest.save(&path).unwrap();
        let loaded = ModelManifest::load(&path).unwrap();
        assert_eq!(loaded.id, manifest.id);
        assert_eq!(loaded.total_size(), 10);
    }
}
