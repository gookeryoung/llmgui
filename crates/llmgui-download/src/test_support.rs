//! Test-only helpers: a tiny HTTP/1.1 fixture server and an in-memory
//! [`Source`] implementation. No public network access is required.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use llmgui_core::{LlmguiError, Result};

use crate::source::{ContentRange, FetchedChunk, RemoteFile, Source};

// ---------------------------------------------------------------------------
// Minimal HTTP fixture
// ---------------------------------------------------------------------------

pub struct TestRequest {
    pub path: String,
    pub query: String,
    headers: HashMap<String, String>,
}

impl TestRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

pub struct TestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body,
        }
    }

    pub fn ok(body: Vec<u8>) -> Self {
        Self::new(200, body)
    }

    pub fn json(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.into(),
        }
    }

    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }
}

pub struct TestServer {
    pub base_url: String,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Spawn a fixture HTTP server on an ephemeral loopback port.
pub async fn spawn<F>(handler: F) -> TestServer
where
    F: Fn(TestRequest) -> TestResponse + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handler = Arc::new(handler);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut scratch = [0u8; 2048];
                loop {
                    let read = match socket.read(&mut scratch).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => read,
                    };
                    buffer.extend_from_slice(&scratch[..read]);
                    if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&buffer);
                let mut lines = text.split("\r\n");
                let request_line = lines.next().unwrap_or_default();
                let mut parts = request_line.split_whitespace();
                let _method = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                let (path, query) = match target.split_once('?') {
                    Some((path, query)) => (path.to_string(), query.to_string()),
                    None => (target, String::new()),
                };
                let mut headers = HashMap::new();
                for line in lines {
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                let response = handler(TestRequest {
                    path,
                    query,
                    headers,
                });
                let reason = match response.status {
                    200 => "OK",
                    206 => "Partial Content",
                    404 => "Not Found",
                    416 => "Range Not Satisfiable",
                    _ => "Response",
                };
                let mut head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    reason,
                    response.body.len()
                );
                for (name, value) in &response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let mut bytes = head.into_bytes();
                bytes.extend_from_slice(&response.body);
                let _ = socket.write_all(&bytes).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    TestServer {
        base_url: format!("http://{address}"),
        handle,
    }
}

/// Build a `200`/`206` response honouring a single `Range` header.
pub fn ranged_response(body: &[u8], range: Option<&str>) -> TestResponse {
    let Some(range) = range else {
        return TestResponse::ok(body.to_vec());
    };
    let Some(spec) = range.trim().strip_prefix("bytes=") else {
        return TestResponse::new(416, Vec::new());
    };
    let Some((start, end)) = spec.split_once('-') else {
        return TestResponse::new(416, Vec::new());
    };
    let total = body.len() as u64;
    let Ok(start) = start.trim().parse::<u64>() else {
        return TestResponse::new(416, Vec::new());
    };
    if start >= total {
        return TestResponse::new(416, Vec::new());
    }
    let end = match end.trim() {
        "" => total - 1,
        value => value.parse::<u64>().unwrap_or(total - 1).min(total - 1),
    };
    TestResponse::new(206, body[start as usize..=end as usize].to_vec())
        .with_header("Content-Range", format!("bytes {start}-{end}/{total}"))
}

// ---------------------------------------------------------------------------
// In-memory source
// ---------------------------------------------------------------------------

/// In-memory [`Source`] used to unit-test the downloader without HTTP.
pub struct FakeSource {
    name: String,
    files: Vec<(RemoteFile, Vec<u8>)>,
    ranges: Mutex<Vec<Option<Range<u64>>>>,
    attempts: AtomicUsize,
    fail_first: AtomicUsize,
    ignore_range: bool,
    listed: Mutex<bool>,
}

impl FakeSource {
    pub fn new(files: Vec<(RemoteFile, Vec<u8>)>) -> Self {
        Self {
            name: "fake".to_string(),
            files,
            ranges: Mutex::new(Vec::new()),
            attempts: AtomicUsize::new(0),
            fail_first: AtomicUsize::new(0),
            ignore_range: false,
            listed: Mutex::new(false),
        }
    }

    /// Make the next `count` fetch calls fail with a transient HTTP error.
    pub fn fail_first(mut self, count: usize) -> Self {
        self.fail_first = AtomicUsize::new(count);
        self
    }

    /// Answer every ranged request with the whole body and `200 OK`.
    pub fn ignore_range(mut self) -> Self {
        self.ignore_range = true;
        self
    }

    pub fn fetch_count(&self) -> usize {
        self.ranges.lock().unwrap().len()
    }

    /// Total fetch attempts, including failed ones.
    pub fn attempt_count(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }

    pub fn requested_ranges(&self) -> Vec<Option<Range<u64>>> {
        self.ranges.lock().unwrap().clone()
    }

    fn lookup(&self, path: &str) -> Option<&(RemoteFile, Vec<u8>)> {
        self.files.iter().find(|(file, _)| file.path == path)
    }
}

#[async_trait]
impl Source for FakeSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn list(&self, _repo: &str, _revision: &str) -> Result<Vec<RemoteFile>> {
        *self.listed.lock().unwrap() = true;
        Ok(self.files.iter().map(|(file, _)| file.clone()).collect())
    }

    async fn fetch(&self, file: &RemoteFile, range: Option<Range<u64>>) -> Result<Vec<u8>> {
        Ok(self.fetch_chunk(file, range).await?.data)
    }

    async fn fetch_chunk(
        &self,
        file: &RemoteFile,
        range: Option<Range<u64>>,
    ) -> Result<FetchedChunk> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_first.load(Ordering::SeqCst) > 0 {
            self.fail_first.fetch_sub(1, Ordering::SeqCst);
            return Err(LlmguiError::Http("transient fixture failure".to_string()));
        }
        let Some((_, data)) = self.lookup(&file.path) else {
            return Err(LlmguiError::NotFound(file.path.clone()));
        };
        self.ranges.lock().unwrap().push(range.clone());
        let Some(range) = range else {
            return Ok(FetchedChunk {
                data: data.clone(),
                status: 200,
                content_range: None,
                etag: Some("fixture-etag".to_string()),
                range_ignored: false,
            });
        };
        if self.ignore_range {
            return Ok(FetchedChunk {
                data: data.clone(),
                status: 200,
                content_range: None,
                etag: Some("fixture-etag".to_string()),
                range_ignored: true,
            });
        }
        let start = range.start as usize;
        let end = (range.end as usize).min(data.len());
        let slice = data.get(start..end).unwrap_or_default().to_vec();
        let content_range = Some(ContentRange {
            start: range.start,
            end: range.end.saturating_sub(1),
            total: Some(data.len() as u64),
        });
        Ok(FetchedChunk {
            data: slice,
            status: 206,
            content_range,
            etag: Some("fixture-etag".to_string()),
            range_ignored: false,
        })
    }
}
