//! Model download, mirror fallback, SHA256 verification and CAS cache.
//!
//! Responsibilities:
//! - source adapters ([`source`]): ModelScope REST + HF-compatible endpoints;
//! - chunked/resumable downloader ([`downloader`]): `Range` requests, `.part`
//!   state, retry, single-stream fallback, streaming SHA256;
//! - content-addressed storage ([`cache`]): blobs, cache snapshots, `models/`
//!   trees and manifests;
//! - manifest construction ([`manifest`]): id/quant inference.
//!
//! [`pull`] intentionally accepts exactly one source; automatic source
//! selection / fallback is the CLI's responsibility.

use std::path::PathBuf;

use llmgui_core::manifest::{ModelFile, ModelManifest};
use llmgui_core::{LlmguiError, Result};

pub mod cache;
pub mod downloader;
pub mod manifest;
pub mod source;

#[cfg(test)]
mod test_support;

pub use cache::{CasStore, model_id_for_repo};
pub use downloader::{DEFAULT_CHUNK_SIZE, DEFAULT_CONCURRENCY, DownloadedFile, Downloader};
pub use manifest::{infer_format, infer_quant};
pub use source::{
    ContentRange, DEFAULT_REVISION, FetchedChunk, HF_MIRROR_BASE, HfSource, MODEL_SCOPE_BASE,
    ModelScopeSource, RemoteFile, Source,
};

/// Options for one `pull` invocation.
#[derive(Debug, Clone, Default)]
pub struct DownloadOptions {
    /// Revision (branch/tag/commit); empty means [`DEFAULT_REVISION`].
    pub revision: String,
    /// Optional single file (or shard-name fragment) to download.
    pub file: Option<String>,
    /// Concurrent range requests per file (`0` -> [`DEFAULT_CONCURRENCY`]).
    pub concurrency: usize,
    /// Range size in bytes (`0` -> [`DEFAULT_CHUNK_SIZE`]).
    pub chunk_size: u64,
    /// Cache root override; defaults to [`llmgui_core::paths::cache_dir`].
    pub cache_dir: Option<PathBuf>,
}

/// Download every file of `repo` and return a validated manifest.
///
/// The cache root is `options.cache_dir` or [`llmgui_core::paths::cache_dir`];
/// the externally visible model tree is written under
/// [`llmgui_core::paths::models_dir`] (`<id>/manifest.json` + hard-linked or
/// copied files).
pub async fn pull(
    source: &dyn Source,
    repo: &str,
    options: &DownloadOptions,
    progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<ModelManifest> {
    let cache_root = options
        .cache_dir
        .clone()
        .unwrap_or_else(llmgui_core::paths::cache_dir);
    let store = CasStore::new(cache_root)?;
    pull_with_store(&store, source, repo, options, progress).await
}

/// [`pull`] with an explicit [`CasStore`] (used by tests and future callers
/// that manage their own cache/model roots).
pub async fn pull_with_store(
    store: &CasStore,
    source: &dyn Source,
    repo: &str,
    options: &DownloadOptions,
    progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<ModelManifest> {
    let revision = source::normalize_revision(&options.revision);
    let listed = source.list(repo, &revision).await?;
    let selected = select_files(listed, options.file.as_deref())?;
    if selected.is_empty() {
        return Err(LlmguiError::NotFound(format!(
            "repository {repo}@{revision} has no downloadable files"
        )));
    }
    for file in &selected {
        source::validate_relative_path(&file.path)?;
    }

    let model_id = cache::model_id_for_repo(repo);
    let total: u64 = selected.iter().map(|file| file.size).sum();
    report(&progress, 0, total);

    let downloader = Downloader::new(source, store, options.concurrency, options.chunk_size);
    let mut downloaded = Vec::with_capacity(selected.len());
    let mut completed = 0u64;
    for file in &selected {
        let base = completed;
        let file_size = file.size;
        let result = downloader
            .download(repo, &revision, file, |file_done| {
                report(&progress, base + file_done.min(file_size), total);
            })
            .await?;
        completed += file.size;
        downloaded.push((file.clone(), result));
    }

    let mut model_files = Vec::with_capacity(downloaded.len());
    for (file, result) in &downloaded {
        source::validate_relative_path(&file.path)?;
        store
            .link_into_snapshot(&model_id, &revision, &file.path, &result.path)
            .await?;
        store
            .link_into_model(&model_id, &file.path, &result.path)
            .await?;
        model_files.push(ModelFile {
            path: file.path.clone(),
            size: file.size,
            sha256: result.sha256.clone(),
        });
    }

    let manifest = manifest::build_manifest(
        source.name(),
        repo,
        &revision,
        &model_id,
        model_files,
        &selected,
    );
    manifest.save(&store.manifest_path(&model_id)?)?;
    store
        .write_ref(source.name(), repo, &revision, &manifest)
        .await?;
    report(&progress, total, total);
    Ok(manifest)
}

fn report(progress: &Option<Box<dyn Fn(u64, u64) + Send + Sync>>, done: u64, total: u64) {
    if let Some(callback) = progress {
        callback(done.min(total), total);
    }
}

/// Filter the repository listing down to the requested files and skip unsafe
/// pickle-style payloads (unless explicitly requested, which is an error).
fn select_files(files: Vec<RemoteFile>, wanted: Option<&str>) -> Result<Vec<RemoteFile>> {
    if let Some(wanted) = wanted {
        let wanted = wanted.trim();
        if wanted.is_empty() {
            return Err(LlmguiError::Config("empty --file filter".to_string()));
        }
        let suffix = format!("/{wanted}");
        let matched: Vec<RemoteFile> = files
            .into_iter()
            .filter(|file| {
                file.path == wanted
                    || file.path.ends_with(&suffix)
                    || file.path.rsplit('/').next() == Some(wanted)
            })
            .collect();
        if matched.is_empty() {
            return Err(LlmguiError::NotFound(format!(
                "file {wanted:?} not found in repository listing"
            )));
        }
        if let Some(file) = matched
            .iter()
            .find(|file| manifest::is_forbidden_file(&file.path))
        {
            return Err(LlmguiError::Unsupported(format!(
                "refusing to download unsafe file type: {}",
                file.path
            )));
        }
        return Ok(matched);
    }
    let selected: Vec<RemoteFile> = files
        .into_iter()
        .filter(|file| {
            let forbidden = manifest::is_forbidden_file(&file.path);
            if forbidden {
                tracing::warn!(path = %file.path, "skipping pickle-style file");
            }
            !forbidden
        })
        .collect();
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::test_support::{FakeSource, TestResponse, ranged_response, spawn};

    fn store() -> (tempfile::TempDir, tempfile::TempDir, CasStore) {
        let cache = tempfile::tempdir().unwrap();
        let models = tempfile::tempdir().unwrap();
        let store = CasStore::with_roots(cache.path(), models.path()).unwrap();
        (cache, models, store)
    }

    fn remote(path: &str, data: &[u8]) -> RemoteFile {
        RemoteFile {
            path: path.to_string(),
            size: data.len() as u64,
            sha256: Some(hex::encode(Sha256::digest(data))),
        }
    }

    fn options() -> DownloadOptions {
        DownloadOptions {
            revision: "master".to_string(),
            file: None,
            concurrency: 2,
            chunk_size: 512,
            cache_dir: None,
        }
    }

    #[tokio::test]
    async fn pull_downloads_and_materializes() {
        let (cache, _models, store) = store();
        let gguf = b"gguf-weight-bytes".repeat(100);
        let config = br#"{"arch":"qwen2"}"#.to_vec();
        let source = FakeSource::new(vec![
            (remote("model-q4_k_m.gguf", &gguf), gguf.clone()),
            (remote("config.json", &config), config.clone()),
        ]);
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = Arc::clone(&updates);
        let manifest = pull_with_store(
            &store,
            &source,
            "Qwen/Qwen2.5-7B-Instruct-GGUF",
            &options(),
            Some(Box::new(move |done, total| {
                recorder.lock().unwrap().push((done, total));
            })),
        )
        .await
        .unwrap();
        let updates = updates.lock().unwrap().clone();

        assert_eq!(manifest.id, "Qwen--Qwen2.5-7B-Instruct-GGUF");
        assert_eq!(manifest.source.kind, "fake");
        assert_eq!(manifest.source.repo, "Qwen/Qwen2.5-7B-Instruct-GGUF");
        assert_eq!(manifest.source.revision, "master");
        assert_eq!(manifest.files.len(), 2);
        assert_eq!(manifest.quant.format, "gguf");
        assert_eq!(manifest.quant.kind, "Q4_K_M");
        assert_eq!(manifest.total_size(), (gguf.len() + config.len()) as u64);
        manifest.validate().unwrap();

        // Manifest + model tree live under the models root, blobs under cache.
        let model_dir = store.model_dir(&manifest.id).unwrap();
        assert_eq!(
            std::fs::read(model_dir.join("model-q4_k_m.gguf")).unwrap(),
            gguf
        );
        assert!(model_dir.join("manifest.json").is_file());
        assert!(cache.path().join("snapshots").is_dir());
        let refs = cache.path().join("refs").join("fake");
        assert!(
            std::fs::read_dir(&refs).unwrap().count() == 1,
            "source ref snapshot written"
        );
        for file in &manifest.files {
            assert!(store.has_blob(&file.sha256).unwrap());
        }
        assert_eq!(updates.first(), Some(&(0, manifest.total_size())));
        assert_eq!(
            updates.last(),
            Some(&(manifest.total_size(), manifest.total_size()))
        );
        assert!(updates.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }

    #[tokio::test]
    async fn pull_over_local_http_fixture() {
        let (cache, _models, store) = store();
        let body = Arc::new(b"0123456789abcdef".repeat(64));
        let list_body = format!(
            r#"{{"Code":200,"Data":{{"Files":[
                {{"Path":"shards/a.gguf","Size":{},"Sha256":"{}","Type":"blob"}},
                {{"Path":"config.json","Size":2,"Sha256":null,"Type":"blob"}}
            ]}}}}"#,
            body.len(),
            hex::encode(Sha256::digest(&body[..])),
        );
        let server = {
            let body = Arc::clone(&body);
            let list_body = list_body.clone();
            spawn(move |request| {
                if request.path.ends_with("/repo/files") {
                    return TestResponse::json(200, list_body.clone());
                }
                if request.query.contains("FilePath=config.json") {
                    return ranged_response(b"{}", request.header("range"));
                }
                ranged_response(&body, request.header("range"))
            })
            .await
        };
        let source = ModelScopeSource::with_base_url(&server.base_url).unwrap();
        let manifest = pull_with_store(&store, &source, "Qwen/X", &options(), None)
            .await
            .unwrap();
        assert_eq!(manifest.source.kind, "modelscope");
        assert_eq!(manifest.files.len(), 2);
        let shard = store.model_dir(&manifest.id).unwrap().join("shards/a.gguf");
        assert_eq!(std::fs::read(shard).unwrap(), *body);
        assert_eq!(manifest.total_size(), body.len() as u64 + 2);
        assert!(cache.path().join("blobs/sha256").is_dir());
    }

    #[tokio::test]
    async fn pull_reuses_cas_blobs_on_second_run() {
        let (_cache, _models, store) = store();
        let data = b"reusable-content".repeat(20);
        let file = remote("a.gguf", &data);
        let source = FakeSource::new(vec![(file.clone(), data.clone())]);
        pull_with_store(&store, &source, "Qwen/X", &options(), None)
            .await
            .unwrap();
        assert_eq!(source.fetch_count(), 1);

        // A second pull must be served from the blob store without fetching.
        let failing = FakeSource::new(vec![(file, data)]).fail_first(1000);
        let manifest = pull_with_store(&store, &failing, "Qwen/X", &options(), None)
            .await
            .unwrap();
        assert_eq!(failing.fetch_count(), 0);
        assert_eq!(manifest.files.len(), 1);
    }

    #[tokio::test]
    async fn pull_rejects_path_escape() {
        let (_cache, _models, store) = store();
        let source = FakeSource::new(vec![(
            RemoteFile {
                path: "../escape.gguf".to_string(),
                size: 1,
                sha256: None,
            },
            b"x".to_vec(),
        )]);
        let err = pull_with_store(&store, &source, "Qwen/X", &options(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmguiError::PathEscape(_)), "{err}");
        assert!(
            !store.model_dir("Qwen--X").unwrap().exists()
                || std::fs::read_dir(store.model_dir("Qwen--X").unwrap())
                    .unwrap()
                    .count()
                    == 0
        );
    }

    #[tokio::test]
    async fn pull_selects_requested_file_and_skips_unsafe_ones() {
        let (_cache, _models, store) = store();
        let a = b"aaaa".to_vec();
        let b = b"bbbb".to_vec();
        let source = FakeSource::new(vec![
            (remote("a.gguf", &a), a.clone()),
            (remote("nested/b.gguf", &b), b.clone()),
            (remote("pytorch_model.bin", b"junk"), b"junk".to_vec()),
        ]);
        let mut opts = options();
        opts.file = Some("b.gguf".to_string());
        let manifest = pull_with_store(&store, &source, "Qwen/X", &opts, None)
            .await
            .unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "nested/b.gguf");

        // Without a filter, pickle-style files are skipped.
        let opts = options();
        let source = FakeSource::new(vec![
            (remote("a.gguf", &a), a.clone()),
            (remote("pytorch_model.bin", b"junk"), b"junk".to_vec()),
        ]);
        let manifest = pull_with_store(&store, &source, "Qwen/Y", &opts, None)
            .await
            .unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "a.gguf");

        // Explicitly requesting a pickle file is an error.
        let mut opts = options();
        opts.file = Some("pytorch_model.bin".to_string());
        let source = FakeSource::new(vec![(
            remote("pytorch_model.bin", b"junk"),
            b"junk".to_vec(),
        )]);
        let err = pull_with_store(&store, &source, "Qwen/Y", &opts, None)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmguiError::Unsupported(_)), "{err}");
    }

    #[tokio::test]
    async fn pull_missing_requested_file_is_not_found() {
        let (_cache, _models, store) = store();
        let source = FakeSource::new(vec![(remote("a.gguf", b"a"), b"a".to_vec())]);
        let mut opts = options();
        opts.file = Some("nope.gguf".to_string());
        let err = pull_with_store(&store, &source, "Qwen/X", &opts, None)
            .await
            .unwrap_err();
        assert!(matches!(err, LlmguiError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn pull_defaults_empty_revision_to_master() {
        let (_cache, _models, store) = store();
        let data = b"data".to_vec();
        let source = FakeSource::new(vec![(remote("a.gguf", &data), data)]);
        let opts = DownloadOptions {
            revision: "  ".to_string(),
            ..options()
        };
        let manifest = pull_with_store(&store, &source, "Qwen/X", &opts, None)
            .await
            .unwrap();
        assert_eq!(manifest.source.revision, "master");
    }
}
