//! Concurrent range downloader with resumable `.part` files, streaming SHA256
//! verification, single-stream fallback and CAS insertion.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};

use llmgui_core::{LlmguiError, Result};

use crate::cache::{CasStore, looks_like_sha256};
use crate::source::{FetchedChunk, RemoteFile, Source};

/// Default number of concurrent range requests per file.
pub const DEFAULT_CONCURRENCY: usize = 8;
/// Default range size (16 MiB).
pub const DEFAULT_CHUNK_SIZE: u64 = 16 * 1024 * 1024;

const STATE_VERSION: u32 = 1;
const MAX_ATTEMPTS: usize = 3;
const RETRY_BASE_DELAY_MS: u64 = 150;
const HASH_BUFFER: usize = 1024 * 1024;

/// Result of downloading one repository file into the CAS.
#[derive(Debug, Clone)]
pub struct DownloadedFile {
    /// Path of the content blob (or its existing copy when reused).
    pub path: PathBuf,
    /// Verified SHA256 of the file content.
    pub sha256: String,
    pub size: u64,
    /// `true` when the blob already existed and no network traffic was needed.
    pub reused: bool,
}

/// Chunked, resumable downloader bound to one source and CAS store.
pub struct Downloader<'a> {
    source: &'a dyn Source,
    store: &'a CasStore,
    concurrency: usize,
    chunk_size: u64,
    max_attempts: usize,
}

impl<'a> Downloader<'a> {
    pub fn new(
        source: &'a dyn Source,
        store: &'a CasStore,
        concurrency: usize,
        chunk_size: u64,
    ) -> Self {
        Self {
            source,
            store,
            concurrency: if concurrency == 0 {
                DEFAULT_CONCURRENCY
            } else {
                concurrency
            },
            chunk_size: if chunk_size == 0 {
                DEFAULT_CHUNK_SIZE
            } else {
                chunk_size
            },
            max_attempts: MAX_ATTEMPTS,
        }
    }

    /// Download `file` into the CAS, reporting per-file progress in bytes.
    ///
    /// The download is resumable: chunk completion is persisted to
    /// `<cache>/state/<key>.state.json` and the partial data lives in
    /// `<cache>/state/<key>.part`. A server that does not support `Range`
    /// causes an automatic single-stream fallback.
    pub async fn download<F>(
        &self,
        repo: &str,
        revision: &str,
        file: &RemoteFile,
        mut on_progress: F,
    ) -> Result<DownloadedFile>
    where
        F: FnMut(u64),
    {
        let expected = expected_sha256(file);
        if let Some(sha) = &expected
            && self.store.has_blob(sha)?
        {
            let path = self.store.blob_path(sha)?;
            on_progress(file.size);
            return Ok(DownloadedFile {
                path,
                sha256: sha.clone(),
                size: file.size,
                reused: true,
            });
        }

        let key = identity_key(repo, revision, file);
        let part = self.store.part_path(&key)?;
        let state_path = self.store.state_path(&key)?;

        match self
            .download_chunked(file, &key, &part, &state_path, &mut on_progress)
            .await
        {
            Ok(actual) => {
                self.finish(actual, &part, &state_path, file, &mut on_progress)
                    .await
            }
            Err(AttemptError::Fatal(err)) => Err(err),
            Err(AttemptError::Fallback(reason)) => {
                tracing::warn!(
                    file = %file.path,
                    error = %reason,
                    "chunked download failed; falling back to single stream"
                );
                let actual = self
                    .download_whole(file, &part, &state_path, &mut on_progress)
                    .await?;
                self.finish(actual, &part, &state_path, file, &mut on_progress)
                    .await
            }
        }
    }

    async fn finish<F>(
        &self,
        actual: String,
        part: &Path,
        state_path: &Path,
        file: &RemoteFile,
        on_progress: &mut F,
    ) -> Result<DownloadedFile>
    where
        F: FnMut(u64),
    {
        if let Some(expected) = expected_sha256(file)
            && expected != actual
        {
            // Poisoned/corrupt transfer: drop the partial data so the next
            // attempt starts clean, then surface a checksum error.
            let _ = tokio::fs::remove_file(part).await;
            let _ = tokio::fs::remove_file(state_path).await;
            return Err(LlmguiError::Checksum {
                path: file.path.clone(),
                expected,
                actual,
            });
        }
        let blob = self.store.insert_blob(&actual, part).await?;
        let _ = tokio::fs::remove_file(state_path).await;
        on_progress(file.size);
        Ok(DownloadedFile {
            path: blob,
            sha256: actual,
            size: file.size,
            reused: false,
        })
    }

    async fn download_chunked<F>(
        &self,
        file: &RemoteFile,
        key: &str,
        part: &Path,
        state_path: &Path,
        on_progress: &mut F,
    ) -> std::result::Result<String, AttemptError>
    where
        F: FnMut(u64),
    {
        let mut state = self
            .prepare_state(file, key, part, state_path)
            .await
            .map_err(AttemptError::Fatal)?;
        let total_chunks = chunk_count(file.size, self.chunk_size);
        let mut downloaded: u64 = state
            .completed
            .iter()
            .map(|index| chunk_len(*index, file.size, self.chunk_size))
            .sum();
        on_progress(downloaded);

        let pending: Vec<(u32, u64, u64)> = (0..total_chunks)
            .filter(|index| !state.completed.contains(index))
            .map(|index| {
                let start = u64::from(index) * self.chunk_size;
                let end = (start + self.chunk_size).min(file.size);
                (index, start, end)
            })
            .collect();

        if !pending.is_empty() {
            let source = self.source;
            let file_size = file.size;
            let part = part.to_path_buf();
            // Each task opens its own handle: on Windows a duplicated handle
            // shares the file pointer, so `try_clone` would corrupt concurrent
            // positional writes.
            let mut stream = stream::iter(pending.into_iter().map(move |(index, start, end)| {
                let part = part.clone();
                async move {
                    let outcome =
                        fetch_chunk_with_retry(source, file, start..end, MAX_ATTEMPTS).await?;
                    let etag = outcome.etag.clone();
                    let bytes = validate_chunk(outcome, start, end - start, file_size)
                        .map_err(AttemptError::Fallback)?;
                    let mut handle = OpenOptions::new()
                        .write(true)
                        .open(&part)
                        .await
                        .map_err(|err| AttemptError::Fatal(err.into()))?;
                    handle
                        .seek(SeekFrom::Start(start))
                        .await
                        .map_err(|err| AttemptError::Fatal(err.into()))?;
                    handle
                        .write_all(&bytes)
                        .await
                        .map_err(|err| AttemptError::Fatal(err.into()))?;
                    handle
                        .flush()
                        .await
                        .map_err(|err| AttemptError::Fatal(err.into()))?;
                    Ok::<_, AttemptError>((index, end - start, etag))
                }
            }))
            .buffer_unordered(self.concurrency);

            while let Some(item) = stream.next().await {
                let (index, len, etag) = item?;
                // An ETag change invalidates the resume state: restart the file
                // as a single stream instead of mixing two revisions.
                if let Some(existing) = &state.etag {
                    if let Some(current) = &etag
                        && current != existing
                    {
                        return Err(AttemptError::Fallback(LlmguiError::Download(
                            "remote ETag changed during download".to_string(),
                        )));
                    }
                } else if etag.is_some() {
                    state.etag = etag;
                }
                state.completed.insert(index);
                save_state(state_path, &state)
                    .await
                    .map_err(AttemptError::Fatal)?;
                downloaded += len;
                on_progress(downloaded);
            }
            drop(stream);
        }

        hash_file(part).await.map_err(AttemptError::Fatal)
    }

    async fn download_whole<F>(
        &self,
        file: &RemoteFile,
        part: &Path,
        state_path: &Path,
        on_progress: &mut F,
    ) -> Result<String>
    where
        F: FnMut(u64),
    {
        let mut last: Option<LlmguiError> = None;
        for attempt in 1..=self.max_attempts {
            match self.source.fetch_chunk(file, None).await {
                Ok(outcome) => {
                    if outcome.data.len() as u64 != file.size {
                        last = Some(LlmguiError::Download(format!(
                            "{}: whole-file fetch returned {} bytes, expected {}",
                            file.path,
                            outcome.data.len(),
                            file.size
                        )));
                    } else {
                        let mut handle = File::create(part).await?;
                        handle.write_all(&outcome.data).await?;
                        handle.flush().await?;
                        handle.sync_all().await?;
                        let _ = tokio::fs::remove_file(state_path).await;
                        on_progress(file.size);
                        return Ok(hex::encode(Sha256::digest(&outcome.data)));
                    }
                }
                Err(err) => last = Some(err),
            }
            if attempt < self.max_attempts {
                tokio::time::sleep(Duration::from_millis(RETRY_BASE_DELAY_MS * attempt as u64))
                    .await;
            }
        }
        Err(last.unwrap_or_else(|| {
            LlmguiError::Download(format!("{}: single-stream download failed", file.path))
        }))
    }

    async fn prepare_state(
        &self,
        file: &RemoteFile,
        key: &str,
        part: &Path,
        state_path: &Path,
    ) -> Result<ResumeState> {
        let expected = expected_sha256(file);
        let total_chunks = chunk_count(file.size, self.chunk_size);
        if let Ok(bytes) = tokio::fs::read(state_path).await
            && let Ok(mut existing) = serde_json::from_slice::<ResumeState>(&bytes)
        {
            let part_len = tokio::fs::metadata(part).await.ok().map(|meta| meta.len());
            if existing.version == STATE_VERSION
                && existing.key == key
                && existing.size == file.size
                && existing.chunk_size == self.chunk_size
                && existing.expected_sha256 == expected
                && part_len == Some(file.size)
            {
                existing.completed.retain(|index| *index < total_chunks);
                return Ok(existing);
            }
        }
        // Fresh download: discard any stale partial data.
        let _ = tokio::fs::remove_file(part).await;
        let state = ResumeState {
            version: STATE_VERSION,
            key: key.to_string(),
            size: file.size,
            chunk_size: self.chunk_size,
            expected_sha256: expected,
            etag: None,
            completed: BTreeSet::new(),
        };
        if let Some(parent) = part.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let handle = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(part)
            .await?;
        handle.set_len(file.size).await?;
        handle.sync_all().await?;
        save_state(state_path, &state).await?;
        Ok(state)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeState {
    version: u32,
    key: String,
    size: u64,
    chunk_size: u64,
    expected_sha256: Option<String>,
    etag: Option<String>,
    completed: BTreeSet<u32>,
}

/// Chunk-level failure: `Fallback` means "retry as a single stream",
/// `Fatal` aborts the file download.
enum AttemptError {
    Fallback(LlmguiError),
    Fatal(LlmguiError),
}

impl std::fmt::Display for AttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fallback(err) | Self::Fatal(err) => write!(f, "{err}"),
        }
    }
}

/// Retry one range request, returning a `206` response or a fallback signal.
async fn fetch_chunk_with_retry(
    source: &dyn Source,
    file: &RemoteFile,
    range: Range<u64>,
    max_attempts: usize,
) -> std::result::Result<FetchedChunk, AttemptError> {
    let mut last: Option<LlmguiError> = None;
    for attempt in 1..=max_attempts {
        match source.fetch_chunk(file, Some(range.clone())).await {
            Ok(outcome) => {
                if outcome.range_ignored || outcome.status == 200 {
                    return Err(AttemptError::Fallback(LlmguiError::Download(format!(
                        "{}: server ignored Range for {}",
                        source.name(),
                        file.path
                    ))));
                }
                if outcome.status == 206 {
                    return Ok(outcome);
                }
                return Err(AttemptError::Fallback(LlmguiError::Http(format!(
                    "{}: expected 206 for range request, got {}",
                    file.path, outcome.status
                ))));
            }
            Err(err) => {
                let retryable = !matches!(err, LlmguiError::NotFound(_));
                last = Some(err);
                if !retryable {
                    break;
                }
                if attempt < max_attempts {
                    tokio::time::sleep(Duration::from_millis(RETRY_BASE_DELAY_MS * attempt as u64))
                        .await;
                }
            }
        }
    }
    Err(AttemptError::Fallback(last.unwrap_or_else(|| {
        LlmguiError::Download(format!("{}: range request failed", file.path))
    })))
}

/// Validate a `206 Content-Range` response against the requested range.
fn validate_chunk(
    outcome: FetchedChunk,
    offset: u64,
    expected_len: u64,
    file_size: u64,
) -> std::result::Result<Vec<u8>, LlmguiError> {
    if outcome.status != 206 {
        return Err(LlmguiError::Http(format!(
            "expected 206 Partial Content, got {}",
            outcome.status
        )));
    }
    let content_range = outcome.content_range.ok_or_else(|| {
        LlmguiError::Download("206 response is missing a Content-Range header".to_string())
    })?;
    if content_range.start != offset {
        return Err(LlmguiError::Download(format!(
            "Content-Range starts at {}, expected {offset}",
            content_range.start
        )));
    }
    if content_range.len() != expected_len {
        return Err(LlmguiError::Download(format!(
            "Content-Range length {}, expected {expected_len}",
            content_range.len()
        )));
    }
    if let Some(total) = content_range.total
        && total != file_size
    {
        return Err(LlmguiError::Download(format!(
            "Content-Range total {total}, expected {file_size}"
        )));
    }
    if outcome.data.len() as u64 != expected_len {
        return Err(LlmguiError::Download(format!(
            "range body length {}, expected {expected_len}",
            outcome.data.len()
        )));
    }
    Ok(outcome.data)
}

fn expected_sha256(file: &RemoteFile) -> Option<String> {
    file.sha256
        .as_deref()
        .filter(|sha| looks_like_sha256(sha))
        .map(|sha| sha.to_ascii_lowercase())
}

fn chunk_count(size: u64, chunk_size: u64) -> u32 {
    if size == 0 {
        0
    } else {
        size.div_ceil(chunk_size.max(1)) as u32
    }
}

fn chunk_len(index: u32, size: u64, chunk_size: u64) -> u64 {
    let start = u64::from(index) * chunk_size;
    if start >= size {
        0
    } else {
        (start + chunk_size).min(size) - start
    }
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn save_state(path: &Path, state: &ResumeState) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(state)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Stable identity for one remote file, used for the resume state key.
pub(crate) fn identity_key(repo: &str, revision: &str, file: &RemoteFile) -> String {
    let mut hasher = Sha256::new();
    hasher.update(repo.as_bytes());
    hasher.update([0]);
    hasher.update(revision.as_bytes());
    hasher.update([0]);
    hasher.update(file.path.as_bytes());
    hasher.update([0]);
    hasher.update(file.size.to_le_bytes());
    hasher.update([0]);
    hasher.update(file.sha256.as_deref().unwrap_or_default().as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeSource;

    fn store() -> (tempfile::TempDir, tempfile::TempDir, CasStore) {
        let cache = tempfile::tempdir().unwrap();
        let models = tempfile::tempdir().unwrap();
        let store = CasStore::with_roots(cache.path(), models.path()).unwrap();
        (cache, models, store)
    }

    fn payload(size: usize) -> Vec<u8> {
        (0..size).map(|index| (index * 31 % 251) as u8).collect()
    }

    fn remote(path: &str, data: &[u8], with_sha: bool) -> RemoteFile {
        RemoteFile {
            path: path.to_string(),
            size: data.len() as u64,
            sha256: with_sha.then(|| hex::encode(Sha256::digest(data))),
        }
    }

    #[tokio::test]
    async fn downloads_all_chunks_into_cas() {
        let (_cache, _models, store) = store();
        let data = payload(2500);
        let file = remote("a.gguf", &data, true);
        let source = FakeSource::new(vec![(file.clone(), data.clone())]);
        source.list("Qwen/X", "master").await.unwrap();
        let downloader = Downloader::new(&source, &store, 3, 1024);
        let result = downloader
            .download("Qwen/X", "master", &file, |_| {})
            .await
            .unwrap();
        assert_eq!(result.sha256, file.sha256.clone().unwrap());
        assert!(!result.reused);
        assert_eq!(std::fs::read(&result.path).unwrap(), data);
        assert_eq!(source.fetch_count(), 3);
        let key = identity_key("Qwen/X", "master", &file);
        assert!(!store.state_path(&key).unwrap().exists());
        assert!(!store.part_path(&key).unwrap().exists());
    }

    #[tokio::test]
    async fn empty_file_is_hashed_and_stored() {
        let (_cache, _models, store) = store();
        let data = Vec::new();
        let file = remote("empty.txt", &data, true);
        let source = FakeSource::new(vec![(file.clone(), data.clone())]);
        source.list("Qwen/X", "master").await.unwrap();
        let downloader = Downloader::new(&source, &store, 2, 16);
        let result = downloader
            .download("Qwen/X", "master", &file, |_| {})
            .await
            .unwrap();
        assert_eq!(result.size, 0);
        assert_eq!(result.sha256, hex::encode(Sha256::digest(b"")));
        assert_eq!(std::fs::read(&result.path).unwrap(), data);
        assert_eq!(source.fetch_count(), 0);
    }

    #[tokio::test]
    async fn reuses_existing_blob_without_network() {
        let (cache, _models, store) = store();
        let data = payload(64);
        let file = remote("a.gguf", &data, true);
        let sha = file.sha256.clone().unwrap();
        // Seed the CAS manually.
        let src = cache.path().join("seeded");
        std::fs::write(&src, &data).unwrap();
        store.insert_blob(&sha, &src).await.unwrap();

        let source = FakeSource::new(vec![(file.clone(), data.clone())]);
        source.list("Qwen/X", "master").await.unwrap();
        let downloader = Downloader::new(&source, &store, 2, 16);
        let result = downloader
            .download("Qwen/X", "master", &file, |_| {})
            .await
            .unwrap();
        assert!(result.reused);
        assert_eq!(source.fetch_count(), 0);
    }

    #[tokio::test]
    async fn sha_mismatch_is_fatal_and_cleans_partial_state() {
        let (_cache, _models, store) = store();
        let data = payload(100);
        let mut file = remote("a.gguf", &data, true);
        file.sha256 = Some("f".repeat(64));
        let source = FakeSource::new(vec![(file.clone(), data.clone())]);
        source.list("Qwen/X", "master").await.unwrap();
        let downloader = Downloader::new(&source, &store, 2, 32);
        let err = downloader
            .download("Qwen/X", "master", &file, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, LlmguiError::Checksum { .. }), "{err}");
        let key = identity_key("Qwen/X", "master", &file);
        assert!(!store.part_path(&key).unwrap().exists());
        assert!(!store.state_path(&key).unwrap().exists());
    }

    #[tokio::test]
    async fn falls_back_when_server_ignores_range() {
        let (_cache, _models, store) = store();
        let data = payload(700);
        let file = remote("a.gguf", &data, true);
        let source = FakeSource::new(vec![(file.clone(), data.clone())]).ignore_range();
        source.list("Qwen/X", "master").await.unwrap();
        let downloader = Downloader::new(&source, &store, 2, 256);
        let result = downloader
            .download("Qwen/X", "master", &file, |_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&result.path).unwrap(), data);
        assert_eq!(result.sha256, file.sha256.clone().unwrap());
    }

    #[tokio::test]
    async fn retries_transient_failures() {
        let (_cache, _models, store) = store();
        let data = payload(300);
        let file = remote("a.gguf", &data, true);
        let source = FakeSource::new(vec![(file.clone(), data.clone())]).fail_first(2);
        source.list("Qwen/X", "master").await.unwrap();
        let downloader = Downloader::new(&source, &store, 1, 100);
        let result = downloader
            .download("Qwen/X", "master", &file, |_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&result.path).unwrap(), data);
        assert_eq!(source.fetch_count(), 3, "three successful chunk fetches");
        assert_eq!(
            source.attempt_count(),
            5,
            "two transient failures + 3 chunks"
        );
    }

    #[tokio::test]
    async fn resumes_partial_chunk_state() {
        let (_cache, _models, store) = store();
        let data = payload(3000);
        let file = remote("a.gguf", &data, true);
        let key = identity_key("Qwen/X", "master", &file);
        let part = store.part_path(&key).unwrap();
        let state_path = store.state_path(&key).unwrap();

        // Simulate an interrupted run: first 1000-byte chunk already on disk.
        std::fs::write(&part, &data[..1000]).unwrap();
        let mut part_file = OpenOptions::new().write(true).open(&part).await.unwrap();
        part_file.set_len(3000).await.unwrap();
        part_file.seek(SeekFrom::Start(0)).await.unwrap();
        part_file.write_all(&data[..1000]).await.unwrap();
        part_file.flush().await.unwrap();
        drop(part_file);
        save_state(
            &state_path,
            &ResumeState {
                version: STATE_VERSION,
                key: key.clone(),
                size: 3000,
                chunk_size: 1000,
                expected_sha256: file.sha256.clone(),
                etag: None,
                completed: BTreeSet::from([0]),
            },
        )
        .await
        .unwrap();

        let source = FakeSource::new(vec![(file.clone(), data.clone())]);
        source.list("Qwen/X", "master").await.unwrap();
        let mut progress = Vec::new();
        let downloader = Downloader::new(&source, &store, 2, 1000);
        let result = downloader
            .download("Qwen/X", "master", &file, |done| progress.push(done))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&result.path).unwrap(), data);
        assert_eq!(source.fetch_count(), 2, "only missing chunks are fetched");
        let mut ranges = source.requested_ranges();
        ranges.sort_by_key(|range| range.as_ref().map(|range| range.start));
        assert_eq!(
            ranges
                .iter()
                .map(|range| range.as_ref().map(|range| (range.start, range.end)))
                .collect::<Vec<_>>(),
            vec![Some((1000, 2000)), Some((2000, 3000))]
        );
        assert_eq!(progress, vec![1000u64, 2000, 3000, 3000]);
        assert!(!state_path.exists());
    }

    #[tokio::test]
    async fn reset_state_when_file_identity_changes() {
        let (_cache, _models, store) = store();
        let data = payload(500);
        let file = remote("a.gguf", &data, true);
        let key = identity_key("Qwen/X", "master", &file);
        let part = store.part_path(&key).unwrap();
        let state_path = store.state_path(&key).unwrap();
        // Stale state with a different size must be discarded.
        std::fs::write(&part, vec![0u8; 10]).unwrap();
        save_state(
            &state_path,
            &ResumeState {
                version: STATE_VERSION,
                key,
                size: 10,
                chunk_size: 100,
                expected_sha256: None,
                etag: None,
                completed: BTreeSet::from([0]),
            },
        )
        .await
        .unwrap();
        let source = FakeSource::new(vec![(file.clone(), data.clone())]);
        source.list("Qwen/X", "master").await.unwrap();
        let downloader = Downloader::new(&source, &store, 2, 100);
        let result = downloader
            .download("Qwen/X", "master", &file, |_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&result.path).unwrap(), data);
        assert_eq!(source.fetch_count(), 5);
    }

    #[test]
    fn chunk_math() {
        assert_eq!(chunk_count(0, 10), 0);
        assert_eq!(chunk_count(10, 10), 1);
        assert_eq!(chunk_count(11, 10), 2);
        assert_eq!(chunk_len(0, 11, 10), 10);
        assert_eq!(chunk_len(1, 11, 10), 1);
        assert_eq!(chunk_len(2, 11, 10), 0);
    }
}
