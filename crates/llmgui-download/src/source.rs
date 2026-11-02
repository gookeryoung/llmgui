//! Source adapters: ModelScope REST, and HF-compatible endpoints
//! (`hf-mirror.com`, Gitee AI / 模力方舟).

use std::fmt;
use std::ops::Range;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{CONTENT_RANGE, ETAG, RANGE};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;

use llmgui_core::{LlmguiError, Result};

use crate::cache::looks_like_sha256;

/// Default revision used when the caller passes an empty revision.
pub const DEFAULT_REVISION: &str = "master";
/// ModelScope production endpoint.
pub const MODEL_SCOPE_BASE: &str = "https://modelscope.cn";
/// Hugging Face mirror endpoint.
pub const HF_MIRROR_BASE: &str = "https://hf-mirror.com";
/// Gitee AI (模力方舟) HF-compatible endpoint.
pub const GITEE_AI_BASE: &str = "https://hf-api.gitee.com";

/// One file listed by a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFile {
    pub path: String,
    pub size: u64,
    /// Expected content SHA256, when the source exposes it.
    pub sha256: Option<String>,
}

/// Parsed `Content-Range: bytes <start>-<end>/<total>` response header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    /// Inclusive first byte offset.
    pub start: u64,
    /// Inclusive last byte offset.
    pub end: u64,
    /// Total resource size when the server reports it (`*` otherwise).
    pub total: Option<u64>,
}

impl ContentRange {
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start).saturating_add(1)
    }

    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }

    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.trim().strip_prefix("bytes")?.trim_start();
        let (range, total) = rest.split_once('/')?;
        let (start, end) = range.split_once('-')?;
        let start = start.trim().parse::<u64>().ok()?;
        let end = end.trim().parse::<u64>().ok()?;
        if end < start {
            return None;
        }
        let total = match total.trim() {
            "*" => None,
            value => Some(value.parse::<u64>().ok()?),
        };
        Some(Self { start, end, total })
    }
}

impl fmt::Display for ContentRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "bytes {}-{}/{}",
            self.start,
            self.end,
            self.total
                .map_or_else(|| "*".to_string(), |total| total.to_string())
        )
    }
}

/// One HTTP chunk returned by a [`Source`].
#[derive(Debug, Clone)]
pub struct FetchedChunk {
    pub data: Vec<u8>,
    pub status: u16,
    pub content_range: Option<ContentRange>,
    pub etag: Option<String>,
    /// The server answered a ranged request with `200 OK` (Range unsupported).
    pub range_ignored: bool,
}

/// A model source. Implementations are thin HTTP adapters; retry, fallback and
/// CAS logic live in the downloader.
#[async_trait]
pub trait Source: Send + Sync {
    /// Stable source name (`modelscope`, `hf-mirror`, `gitee-ai`).
    fn name(&self) -> &str;

    /// List all blob files of `repo` at `revision`.
    async fn list(&self, repo: &str, revision: &str) -> Result<Vec<RemoteFile>>;

    /// Fetch `file` (or the requested byte range) into memory.
    async fn fetch(&self, file: &RemoteFile, range: Option<Range<u64>>) -> Result<Vec<u8>>;

    /// Fetch `file` and keep HTTP metadata (status/Content-Range/ETag) so the
    /// downloader can verify `206` responses. The default is a byte-only
    /// fallback for simple in-memory sources.
    async fn fetch_chunk(
        &self,
        file: &RemoteFile,
        range: Option<Range<u64>>,
    ) -> Result<FetchedChunk> {
        let ranged = range.is_some();
        let data = self.fetch(file, range).await?;
        Ok(FetchedChunk {
            data,
            status: 200,
            content_range: None,
            etag: None,
            range_ignored: ranged,
        })
    }
}

/// Normalise an empty revision to [`DEFAULT_REVISION`].
pub fn normalize_revision(revision: &str) -> String {
    let revision = revision.trim();
    if revision.is_empty() {
        DEFAULT_REVISION.to_string()
    } else {
        revision.to_string()
    }
}

/// Reject unsafe repository ids before they are placed into a URL path.
fn repo_segments(repo: &str) -> Result<Vec<String>> {
    let mut segments = Vec::new();
    for part in repo.trim().trim_matches('/').split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(LlmguiError::Config(format!(
                "invalid repository id: {repo:?}"
            )));
        }
        if part.contains('\\') || part.contains(':') || part.chars().any(char::is_control) {
            return Err(LlmguiError::Config(format!(
                "unsafe repository id: {repo:?}"
            )));
        }
        segments.push(part.to_string());
    }
    if segments.is_empty() {
        return Err(LlmguiError::Config("empty repository id".to_string()));
    }
    Ok(segments)
}

/// Validate a relative model file path and return its segments.
pub fn relative_path_segments(path: &str) -> Result<Vec<String>> {
    if path.is_empty() {
        return Err(LlmguiError::Download("empty model file path".to_string()));
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err(LlmguiError::PathEscape(std::path::PathBuf::from(path)));
    }
    let mut segments = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(LlmguiError::PathEscape(std::path::PathBuf::from(path)));
        }
        if part.chars().any(char::is_control) {
            return Err(LlmguiError::PathEscape(std::path::PathBuf::from(path)));
        }
        segments.push(part.to_string());
    }
    Ok(segments)
}

pub(crate) fn validate_relative_path(path: &str) -> Result<()> {
    relative_path_segments(path).map(|_| ())
}

fn build_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("llmgui-download/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| LlmguiError::Http(format!("build http client: {err}")))
}

fn parse_base(base: &str) -> Result<Url> {
    Url::parse(base)
        .map_err(|err| LlmguiError::Config(format!("invalid source base {base:?}: {err}")))
}

/// Append path segments to `base`, percent-encoding each segment.
fn with_path(base: &Url, segments: &[String]) -> Result<Url> {
    let mut url = base.clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| LlmguiError::Config(format!("source base {base} cannot be a base url")))?;
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
}

fn modelscope_list_url(base: &Url, repo: &str, revision: &str) -> Result<Url> {
    let repo = repo_segments(repo)?;
    let mut segments = vec!["api".to_string(), "v1".to_string(), "models".to_string()];
    segments.extend(repo);
    segments.push("repo".to_string());
    segments.push("files".to_string());
    let mut url = with_path(base, &segments)?;
    url.query_pairs_mut()
        .append_pair("Revision", revision)
        .append_pair("Recursive", "true");
    Ok(url)
}

fn modelscope_download_url(base: &Url, repo: &str, revision: &str, path: &str) -> Result<Url> {
    let repo = repo_segments(repo)?;
    let mut segments = vec!["api".to_string(), "v1".to_string(), "models".to_string()];
    segments.extend(repo);
    segments.push("repo".to_string());
    let mut url = with_path(base, &segments)?;
    url.query_pairs_mut()
        .append_pair("Revision", revision)
        .append_pair("FilePath", path);
    Ok(url)
}

fn hf_tree_url(base: &Url, repo: &str, revision: &str) -> Result<Url> {
    let repo = repo_segments(repo)?;
    let mut segments = vec!["api".to_string(), "models".to_string()];
    segments.extend(repo);
    segments.push("tree".to_string());
    segments.push(normalize_revision(revision));
    let mut url = with_path(base, &segments)?;
    url.query_pairs_mut().append_pair("recursive", "true");
    Ok(url)
}

fn hf_download_url(base: &Url, repo: &str, revision: &str, path: &str) -> Result<Url> {
    let mut segments = repo_segments(repo)?;
    segments.push("resolve".to_string());
    segments.push(normalize_revision(revision));
    segments.extend(relative_path_segments(path)?);
    with_path(base, &segments)
}

async fn get_chunk(client: &Client, url: Url, range: Option<Range<u64>>) -> Result<FetchedChunk> {
    let mut request = client.get(url.clone());
    if let Some(range) = &range {
        if range.end <= range.start {
            return Err(LlmguiError::Download(format!(
                "invalid byte range {}..{}",
                range.start, range.end
            )));
        }
        request = request.header(RANGE, format!("bytes={}-{}", range.start, range.end - 1));
    }
    let response = request
        .send()
        .await
        .map_err(|err| LlmguiError::Http(format!("GET {url} failed: {err}")))?;
    let status = response.status();
    if !status.is_success() {
        let message = format!("GET {url} returned {status}");
        return Err(if status == StatusCode::NOT_FOUND {
            LlmguiError::NotFound(message)
        } else {
            LlmguiError::Http(message)
        });
    }
    let content_range = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(ContentRange::parse);
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status_code = status.as_u16();
    let range_ignored = range.is_some() && status == StatusCode::OK;
    let data = response
        .bytes()
        .await
        .map_err(|err| LlmguiError::Http(format!("read body of {url}: {err}")))?
        .to_vec();
    Ok(FetchedChunk {
        data,
        status: status_code,
        content_range,
        etag,
        range_ignored,
    })
}

#[derive(Debug, Clone)]
struct SourceContext {
    repo: String,
    revision: String,
}

/// ModelScope (`modelscope.cn`) adapter.
#[derive(Debug)]
pub struct ModelScopeSource {
    client: Client,
    base_url: Url,
    context: Mutex<Option<SourceContext>>,
}

impl ModelScopeSource {
    pub fn new() -> Result<Self> {
        Self::with_base_url(MODEL_SCOPE_BASE)
    }

    /// Build an adapter against a custom base URL (local fixtures, enterprise
    /// proxies). Kept public for tests and future config support.
    pub fn with_base_url(base: impl AsRef<str>) -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            base_url: parse_base(base.as_ref())?,
            context: Mutex::new(None),
        })
    }

    fn context(&self) -> Result<SourceContext> {
        self.context
            .lock()
            .expect("source context lock poisoned")
            .clone()
            .ok_or_else(|| {
                LlmguiError::Download(format!(
                    "{}: fetch() called before list(); source has no repo context",
                    self.name()
                ))
            })
    }
}

#[async_trait]
impl Source for ModelScopeSource {
    fn name(&self) -> &str {
        "modelscope"
    }

    async fn list(&self, repo: &str, revision: &str) -> Result<Vec<RemoteFile>> {
        let revision = normalize_revision(revision);
        let url = modelscope_list_url(&self.base_url, repo, &revision)?;
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|err| LlmguiError::Http(format!("GET {url} failed: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            let message = format!("GET {url} returned {status}");
            return Err(if status == StatusCode::NOT_FOUND {
                LlmguiError::NotFound(message)
            } else {
                LlmguiError::Http(message)
            });
        }
        let payload: ModelScopeListResponse = response
            .json()
            .await
            .map_err(|err| LlmguiError::Http(format!("parse modelscope listing: {err}")))?;
        if let Some(code) = payload.code
            && code != 200
            && code != 0
        {
            return Err(LlmguiError::Download(format!(
                "modelscope listing for {repo}@{revision} returned code {code}: {}",
                payload.message.unwrap_or_default()
            )));
        }
        let files = payload
            .data
            .map(|data| data.files)
            .unwrap_or_default()
            .into_iter()
            .filter(|file| file.kind.eq_ignore_ascii_case("blob"))
            .filter_map(|file| {
                let path = file.path?;
                if path.is_empty() {
                    return None;
                }
                Some(RemoteFile {
                    path,
                    size: file.size.unwrap_or(0),
                    sha256: file
                        .sha256
                        .filter(|sha| looks_like_sha256(sha))
                        .map(|sha| sha.to_ascii_lowercase()),
                })
            })
            .collect();
        *self.context.lock().expect("source context lock poisoned") = Some(SourceContext {
            repo: repo.to_string(),
            revision,
        });
        Ok(files)
    }

    async fn fetch(&self, file: &RemoteFile, range: Option<Range<u64>>) -> Result<Vec<u8>> {
        Ok(self.fetch_chunk(file, range).await?.data)
    }

    async fn fetch_chunk(
        &self,
        file: &RemoteFile,
        range: Option<Range<u64>>,
    ) -> Result<FetchedChunk> {
        let context = self.context()?;
        validate_relative_path(&file.path)?;
        let url =
            modelscope_download_url(&self.base_url, &context.repo, &context.revision, &file.path)?;
        get_chunk(&self.client, url, range).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HfKind {
    HfMirror,
    GiteeAi,
}

impl HfKind {
    fn parse(kind: &str) -> Result<Self> {
        match kind {
            "hf-mirror" => Ok(Self::HfMirror),
            "gitee-ai" => Ok(Self::GiteeAi),
            other => Err(LlmguiError::Config(format!(
                "unknown HF-compatible source {other}; use hf-mirror|gitee-ai"
            ))),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::HfMirror => "hf-mirror",
            Self::GiteeAi => "gitee-ai",
        }
    }

    fn default_base(self) -> &'static str {
        match self {
            Self::HfMirror => HF_MIRROR_BASE,
            Self::GiteeAi => GITEE_AI_BASE,
        }
    }
}

/// Hugging Face-compatible adapter (hf-mirror.com and Gitee AI).
#[derive(Debug)]
pub struct HfSource {
    client: Client,
    base_url: Url,
    kind: HfKind,
    context: Mutex<Option<SourceContext>>,
}

impl HfSource {
    /// `kind` is `hf-mirror` or `gitee-ai`.
    pub fn new(kind: &str) -> Result<Self> {
        let kind = HfKind::parse(kind)?;
        Self::build(kind, kind.default_base())
    }

    /// Build an adapter against a custom base URL (local fixtures).
    pub fn with_base_url(kind: &str, base: impl AsRef<str>) -> Result<Self> {
        let kind = HfKind::parse(kind)?;
        Self::build(kind, base.as_ref())
    }

    fn build(kind: HfKind, base: &str) -> Result<Self> {
        Ok(Self {
            client: build_client()?,
            base_url: parse_base(base)?,
            kind,
            context: Mutex::new(None),
        })
    }

    fn context(&self) -> Result<SourceContext> {
        self.context
            .lock()
            .expect("source context lock poisoned")
            .clone()
            .ok_or_else(|| {
                LlmguiError::Download(format!(
                    "{}: fetch() called before list(); source has no repo context",
                    self.name()
                ))
            })
    }
}

#[async_trait]
impl Source for HfSource {
    fn name(&self) -> &str {
        self.kind.name()
    }

    async fn list(&self, repo: &str, revision: &str) -> Result<Vec<RemoteFile>> {
        let revision = normalize_revision(revision);
        let url = hf_tree_url(&self.base_url, repo, &revision)?;
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|err| LlmguiError::Http(format!("GET {url} failed: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            let message = format!("GET {url} returned {status}");
            return Err(if status == StatusCode::NOT_FOUND {
                LlmguiError::NotFound(message)
            } else {
                LlmguiError::Http(message)
            });
        }
        let entries: Vec<HfTreeEntry> = response
            .json()
            .await
            .map_err(|err| LlmguiError::Http(format!("parse HF tree listing: {err}")))?;
        let files = entries
            .into_iter()
            .filter(|entry| {
                entry
                    .kind
                    .as_deref()
                    .is_none_or(|kind| kind.eq_ignore_ascii_case("file"))
            })
            .filter_map(|entry| {
                let path = entry.path?;
                if path.is_empty() {
                    return None;
                }
                let lfs_size = entry.lfs.as_ref().and_then(|lfs| lfs.size);
                let sha256 = entry
                    .lfs
                    .as_ref()
                    .and_then(|lfs| lfs.oid.clone())
                    .filter(|oid| looks_like_sha256(oid))
                    .map(|oid| oid.to_ascii_lowercase());
                Some(RemoteFile {
                    path,
                    size: lfs_size.or(entry.size).unwrap_or(0),
                    sha256,
                })
            })
            .collect();
        *self.context.lock().expect("source context lock poisoned") = Some(SourceContext {
            repo: repo.to_string(),
            revision,
        });
        Ok(files)
    }

    async fn fetch(&self, file: &RemoteFile, range: Option<Range<u64>>) -> Result<Vec<u8>> {
        Ok(self.fetch_chunk(file, range).await?.data)
    }

    async fn fetch_chunk(
        &self,
        file: &RemoteFile,
        range: Option<Range<u64>>,
    ) -> Result<FetchedChunk> {
        let context = self.context()?;
        validate_relative_path(&file.path)?;
        let url = hf_download_url(&self.base_url, &context.repo, &context.revision, &file.path)?;
        get_chunk(&self.client, url, range).await
    }
}

#[derive(Debug, Deserialize)]
struct ModelScopeListResponse {
    #[serde(rename = "Code", default)]
    code: Option<i64>,
    #[serde(rename = "Message", default)]
    message: Option<String>,
    #[serde(rename = "Data", default)]
    data: Option<ModelScopeListData>,
}

#[derive(Debug, Deserialize)]
struct ModelScopeListData {
    #[serde(rename = "Files", default)]
    files: Vec<ModelScopeFile>,
}

#[derive(Debug, Deserialize)]
struct ModelScopeFile {
    #[serde(rename = "Path", default)]
    path: Option<String>,
    #[serde(rename = "Size", default)]
    size: Option<u64>,
    #[serde(rename = "Sha256", default)]
    sha256: Option<String>,
    #[serde(rename = "Type", default)]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct HfTreeEntry {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    lfs: Option<HfLfs>,
}

#[derive(Debug, Deserialize)]
struct HfLfs {
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestResponse, ranged_response, spawn};

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn parses_content_range() {
        let range = ContentRange::parse("bytes 0-99/1000").unwrap();
        assert_eq!(range.start, 0);
        assert_eq!(range.end, 99);
        assert_eq!(range.total, Some(1000));
        assert_eq!(range.len(), 100);
        let open = ContentRange::parse("bytes 10-19/*").unwrap();
        assert_eq!(open.total, None);
        assert!(ContentRange::parse("bytes 10-9/100").is_none());
        assert!(ContentRange::parse("garbage").is_none());
    }

    #[test]
    fn validates_relative_paths() {
        assert!(validate_relative_path("sub/a.gguf").is_ok());
        assert!(validate_relative_path("../evil.gguf").is_err());
        assert!(validate_relative_path("/abs.gguf").is_err());
        assert!(validate_relative_path("a\\b.gguf").is_err());
        assert!(validate_relative_path("a//b.gguf").is_err());
    }

    #[tokio::test]
    async fn modelscope_list_parses_blobs() {
        let server = spawn(|request| {
            assert!(
                request.path.ends_with("/repo/files"),
                "path: {}",
                request.path
            );
            assert!(request.query.contains("Revision=master"));
            assert!(request.query.contains("Recursive=true"));
            TestResponse::json(
                200,
                r#"{"Code":200,"Data":{"Files":[
                    {"Path":"a.gguf","Size":3,"Sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","Type":"blob"},
                    {"Path":"sub","Size":0,"Type":"tree"},
                    {"Path":"b.gguf","Size":5,"Sha256":"","Type":"blob"}
                ]}}"#,
            )
        })
        .await;
        let source = ModelScopeSource::with_base_url(&server.base_url).unwrap();
        let files = source
            .list("Qwen/Qwen2.5-7B-Instruct-GGUF", "master")
            .await
            .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.gguf");
        assert_eq!(files[0].sha256.as_deref(), Some(SHA));
        assert_eq!(files[1].size, 5);
        assert_eq!(files[1].sha256, None);
    }

    #[tokio::test]
    async fn ranged_download_returns_content_range() {
        let body = b"0123456789".to_vec();
        let expected = body.clone();
        let server = spawn(move |request| {
            if request.path.ends_with("/repo/files") {
                return TestResponse::json(
                    200,
                    r#"{"Code":200,"Data":{"Files":[{"Path":"sub/a.gguf","Size":10,"Sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","Type":"blob"}]}}"#,
                );
            }
            assert!(request.path.contains("/api/v1/models/Qwen/X/repo"));
            assert!(request.query.contains("Revision=master"));
            assert!(request.query.contains("FilePath=sub%2Fa.gguf"));
            ranged_response(&body, request.header("range"))
        })
        .await;
        let source = ModelScopeSource::with_base_url(&server.base_url).unwrap();
        let files = source.list("Qwen/X", "master").await.unwrap();
        let file = &files[0];
        let chunk = source.fetch_chunk(file, Some(2..5)).await.unwrap();
        assert_eq!(chunk.status, 206);
        assert_eq!(chunk.data, b"234");
        let range = chunk.content_range.unwrap();
        assert_eq!((range.start, range.end, range.total), (2, 4, Some(10)));
        assert!(!chunk.range_ignored);
        assert_eq!(source.fetch(file, None).await.unwrap(), expected);
    }

    #[tokio::test]
    async fn range_ignored_is_reported() {
        let server = spawn(|request| {
            if request.path.ends_with("/tree/main") {
                TestResponse::json(
                    200,
                    r#"[{"type":"file","path":"a.gguf","size":10,"oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]"#,
                )
            } else {
                TestResponse::ok(b"whole-body".to_vec())
            }
        })
        .await;
        let source = HfSource::with_base_url("hf-mirror", &server.base_url).unwrap();
        let file = RemoteFile {
            path: "a.gguf".to_string(),
            size: 10,
            sha256: None,
        };
        // fetch before list fails loudly: the source needs repo context.
        assert!(source.fetch(&file, None).await.is_err());
        source.list("Qwen/X", "main").await.unwrap();
        let chunk = source.fetch_chunk(&file, Some(0..5)).await.unwrap();
        assert!(chunk.range_ignored);
        assert_eq!(chunk.status, 200);
        assert_eq!(chunk.data, b"whole-body");
    }

    #[tokio::test]
    async fn hf_tree_parses_lfs_sha_and_plain_oid() {
        let server = spawn(|request| {
            assert!(request.path.ends_with("/api/models/Qwen/X/tree/main"));
            assert!(request.query.contains("recursive=true"));
            TestResponse::json(
                200,
                r#"[
                    {"type":"file","path":"a.safetensors","size":11,"lfs":{"oid":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","size":2048}},
                    {"type":"file","path":"config.json","size":42,"oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                    {"type":"directory","path":"sub","size":0}
                ]"#,
            )
        })
        .await;
        let source = HfSource::with_base_url("gitee-ai", &server.base_url).unwrap();
        assert_eq!(source.name(), "gitee-ai");
        let files = source.list("Qwen/X", "main").await.unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.safetensors");
        assert_eq!(files[0].size, 2048);
        assert_eq!(files[0].sha256.as_deref(), Some(SHA));
        assert_eq!(files[1].path, "config.json");
        assert_eq!(files[1].size, 42);
        assert_eq!(files[1].sha256, None);
    }

    #[tokio::test]
    async fn hf_download_url_shape() {
        let server = spawn(|request| {
            if request.path.ends_with("/tree/main") {
                TestResponse::json(
                    200,
                    r#"[{"type":"file","path":"sub/a.gguf","size":4,"lfs":{"oid":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","size":4}}]"#,
                )
            } else {
                assert_eq!(request.path, "/Qwen/X/resolve/main/sub/a.gguf");
                TestResponse::ok(b"data".to_vec())
            }
        })
        .await;
        let source = HfSource::with_base_url("hf-mirror", &server.base_url).unwrap();
        let files = source.list("Qwen/X", "main").await.unwrap();
        assert_eq!(source.fetch(&files[0], None).await.unwrap(), b"data");
    }

    #[tokio::test]
    async fn rejects_unknown_hf_kind() {
        assert!(HfSource::new("huggingface").is_err());
        HfSource::new("hf-mirror").unwrap();
        HfSource::new("gitee-ai").unwrap();
    }
}
