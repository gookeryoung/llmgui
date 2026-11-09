//! Versioned remote catalog: bundled TOML, local cache, TTL refresh and tag
//! helpers. The catalog is metadata only; refreshing never downloads a model.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use llmgui_core::config::CatalogConfig;
use llmgui_core::{LlmguiError, Result};
use llmgui_models::GgufInfo;
use llmgui_models::library::{ModelEntry, ModelKind, ModelPrefs};
use llmgui_models::tags::{self, Tag};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CATALOG_SCHEMA: u32 = 2;
pub const MAX_ENTRIES: usize = 1_000;
const MAX_BYTES: u64 = 2 * 1024 * 1024;
const CACHE_FILE: &str = "cache.toml";
const STATE_FILE: &str = "state.json";
const LOCK_FILE: &str = "catalog.lock";
const LOCK_STALE: Duration = Duration::from_secs(60);
const LOCK_WAIT: Duration = Duration::from_secs(5);

const BUNDLED: &str = include_str!("../../../models/v0.toml");

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogDocument {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub models: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    pub repo: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub quant: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl CatalogDocument {
    pub fn empty() -> Self {
        Self {
            schema: CATALOG_SCHEMA,
            updated_at: None,
            models: Vec::new(),
        }
    }

    /// Parse and validate a catalog; unknown tag IDs are dropped for forward
    /// compatibility, structural errors reject the whole document.
    pub fn parse(text: &str) -> Result<Self> {
        let mut doc: Self = toml::from_str(text)?;
        doc.validate()?;
        for model in &mut doc.models {
            model.tags.retain(|tag| tags::is_known(tag));
            model.tags.sort();
            model.tags.dedup();
        }
        Ok(doc)
    }

    fn validate(&self) -> Result<()> {
        if !(1..=CATALOG_SCHEMA).contains(&self.schema) {
            return Err(LlmguiError::Config(format!(
                "unsupported catalog schema {} (expected 1..={CATALOG_SCHEMA})",
                self.schema
            )));
        }
        if self.models.len() > MAX_ENTRIES {
            return Err(LlmguiError::Config(format!(
                "catalog has {} entries (limit {MAX_ENTRIES})",
                self.models.len()
            )));
        }
        let mut ids = std::collections::BTreeSet::new();
        for model in &self.models {
            if model.id.trim().is_empty() || model.id.len() > 200 {
                return Err(LlmguiError::Config(
                    "catalog entry has an empty or oversized id".to_string(),
                ));
            }
            if !ids.insert(model.id.as_str()) {
                return Err(LlmguiError::Config(format!(
                    "duplicate catalog entry id {}",
                    model.id
                )));
            }
            if !valid_repo(&model.repo) {
                return Err(LlmguiError::Config(format!(
                    "catalog entry {} has an invalid repo",
                    model.id
                )));
            }
            if !valid_file(&model.file) {
                return Err(LlmguiError::Config(format!(
                    "catalog entry {} has an unsafe file path",
                    model.id
                )));
            }
        }
        Ok(())
    }
}

fn valid_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !owner.is_empty()
        && !name.is_empty()
        && [owner, name].iter().all(|part| {
            part.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

fn valid_file(file: &str) -> bool {
    if file.is_empty() {
        return true;
    }
    if file.len() > 512 || file.contains('\\') || file.starts_with('/') {
        return false;
    }
    let bytes = file.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    !Path::new(file).components().any(|part| {
        matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// Refresh state persisted in `state.json`; additive fields stay optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogState {
    #[serde(default = "state_schema")]
    schema: u32,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    last_modified: Option<String>,
    #[serde(default)]
    checked_at: Option<String>,
    #[serde(default)]
    fetched_at: Option<String>,
    #[serde(default)]
    catalog_schema: Option<u32>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    entries: usize,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_retry_at: Option<String>,
    #[serde(default)]
    failures: u32,
}

impl Default for CatalogState {
    fn default() -> Self {
        Self {
            schema: 1,
            url: None,
            etag: None,
            last_modified: None,
            checked_at: None,
            fetched_at: None,
            catalog_schema: None,
            sha256: None,
            entries: 0,
            last_error: None,
            next_retry_at: None,
            failures: 0,
        }
    }
}

fn state_schema() -> u32 {
    1
}

/// Public refresh metadata returned by `/admin/catalog`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CatalogMeta {
    pub schema: u32,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    pub stale: bool,
    pub refreshing: bool,
    pub auto_update: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
}

pub struct CatalogSnapshot {
    pub doc: Arc<CatalogDocument>,
    pub meta: CatalogMeta,
}

struct Inner {
    doc: Arc<CatalogDocument>,
    source: &'static str,
    state: CatalogState,
}

impl Inner {
    fn meta(&self, config: &CatalogConfig, refreshing: bool) -> CatalogMeta {
        CatalogMeta {
            schema: self.doc.schema,
            source: self.source.to_string(),
            updated_at: self.doc.updated_at.clone(),
            fetched_at: self.state.fetched_at.clone(),
            last_checked_at: self.state.checked_at.clone(),
            stale: is_stale(&self.state, config.ttl_hours),
            refreshing,
            auto_update: config.auto_update,
            error: self.state.last_error.clone(),
            skipped_reason: None,
        }
    }
}

pub struct CatalogStore {
    config: CatalogConfig,
    dir: PathBuf,
    inner: RwLock<Inner>,
    refreshing: AtomicBool,
    client: reqwest::Client,
}

impl CatalogStore {
    pub fn new(config: &CatalogConfig, home: impl Into<PathBuf>) -> Self {
        let mut config = config.clone();
        config.ttl_hours = config.ttl_hours.clamp(1, 168);
        config.timeout_secs = config.timeout_secs.clamp(1, 60);
        config.max_bytes = config.max_bytes.clamp(64 * 1024, MAX_BYTES);
        let dir = home.into().join("catalog");

        let bundled = Arc::new(CatalogDocument::parse(BUNDLED).unwrap_or_else(|error| {
            tracing::warn!(%error, "bundled catalog is invalid; using an empty catalog");
            CatalogDocument::empty()
        }));
        let state = read_state(&dir).unwrap_or_default();
        let mut doc = bundled;
        let mut source = "bundled";
        if let Ok(text) = fs::read_to_string(dir.join(CACHE_FILE)) {
            match CatalogDocument::parse(&text) {
                Ok(parsed) => {
                    doc = Arc::new(parsed);
                    source = "cache";
                }
                Err(error) => {
                    tracing::warn!(%error, "ignoring unreadable catalog cache");
                }
            }
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(concat!("llmgui/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config,
            dir,
            inner: RwLock::new(Inner { doc, source, state }),
            refreshing: AtomicBool::new(false),
            client,
        }
    }

    pub fn current(&self) -> CatalogSnapshot {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let meta = inner.meta(&self.config, self.refreshing.load(Ordering::SeqCst));
        CatalogSnapshot {
            doc: inner.doc.clone(),
            meta,
        }
    }

    /// Periodic entry point: no-op when auto update is disabled, when the
    /// cached copy is fresh, or while a retry backoff is active.
    pub async fn maybe_refresh(&self) {
        if !self.config.auto_update || self.config.url.trim().is_empty() {
            return;
        }
        if !self.needs_check() {
            return;
        }
        let _ = self.refresh(false).await;
    }

    fn needs_check(&self) -> bool {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Utc::now();
        if let Some(retry) = parse_time(inner.state.next_retry_at.as_deref())
            && now < retry
        {
            return false;
        }
        match parse_time(inner.state.checked_at.as_deref()) {
            Some(checked) => now >= checked + chrono::Duration::hours(self.config.ttl_hours as i64),
            None => true,
        }
    }

    /// Refresh the catalog. `force` bypasses TTL (manual refresh) but never
    /// bypasses `auto_update = false`, which is the network kill switch.
    pub async fn refresh(&self, force: bool) -> CatalogMeta {
        let mut meta = self.current().meta;
        if self.config.url.trim().is_empty() || (!self.config.auto_update && !force) {
            meta.skipped_reason = Some("disabled".to_string());
            return meta;
        }
        if !force && !meta.stale {
            meta.skipped_reason = Some("fresh".to_string());
            return meta;
        }
        if self.refreshing.swap(true, Ordering::SeqCst) {
            meta.refreshing = true;
            meta.skipped_reason = Some("refreshing".to_string());
            return meta;
        }
        let result = self.fetch_and_store().await;
        self.refreshing.store(false, Ordering::SeqCst);
        let mut meta = self.current().meta;
        if let Err(error) = result {
            let message = short_error(&error);
            self.record_failure(&message);
            meta.error = Some(message);
        }
        meta
    }

    async fn fetch_and_store(&self) -> Result<()> {
        let url = validate_url(&self.config.url)?;
        fs::create_dir_all(&self.dir)?;
        let _lock = CatalogLock::acquire(&self.dir)?;
        let (etag, last_modified) = {
            let inner = self
                .inner
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (inner.state.etag.clone(), inner.state.last_modified.clone())
        };
        let mut request = self.client.get(url.clone());
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = last_modified {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
        }
        let response = request
            .send()
            .await
            .map_err(|error| LlmguiError::Http(short_error(&error)))?;
        let etag = header_value(&response, reqwest::header::ETAG);
        let last_modified = header_value(&response, reqwest::header::LAST_MODIFIED);

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            self.record_success(etag, last_modified, None);
            return Ok(());
        }
        if !response.status().is_success() {
            return Err(LlmguiError::Http(format!(
                "catalog server returned {}",
                response.status()
            )));
        }
        if let Some(length) = response.content_length()
            && length > self.config.max_bytes
        {
            return Err(LlmguiError::Http(format!(
                "catalog response is {length} bytes (limit {})",
                self.config.max_bytes
            )));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| LlmguiError::Http(short_error(&error)))?;
            if body.len() + chunk.len() > self.config.max_bytes as usize {
                return Err(LlmguiError::Http(format!(
                    "catalog response exceeds {} bytes",
                    self.config.max_bytes
                )));
            }
            body.extend_from_slice(&chunk);
        }
        let text = std::str::from_utf8(&body)
            .map_err(|_| LlmguiError::Config("catalog is not valid UTF-8".to_string()))?;
        let doc = CatalogDocument::parse(text)?;
        write_atomic(&self.dir.join(CACHE_FILE), &body)?;
        let sha256 = hex::encode(Sha256::digest(&body));
        let entries = doc.models.len();
        self.record_success(etag, last_modified, Some((doc, sha256, entries)));
        Ok(())
    }

    fn record_success(
        &self,
        etag: Option<String>,
        last_modified: Option<String>,
        fetched: Option<(CatalogDocument, String, usize)>,
    ) {
        let now = llmgui_core::now_rfc3339();
        {
            let mut inner = self
                .inner
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.state.url = Some(self.config.url.clone());
            inner.state.etag = etag;
            inner.state.last_modified = last_modified;
            inner.state.checked_at = Some(now.clone());
            inner.state.last_error = None;
            inner.state.next_retry_at = None;
            inner.state.failures = 0;
            if let Some((doc, sha256, entries)) = fetched {
                inner.state.fetched_at = Some(now);
                inner.state.catalog_schema = Some(doc.schema);
                inner.state.sha256 = Some(sha256);
                inner.state.entries = entries;
                inner.doc = Arc::new(doc);
                inner.source = "remote";
            }
        }
        self.write_state();
    }

    fn record_failure(&self, message: &str) {
        let now = llmgui_core::now_rfc3339();
        {
            let mut inner = self
                .inner
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let failures = inner.state.failures.saturating_add(1);
            let backoff = match failures {
                1 => 30 * 60,
                2 => 2 * 60 * 60,
                _ => 6 * 60 * 60,
            };
            inner.state.failures = failures;
            inner.state.checked_at = Some(now);
            inner.state.last_error = Some(message.to_string());
            inner.state.next_retry_at =
                Some((Utc::now() + chrono::Duration::seconds(backoff)).to_rfc3339());
        }
        self.write_state();
    }

    fn write_state(&self) {
        let state = {
            let inner = self
                .inner
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.state.clone()
        };
        let Ok(bytes) = serde_json::to_vec_pretty(&state) else {
            return;
        };
        if let Err(error) = write_atomic(&self.dir.join(STATE_FILE), &bytes) {
            tracing::warn!(%error, "failed to persist catalog state");
        }
    }
}

fn header_value(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn validate_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|error| LlmguiError::Config(format!("invalid catalog url: {error}")))?;
    match url.scheme() {
        "https" => {}
        "http"
            if matches!(
                url.host_str(),
                Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
            ) => {}
        other => {
            return Err(LlmguiError::Config(format!(
                "catalog url scheme {other} is not allowed"
            )));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(LlmguiError::Config(
            "catalog url must not contain credentials".to_string(),
        ));
    }
    Ok(url)
}

fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn is_stale(state: &CatalogState, ttl_hours: u64) -> bool {
    let Some(fetched) = parse_time(state.fetched_at.as_deref()) else {
        return true;
    };
    Utc::now().signed_duration_since(fetched) >= chrono::Duration::hours(ttl_hours as i64)
}

fn read_state(dir: &Path) -> Option<CatalogState> {
    let text = fs::read_to_string(dir.join(STATE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.to_path_buf();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "catalog".to_string());
    tmp.set_file_name(format!("{name}.tmp"));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn short_error(error: &impl std::fmt::Display) -> String {
    error.to_string().chars().take(200).collect()
}

struct CatalogLock {
    path: PathBuf,
}

impl CatalogLock {
    fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join(LOCK_FILE);
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    if is_stale_lock(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(LlmguiError::Conflict(
                            "catalog refresh already running".to_string(),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for CatalogLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn is_stale_lock(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > LOCK_STALE)
}

/// Curated plus name-suggested tags for one catalog entry.
pub fn entry_tags(entry: &CatalogEntry) -> Vec<Tag> {
    let files = if entry.file.is_empty() {
        Vec::new()
    } else {
        vec![entry.file.clone()]
    };
    let suggested = tags::suggest(
        Some(&entry.repo),
        Some(&entry.id),
        &files,
        None,
        None,
        None,
        None,
    );
    tags::merge(&entry.tags, &suggested, &[], &[])
}

/// Lowercase repo -> curated tags, used to merge directory tags into installed
/// models without persisting them.
pub fn curated_map(doc: &CatalogDocument) -> BTreeMap<String, Vec<String>> {
    doc.models
        .iter()
        .map(|entry| (entry.repo.to_ascii_lowercase(), entry.tags.clone()))
        .collect()
}

/// Merge directory tags, local inference and user overrides for a model.
pub fn merged_model_tags(
    entry: &ModelEntry,
    curated: Option<&[String]>,
    metadata: Option<&GgufInfo>,
    prefs: &ModelPrefs,
) -> Vec<Tag> {
    let files: Vec<String> = entry.files.iter().map(|file| file.path.clone()).collect();
    let role = match entry.kind {
        ModelKind::Train => Some("train-base"),
        ModelKind::Inference => None,
    };
    let n_ctx_train = metadata
        .map(|info| info.n_ctx_train)
        .filter(|value| *value > 0)
        .or_else(|| (entry.context.train > 0).then_some(entry.context.train));
    let architecture = metadata
        .and_then(|info| info.architecture.as_deref())
        .or(entry.architecture.as_deref());
    let suggested = tags::suggest(
        entry.repo.as_deref(),
        Some(entry.id.as_str()),
        &files,
        architecture,
        n_ctx_train,
        role,
        metadata.map(|info| &info.metadata),
    );
    tags::merge(
        curated.unwrap_or_default(),
        &suggested,
        &prefs.tags,
        &prefs.hidden_tags,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    const CATALOG: &str = r#"
schema = 2
updated_at = "2026-11-09T00:00:00Z"

[[models]]
id = "qwen3-coder-30b-a3b-q4km"
repo = "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF"
file = "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf"
quant = "Q4_K_M"
license = "Apache-2.0"
size_bytes = 18556689568
role = "coder"
tags = ["coding", "chat", "moe"]
"#;

    struct FixtureResponse {
        status: u16,
        body: String,
        etag: Option<String>,
    }

    fn fixture(responses: Vec<FixtureResponse>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = [0u8; 8192];
                let _ = stream.read(&mut buffer);
                let reason = if response.status == 304 {
                    "Not Modified"
                } else {
                    "OK"
                };
                let mut head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    reason,
                    response.body.len()
                );
                if let Some(etag) = response.etag {
                    head.push_str(&format!("ETag: {etag}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(response.body.as_bytes());
            }
        });
        (format!("http://{addr}/catalog.toml"), handle)
    }

    fn ok(body: &str, etag: Option<&str>) -> FixtureResponse {
        FixtureResponse {
            status: 200,
            body: body.to_string(),
            etag: etag.map(str::to_string),
        }
    }

    fn config(url: &str) -> CatalogConfig {
        CatalogConfig {
            url: url.to_string(),
            timeout_secs: 2,
            ..CatalogConfig::default()
        }
    }

    #[test]
    fn bundled_catalog_is_schema_two_with_tags() {
        let doc = CatalogDocument::parse(BUNDLED).unwrap();
        assert_eq!(doc.schema, 2);
        assert!(doc.updated_at.is_some());
        let entry = doc
            .models
            .iter()
            .find(|entry| entry.id == "qwen3-coder-30b-a3b-q4km")
            .unwrap();
        assert!(entry.tags.contains(&"coding".to_string()));
        assert!(!entry.tags.contains(&"bogus".to_string()));
    }

    #[test]
    fn parses_schema_one_and_ignores_unknown_tags() {
        let doc = CatalogDocument::parse(
            r#"
schema = 1
[[models]]
id = "m"
repo = "owner/name"
tagged_unknown = true
tags = ["coding", "not-a-tag"]
"#,
        )
        .unwrap();
        assert_eq!(doc.schema, 1);
        assert_eq!(doc.models[0].tags, vec!["coding"]);
    }

    #[test]
    fn rejects_structural_problems() {
        assert!(CatalogDocument::parse("schema = 3\n").is_err());
        assert!(
            CatalogDocument::parse("schema = 2\n[[models]]\nid='x'\nrepo='bad repo'\n").is_err()
        );
        assert!(
            CatalogDocument::parse(
                "schema = 2\n[[models]]\nid='x'\nrepo='a/b'\nfile='../evil.gguf'\n"
            )
            .is_err()
        );
        assert!(
            CatalogDocument::parse(
                "schema = 2\n[[models]]\nid='x'\nrepo='a/b'\nfile='C:\\\\evil.gguf'\n"
            )
            .is_err()
        );
        let mut many = String::from("schema = 2\n");
        for index in 0..=MAX_ENTRIES {
            many.push_str(&format!("[[models]]\nid='m{index}'\nrepo='a/b{index}'\n"));
        }
        assert!(CatalogDocument::parse(&many).is_err());
    }

    #[test]
    fn rejects_http_for_public_hosts() {
        assert!(validate_url("http://example.com/catalog.toml").is_err());
        assert!(validate_url("ftp://example.com/catalog.toml").is_err());
        assert!(validate_url("https://example.com/catalog.toml").is_ok());
        assert!(validate_url("http://127.0.0.1:8080/catalog.toml").is_ok());
    }

    #[test]
    fn corrupt_cache_falls_back_to_bundled() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("catalog");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CACHE_FILE), "schema = 99").unwrap();
        let store = CatalogStore::new(&CatalogConfig::default(), home.path());
        let snapshot = store.current();
        assert_eq!(snapshot.meta.source, "bundled");
        assert!(!snapshot.doc.models.is_empty());
    }

    #[tokio::test]
    async fn refresh_success_writes_cache_and_uses_remote() {
        let (url, handle) = fixture(vec![ok(CATALOG, Some("\"v1\""))]);
        let home = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(&config(&url), home.path());
        let meta = store.refresh(false).await;
        assert_eq!(meta.source, "remote");
        assert!(meta.error.is_none(), "{meta:?}");
        assert!(!meta.stale);
        assert!(home.path().join("catalog/cache.toml").exists());
        assert!(home.path().join("catalog/state.json").exists());
        let snapshot = store.current();
        assert_eq!(snapshot.doc.models.len(), 1);
        assert!(snapshot.doc.models[0].tags.contains(&"coding".to_string()));
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn not_modified_keeps_existing_document() {
        let (url, handle) = fixture(vec![
            ok(CATALOG, Some("\"v1\"")),
            FixtureResponse {
                status: 304,
                body: String::new(),
                etag: Some("\"v1\"".to_string()),
            },
        ]);
        let home = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(&config(&url), home.path());
        let first = store.refresh(false).await;
        let first_fetched = first.fetched_at.clone();
        let second = store.refresh(true).await;
        assert!(second.error.is_none(), "{second:?}");
        assert_eq!(second.fetched_at, first_fetched);
        assert_eq!(store.current().doc.models.len(), 1);
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn failure_keeps_old_document_and_records_error() {
        let (url, handle) = fixture(vec![
            ok(CATALOG, Some("\"v1\"")),
            FixtureResponse {
                status: 500,
                body: "boom".to_string(),
                etag: None,
            },
        ]);
        let home = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(&config(&url), home.path());
        assert_eq!(store.refresh(false).await.source, "remote");
        let meta = store.refresh(true).await;
        assert!(meta.error.is_some());
        assert_eq!(store.current().doc.models.len(), 1);
        let state = read_state(&home.path().join("catalog")).unwrap();
        assert!(state.last_error.is_some());
        assert!(state.next_retry_at.is_some());
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn auto_update_disabled_skips_without_network() {
        let cfg = CatalogConfig {
            auto_update: false,
            url: "http://127.0.0.1:1/catalog.toml".to_string(),
            timeout_secs: 1,
            ..CatalogConfig::default()
        };
        let home = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(&cfg, home.path());
        let meta = store.refresh(false).await;
        assert_eq!(meta.skipped_reason.as_deref(), Some("disabled"));
        assert!(meta.error.is_none());
        assert_eq!(store.current().meta.source, "bundled");
    }

    #[tokio::test]
    async fn http_public_url_is_rejected() {
        let cfg = config("http://example.com/catalog.toml");
        let home = tempfile::tempdir().unwrap();
        let store = CatalogStore::new(&cfg, home.path());
        let meta = store.refresh(true).await;
        assert!(
            meta.error
                .as_deref()
                .is_some_and(|error| error.contains("not allowed"))
        );
    }

    #[test]
    fn entry_tags_include_name_suggestions_and_runtime_flag() {
        let entry = CatalogEntry {
            id: "qwen3-coder-30b-a3b-q4km".to_string(),
            repo: "org/model".to_string(),
            file: "mmproj-vision.gguf".to_string(),
            quant: "Q4_K_M".to_string(),
            license: "Apache-2.0".to_string(),
            size_bytes: 1,
            role: "coder".to_string(),
            tags: vec!["coding".to_string()],
        };
        let tags = entry_tags(&entry);
        let ids: Vec<&str> = tags.iter().map(|tag| tag.id.as_str()).collect();
        assert!(ids.contains(&"coding"));
        assert!(ids.contains(&"multimodal"));
        let multimodal = tags.iter().find(|tag| tag.id == "multimodal").unwrap();
        assert!(!multimodal.runtime_supported);
    }
}
