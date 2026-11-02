//! Reference-counted garbage collection for the content-addressed cache.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use llmgui_core::Result;

use crate::cache::CasStore;
use crate::manifest::is_forbidden_file;

/// Reject files we refuse to fetch (pickle-style weights).
pub fn is_forbidden(path: &str) -> bool {
    is_forbidden_file(path)
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GcReport {
    pub scanned: usize,
    pub removed: usize,
    pub kept: usize,
    pub freed_bytes: u64,
}

/// Delete blobs that are not referenced by any manifest. Blobs younger than
/// `grace` are kept to avoid racing a download that has not yet written its
/// manifest.
pub fn gc_unreferenced(
    store: &CasStore,
    referenced: &BTreeSet<String>,
    dry_run: bool,
    grace: Duration,
) -> Result<GcReport> {
    let mut report = GcReport::default();
    let blobs = store.cache_root().join("blobs").join("sha256");
    if !blobs.exists() {
        return Ok(report);
    }
    let now = SystemTime::now();
    for prefix in std::fs::read_dir(&blobs)?.flatten() {
        let prefix_path = prefix.path();
        if !prefix_path.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&prefix_path)?.flatten() {
            let path = entry.path();
            let Some(hash) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if hash.len() != 64 {
                continue;
            }
            report.scanned += 1;
            if referenced.contains(hash) {
                report.kept += 1;
                continue;
            }
            let meta = entry.metadata()?;
            let age = meta
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .unwrap_or_default();
            if age < grace {
                report.kept += 1;
                continue;
            }
            if !dry_run {
                std::fs::remove_file(&path)?;
            }
            report.removed += 1;
            report.freed_bytes += meta.len();
        }
    }
    Ok(report)
}

/// Read every hash referenced by the manifests under `models_root`.
pub fn referenced_hashes(models_root: &Path) -> Result<BTreeSet<String>> {
    let mut refs = BTreeSet::new();
    if !models_root.exists() {
        return Ok(refs);
    }
    for entry in std::fs::read_dir(models_root)?.flatten() {
        let manifest = entry.path().join("manifest.json");
        if !manifest.exists() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(files) = value.get("files").and_then(|files| files.as_array()) {
            for file in files {
                if let Some(hash) = file.get("sha256").and_then(|hash| hash.as_str())
                    && hash.len() == 64
                {
                    refs.insert(hash.to_string());
                }
            }
        }
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gc_removes_only_unreferenced_blobs() {
        let root = tempfile::tempdir().unwrap();
        let store = CasStore::new(root.path()).unwrap();
        let keep = root.path().join("keep.bin");
        std::fs::write(&keep, b"keep").unwrap();
        let drop = root.path().join("drop.bin");
        std::fs::write(&drop, b"drop").unwrap();
        let keep_hash = "a".repeat(64);
        let drop_hash = "b".repeat(64);
        store.insert_blob(&keep_hash, &keep).await.unwrap();
        store.insert_blob(&drop_hash, &drop).await.unwrap();
        let refs = BTreeSet::from([keep_hash]);
        let report = gc_unreferenced(&store, &refs, false, Duration::ZERO).unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.kept, 1);
        assert!(store.blob_path(&"a".repeat(64)).unwrap().exists());
        assert!(!store.blob_path(&"b".repeat(64)).unwrap().exists());
    }

    #[test]
    fn referenced_hashes_reads_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("m");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::write(
            model.join("manifest.json"),
            serde_json::json!({"files":[{"sha256":"c".repeat(64)}]}).to_string(),
        )
        .unwrap();
        let refs = referenced_hashes(dir.path()).unwrap();
        assert!(refs.contains(&"c".repeat(64)));
    }
}
