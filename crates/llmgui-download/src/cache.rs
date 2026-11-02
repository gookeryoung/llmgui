//! Content-addressed storage (CAS) and snapshot materialisation.
//!
//! Layout (see ADR §7.9 / research/download.md §4.3):
//!
//! ```text
//! <cache>/blobs/sha256/<ab>/<hex>                    verified content blobs
//! <cache>/snapshots/<model-id>/<revision>/<path>     hard links (copy across volumes)
//! <cache>/refs/<source>/<owner>--<name>@<rev>.json   source manifest snapshot
//! <cache>/state/<key>.part                           resumable partial download
//! <cache>/state/<key>.state.json                     resumable state (chunk bitmap)
//! <models-root>/<model-id>/<path>                    externally visible model tree
//! <models-root>/<model-id>/manifest.json             model manifest
//! ```

use std::path::{Path, PathBuf};

use serde::Serialize;

use llmgui_core::manifest::ModelManifest;
use llmgui_core::paths::ensure_within;
use llmgui_core::{LlmguiError, Result};

/// Build a stable, path-safe model id from a repository id such as
/// `Qwen/Qwen2.5-7B-Instruct-GGUF` -> `Qwen--Qwen2.5-7B-Instruct-GGUF`.
pub fn model_id_for_repo(repo: &str) -> String {
    let mut id = String::new();
    for (index, part) in repo
        .trim()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .enumerate()
    {
        if index > 0 {
            id.push_str("--");
        }
        for ch in part.chars() {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                id.push(ch);
            } else {
                id.push('-');
            }
        }
    }
    if id.is_empty() || id == "." || id == ".." || id.chars().all(|ch| ch == '-') {
        "model".to_string()
    } else {
        id
    }
}

/// Validate a SHA256 digest and return its canonical lowercase form.
pub fn canonical_sha256(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(LlmguiError::Download(format!(
            "invalid sha256 digest: {value:?} (expected 64 hex characters)"
        )));
    }
    Ok(value.to_ascii_lowercase())
}

/// `true` when `value` looks like a SHA256 digest (as opposed to a git SHA1 oid).
pub fn looks_like_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Content-addressed store rooted at `<cache>` plus the externally visible
/// model root (`<LLMGUI_HOME>/models` by default).
#[derive(Debug, Clone)]
pub struct CasStore {
    cache_root: PathBuf,
    models_root: PathBuf,
}

impl CasStore {
    /// Create a store whose blobs live under `cache_root` and whose model trees
    /// live under [`llmgui_core::paths::models_dir`].
    pub fn new(cache_root: impl Into<PathBuf>) -> Result<Self> {
        Self::with_roots(cache_root, llmgui_core::paths::models_dir())
    }

    /// Create a store with explicit roots. Primarily useful for tests and for
    /// future `--models-dir` support.
    pub fn with_roots(
        cache_root: impl Into<PathBuf>,
        models_root: impl Into<PathBuf>,
    ) -> Result<Self> {
        let store = Self {
            cache_root: cache_root.into(),
            models_root: models_root.into(),
        };
        for dir in [
            store.cache_root.join("blobs/sha256"),
            store.cache_root.join("snapshots"),
            store.cache_root.join("refs"),
            store.cache_root.join("state"),
            store.models_root.clone(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        // Canonicalise roots once (Windows may return `\\?\`-prefixed paths);
        // `ensure_within` canonicalises its root too, so both sides must agree.
        Ok(Self {
            cache_root: store
                .cache_root
                .canonicalize()
                .unwrap_or(store.cache_root.clone()),
            models_root: store
                .models_root
                .canonicalize()
                .unwrap_or(store.models_root.clone()),
        })
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn models_root(&self) -> &Path {
        &self.models_root
    }

    /// `<cache>/blobs/sha256/<ab>/<hex>`.
    pub fn blob_path(&self, sha256: &str) -> Result<PathBuf> {
        let sha = canonical_sha256(sha256)?;
        let root = self.cache_root.join("blobs").join("sha256");
        ensure_within(&root, &root.join(&sha[..2]).join(&sha))
    }

    pub fn has_blob(&self, sha256: &str) -> Result<bool> {
        Ok(self.blob_path(sha256)?.is_file())
    }

    /// Move (or, across volumes, copy) a verified file into the blob store.
    /// Returns the blob path. Existing blobs win and `src` is removed.
    pub async fn insert_blob(&self, sha256: &str, src: &Path) -> Result<PathBuf> {
        let dest = self.blob_path(sha256)?;
        if tokio::fs::try_exists(&dest).await? {
            remove_if_exists(src).await?;
            return Ok(dest);
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match tokio::fs::rename(src, &dest).await {
            Ok(()) => Ok(dest),
            Err(_) => {
                // Cross-volume (or otherwise unrenameable): copy then rename.
                let tmp = dest.with_extension("tmp");
                tokio::fs::copy(src, &tmp).await?;
                tokio::fs::rename(&tmp, &dest).await?;
                remove_if_exists(src).await?;
                Ok(dest)
            }
        }
    }

    /// `<cache>/snapshots/<model-id>/<revision>`.
    pub fn snapshot_dir(&self, model_id: &str, revision: &str) -> Result<PathBuf> {
        let root = self.cache_root.join("snapshots");
        let model = safe_component(model_id, "model id")?;
        let rev = safe_component(revision, "revision")?;
        ensure_within(&root, &root.join(model).join(rev))
    }

    /// Hard-link a blob into the cache snapshot tree (copy across volumes).
    pub async fn link_into_snapshot(
        &self,
        model_id: &str,
        revision: &str,
        relative_path: &str,
        blob: &Path,
    ) -> Result<PathBuf> {
        let root = self.snapshot_dir(model_id, revision)?;
        let dest = ensure_within(&root, Path::new(relative_path))?;
        link_or_copy(blob, &dest).await?;
        Ok(dest)
    }

    /// `<models-root>/<model-id>`.
    pub fn model_dir(&self, model_id: &str) -> Result<PathBuf> {
        let model = safe_component(model_id, "model id")?;
        ensure_within(&self.models_root, &self.models_root.join(model))
    }

    /// Hard-link a blob into the externally visible model tree.
    pub async fn link_into_model(
        &self,
        model_id: &str,
        relative_path: &str,
        blob: &Path,
    ) -> Result<PathBuf> {
        let root = self.model_dir(model_id)?;
        tokio::fs::create_dir_all(&root).await?;
        let dest = ensure_within(&root, Path::new(relative_path))?;
        link_or_copy(blob, &dest).await?;
        Ok(dest)
    }

    pub fn manifest_path(&self, model_id: &str) -> Result<PathBuf> {
        Ok(self.model_dir(model_id)?.join("manifest.json"))
    }

    /// `<cache>/refs/<source>/<owner>--<name>@<rev>.json`.
    pub fn ref_path(&self, source: &str, repo: &str, revision: &str) -> Result<PathBuf> {
        let root = self.cache_root.join("refs");
        let source = safe_component(source, "source name")?;
        let model = model_id_for_repo(repo);
        let rev = safe_component(revision, "revision")?;
        let name = format!("{model}@{rev}.json");
        ensure_within(&root, &root.join(source).join(name))
    }

    /// Persist a source manifest snapshot under `refs/`.
    pub async fn write_ref(
        &self,
        source: &str,
        repo: &str,
        revision: &str,
        manifest: &ModelManifest,
    ) -> Result<PathBuf> {
        let path = self.ref_path(source, repo, revision)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(manifest)?).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(path)
    }

    /// `<cache>/state/<key>.part`.
    pub fn part_path(&self, key: &str) -> Result<PathBuf> {
        Ok(self.state_root_path(key)?.with_extension("part"))
    }

    /// `<cache>/state/<key>.state.json`.
    pub fn state_path(&self, key: &str) -> Result<PathBuf> {
        let root = self.cache_root.join("state");
        let key = safe_component(key, "download key")?;
        ensure_within(&root, &root.join(format!("{key}.state.json")))
    }

    fn state_root_path(&self, key: &str) -> Result<PathBuf> {
        let root = self.cache_root.join("state");
        let key = safe_component(key, "download key")?;
        ensure_within(&root, &root.join(key))
    }
}

fn safe_component(value: &str, what: &str) -> Result<String> {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    let traverses = value
        .split(['/', '\\'])
        .any(|component| component == "." || component == "..");
    if out.is_empty() || traverses || out.chars().all(|ch| ch == '-') {
        return Err(LlmguiError::Download(format!("unsafe {what}: {value:?}")));
    }
    Ok(out)
}

async fn remove_if_exists(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

async fn link_or_copy(src: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Link/copy to a sibling temporary name, then atomically replace `dest`.
    // This keeps a moved revision from leaving a stale file behind.
    let tmp = dest.with_file_name(format!(
        "{}.llmgui-tmp",
        dest.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("link")
    ));
    remove_if_exists(&tmp).await?;
    match tokio::fs::hard_link(src, &tmp).await {
        Ok(()) => {}
        Err(_) => {
            tokio::fs::copy(src, &tmp).await?;
        }
    }
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

/// Small helper for atomic JSON writes.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    fn store() -> (tempfile::TempDir, tempfile::TempDir, CasStore) {
        let cache = tempfile::tempdir().unwrap();
        let models = tempfile::tempdir().unwrap();
        let store = CasStore::with_roots(cache.path(), models.path()).unwrap();
        (cache, models, store)
    }

    #[test]
    fn model_id_is_path_safe() {
        assert_eq!(
            model_id_for_repo("Qwen/Qwen2.5-7B-Instruct-GGUF"),
            "Qwen--Qwen2.5-7B-Instruct-GGUF"
        );
        assert_eq!(model_id_for_repo("../../etc"), "..--..--etc");
        assert!(!model_id_for_repo("../../etc").contains('/'));
        assert_eq!(model_id_for_repo("/"), "model");
    }

    #[test]
    fn sha256_validation() {
        assert!(canonical_sha256(&"a".repeat(64)).is_ok());
        assert!(canonical_sha256(&"A".repeat(64)).is_ok());
        assert!(canonical_sha256("abc").is_err());
        assert!(canonical_sha256(&"../".repeat(32)).is_err());
        assert!(!looks_like_sha256(&"a".repeat(40)));
    }

    #[test]
    fn blob_layout_and_escape_guard() {
        let (_cache, _models, store) = store();
        let sha = "ab".repeat(32);
        let path = store.blob_path(&sha).unwrap();
        assert!(path.ends_with(format!("blobs/sha256/ab/{sha}")));
        assert!(store.blob_path("../../evil").is_err());
        assert!(store.part_path("../escape").is_err());
        assert!(store.state_path("..").is_err());
    }

    #[tokio::test]
    async fn insert_blob_is_idempotent() {
        let (cache, _models, store) = store();
        let src = cache.path().join("payload");
        std::fs::write(&src, b"hello").unwrap();
        let sha = hex::encode(Sha256::digest(b"hello"));
        let blob = store.insert_blob(&sha, &src).await.unwrap();
        assert!(blob.is_file());
        assert!(!src.exists());
        assert!(store.has_blob(&sha).unwrap());

        let src2 = cache.path().join("payload2");
        std::fs::write(&src2, b"hello").unwrap();
        let blob2 = store.insert_blob(&sha, &src2).await.unwrap();
        assert_eq!(blob, blob2);
        assert!(!src2.exists());
    }

    #[tokio::test]
    async fn links_into_snapshot_and_model() {
        let (cache, _models, store) = store();
        let blob = cache.path().join("blob");
        std::fs::write(&blob, b"weights").unwrap();
        let snapshot = store
            .link_into_snapshot("Qwen--X", "master", "sub/a.gguf", &blob)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&snapshot).unwrap(), b"weights");
        assert!(
            store
                .link_into_snapshot("Qwen--X", "master", "../evil", &blob)
                .await
                .is_err()
        );

        let model_file = store
            .link_into_model("Qwen--X", "sub/a.gguf", &blob)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&model_file).unwrap(), b"weights");
        assert!(model_file.starts_with(store.models_root()));
        assert!(
            store
                .manifest_path("Qwen--X")
                .unwrap()
                .ends_with("manifest.json")
        );
    }
}
