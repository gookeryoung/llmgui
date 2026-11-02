//! Persistent download jobs. The existing pull pipeline does the transfer;
//! this module adds a queue, state transitions, progress snapshots and SSE.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use llmgui_core::config::AppConfig;
use llmgui_core::{LlmguiError, Result};
use llmgui_download::{DownloadOptions, Source};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::library::LibraryState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Resolving,
    Downloading,
    Verifying,
    Installed,
    Failed,
    Paused,
    Cancelled,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Installed | Self::Failed | Self::Cancelled | Self::Paused
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: String,
    pub repo: String,
    pub revision: String,
    pub source: String,
    #[serde(default)]
    pub files: Vec<String>,
    pub state: JobState,
    pub downloaded: u64,
    pub total: u64,
    pub bytes_per_sec: f64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl JobSnapshot {
    pub fn progress(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.downloaded as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }
}

struct Job {
    snapshot: Mutex<JobSnapshot>,
    cancel_kind: AtomicU8,
    cancel: AtomicBool,
    notify: Notify,
}

impl Job {
    fn snapshot(&self) -> JobSnapshot {
        self.snapshot.lock().expect("job lock").clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadRequest {
    pub repo: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

pub struct DownloadManager {
    jobs: Mutex<BTreeMap<String, Arc<Job>>>,
    home: PathBuf,
    library: Arc<LibraryState>,
    config: Arc<AppConfig>,
}

impl DownloadManager {
    pub fn new(home: PathBuf, library: Arc<LibraryState>, config: Arc<AppConfig>) -> Self {
        let manager = Self {
            jobs: Mutex::new(BTreeMap::new()),
            home,
            library,
            config,
        };
        manager.load_persisted();
        manager
    }

    fn jobs_dir(&self) -> PathBuf {
        self.home.join("downloads")
    }

    fn job_path(&self, id: &str) -> PathBuf {
        self.jobs_dir().join(format!("{id}.json"))
    }

    fn load_persisted(&self) {
        let Ok(entries) = std::fs::read_dir(self.jobs_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut snapshot) = serde_json::from_str::<JobSnapshot>(&text) else {
                continue;
            };
            if !snapshot.state.is_terminal() {
                snapshot.state = JobState::Paused;
                snapshot.error = Some("interrupted; resume to continue".to_string());
            }
            let job = Arc::new(Job {
                snapshot: Mutex::new(snapshot.clone()),
                cancel_kind: AtomicU8::new(0),
                cancel: AtomicBool::new(false),
                notify: Notify::new(),
            });
            self.jobs
                .lock()
                .expect("jobs lock")
                .insert(snapshot.id, job);
        }
    }

    pub fn create(self: &Arc<Self>, request: DownloadRequest) -> Result<JobSnapshot> {
        let repo = request.repo.trim();
        if repo.is_empty() {
            return Err(LlmguiError::Config("repo is empty".to_string()));
        }
        let revision = request
            .revision
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("master")
            .to_string();
        let source = request
            .source
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("auto")
            .to_string();
        let now = now();
        let snapshot = JobSnapshot {
            id: format!("dl-{}", uuid::Uuid::new_v4()),
            repo: repo.to_string(),
            revision,
            source,
            files: request.files.clone(),
            state: JobState::Queued,
            downloaded: 0,
            total: 0,
            bytes_per_sec: 0.0,
            error: None,
            model_id: None,
            created_at: now,
            updated_at: now,
        };
        let job = Arc::new(Job {
            snapshot: Mutex::new(snapshot.clone()),
            cancel_kind: AtomicU8::new(0),
            cancel: AtomicBool::new(false),
            notify: Notify::new(),
        });
        self.jobs
            .lock()
            .expect("jobs lock")
            .insert(snapshot.id.clone(), job.clone());
        self.persist(&job);
        let manager = Arc::clone(self);
        let request = DownloadRequest {
            repo: repo.to_string(),
            revision: Some(snapshot.revision.clone()),
            source: Some(snapshot.source.clone()),
            file: request.file,
            files: request.files,
        };
        tokio::spawn(async move {
            manager.run(job, request).await;
        });
        Ok(snapshot)
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        let mut jobs: Vec<JobSnapshot> = self
            .jobs
            .lock()
            .expect("jobs lock")
            .values()
            .map(|job| job.snapshot())
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
        jobs
    }

    pub fn get(&self, id: &str) -> Option<JobSnapshot> {
        self.jobs
            .lock()
            .expect("jobs lock")
            .get(id)
            .map(|job| job.snapshot())
    }

    pub fn control(self: &Arc<Self>, id: &str, action: &str) -> Result<JobSnapshot> {
        let job = self
            .jobs
            .lock()
            .expect("jobs lock")
            .get(id)
            .cloned()
            .ok_or_else(|| LlmguiError::NotFound(format!("download job {id}")))?;
        let snapshot = job.snapshot();
        match action {
            "pause" => {
                if snapshot.state.is_terminal() && snapshot.state != JobState::Paused {
                    return Err(LlmguiError::Conflict(format!(
                        "cannot pause a {:?} job",
                        snapshot.state
                    )));
                }
                job.cancel_kind.store(1, Ordering::SeqCst);
                job.cancel.store(true, Ordering::SeqCst);
                job.notify.notify_waiters();
                self.set_state(&job, JobState::Paused, None);
            }
            "cancel" => {
                if snapshot.state.is_terminal() && snapshot.state != JobState::Paused {
                    return Err(LlmguiError::Conflict(format!(
                        "cannot cancel a {:?} job",
                        snapshot.state
                    )));
                }
                job.cancel_kind.store(2, Ordering::SeqCst);
                job.cancel.store(true, Ordering::SeqCst);
                job.notify.notify_waiters();
                self.set_state(&job, JobState::Cancelled, None);
            }
            "resume" | "retry" => {
                if !matches!(
                    snapshot.state,
                    JobState::Paused | JobState::Cancelled | JobState::Failed
                ) {
                    return Err(LlmguiError::Conflict(format!(
                        "cannot resume a {:?} job",
                        snapshot.state
                    )));
                }
                job.cancel_kind.store(0, Ordering::SeqCst);
                job.cancel.store(false, Ordering::SeqCst);
                {
                    let mut record = job.snapshot.lock().expect("job lock");
                    record.state = JobState::Queued;
                    record.error = None;
                }
                self.persist(&job);
                let manager = Arc::clone(self);
                let request = DownloadRequest {
                    repo: snapshot.repo.clone(),
                    revision: Some(snapshot.revision.clone()),
                    source: Some(snapshot.source.clone()),
                    file: None,
                    files: snapshot.files.clone(),
                };
                let job_task = Arc::clone(&job);
                tokio::spawn(async move {
                    manager.run(job_task, request).await;
                });
            }
            other => {
                return Err(LlmguiError::Config(format!(
                    "unknown download action {other}"
                )));
            }
        }
        Ok(job.snapshot())
    }

    async fn run(&self, job: Arc<Job>, request: DownloadRequest) {
        self.set_state(&job, JobState::Resolving, None);
        let source = match self.build_source(&job.snapshot(), &request).await {
            Ok(source) => source,
            Err(error) => {
                self.set_state(&job, JobState::Failed, Some(error.to_string()));
                return;
            }
        };
        if job.cancel.load(Ordering::SeqCst) {
            self.finish_cancelled(&job);
            return;
        }
        self.set_state(&job, JobState::Downloading, None);
        let options = DownloadOptions {
            revision: request.revision.clone().unwrap_or_else(|| "master".into()),
            file: request.file.clone(),
            files: request.files.clone(),
            concurrency: self.config.download.concurrency,
            chunk_size: self.config.download.chunk_size_mb * 1024 * 1024,
            cache_dir: self.config.download.cache_dir.clone(),
        };
        let repo = request.repo.clone();
        let progress_job = job.clone();
        let progress: Box<dyn Fn(u64, u64) + Send + Sync> = Box::new(move |done, total| {
            let mut record = progress_job.snapshot.lock().expect("job lock");
            let now = now();
            let delta_bytes = done.saturating_sub(record.downloaded);
            let delta_secs = now.saturating_sub(record.updated_at).max(1);
            record.downloaded = done;
            record.total = total;
            if delta_bytes > 0 {
                record.bytes_per_sec = delta_bytes as f64 / delta_secs as f64;
            }
            record.updated_at = now;
        });
        let pull = llmgui_download::pull(source.as_ref(), &repo, &options, Some(progress));
        tokio::pin!(pull);
        let outcome = tokio::select! {
            result = &mut pull => result,
            _ = job.notify.notified() => {
                if job.cancel.load(Ordering::SeqCst) {
                    let state = if job.cancel_kind.load(Ordering::SeqCst) == 1 {
                        JobState::Paused
                    } else {
                        JobState::Cancelled
                    };
                    self.set_state(&job, state, None);
                    return;
                }
                Err(LlmguiError::Cancelled)
            }
        };
        match outcome {
            Ok(manifest) => {
                if job.cancel.load(Ordering::SeqCst) {
                    self.finish_cancelled(&job);
                    return;
                }
                self.set_state(&job, JobState::Verifying, None);
                match self.library.refresh() {
                    Ok((registry, _)) => {
                        let model_id = registry
                            .models
                            .iter()
                            .find(|model| {
                                model.path.file_name().and_then(|name| name.to_str())
                                    == Some(manifest.id.as_str())
                            })
                            .or_else(|| {
                                registry
                                    .models
                                    .iter()
                                    .find(|model| model.repo.as_deref() == Some(repo.as_str()))
                            })
                            .map(|model| model.id.clone())
                            .unwrap_or_else(|| manifest.id.clone());
                        if let Some(record) =
                            self.jobs.lock().expect("jobs lock").get(&job.snapshot().id)
                        {
                            record.snapshot.lock().expect("job lock").model_id = Some(model_id);
                        }
                        self.set_state(&job, JobState::Installed, None);
                    }
                    Err(error) => {
                        self.set_state(&job, JobState::Failed, Some(error.to_string()));
                    }
                }
            }
            Err(error) => {
                if job.cancel.load(Ordering::SeqCst) {
                    let state = if job.cancel_kind.load(Ordering::SeqCst) == 1 {
                        JobState::Paused
                    } else {
                        JobState::Cancelled
                    };
                    self.set_state(&job, state, None);
                } else if matches!(error, LlmguiError::Cancelled) {
                    self.set_state(&job, JobState::Cancelled, None);
                } else {
                    self.set_state(&job, JobState::Failed, Some(error.to_string()));
                }
            }
        }
    }

    fn finish_cancelled(&self, job: &Arc<Job>) {
        let state = if job.cancel_kind.load(Ordering::SeqCst) == 1 {
            JobState::Paused
        } else {
            JobState::Cancelled
        };
        self.set_state(job, state, None);
    }

    async fn build_source(
        &self,
        snapshot: &JobSnapshot,
        request: &DownloadRequest,
    ) -> Result<Box<dyn Source>> {
        match snapshot.source.as_str() {
            "modelscope" => Ok(Box::new(llmgui_download::ModelScopeSource::new()?)),
            "hf-mirror" | "gitee-ai" => {
                Ok(Box::new(llmgui_download::HfSource::new(&snapshot.source)?))
            }
            "auto" => {
                let modelscope = llmgui_download::ModelScopeSource::new()?;
                match modelscope.list(&request.repo, &snapshot.revision).await {
                    Ok(_) => Ok(Box::new(modelscope)),
                    Err(error) => {
                        tracing::warn!(%error, repo = %request.repo, "ModelScope unavailable; using hf-mirror");
                        Ok(Box::new(llmgui_download::HfSource::new("hf-mirror")?))
                    }
                }
            }
            other => Err(LlmguiError::Config(format!(
                "unknown source {other}; use auto|modelscope|hf-mirror|gitee-ai"
            ))),
        }
    }

    fn set_state(&self, job: &Arc<Job>, state: JobState, error: Option<String>) {
        {
            let mut record = job.snapshot.lock().expect("job lock");
            record.state = state;
            record.updated_at = now();
            record.error = error;
        }
        self.persist(job);
    }

    fn persist(&self, job: &Arc<Job>) {
        let snapshot = job.snapshot();
        if let Err(error) = std::fs::create_dir_all(self.jobs_dir()) {
            tracing::warn!(%error, "cannot create downloads directory");
            return;
        }
        let path = self.job_path(&snapshot.id);
        let tmp = path.with_extension("json.tmp");
        match serde_json::to_vec_pretty(&snapshot) {
            Ok(bytes) => {
                if let Err(error) =
                    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &path))
                {
                    tracing::warn!(%error, job = %snapshot.id, "cannot persist download job");
                }
            }
            Err(error) => tracing::warn!(%error, "cannot serialise download job"),
        }
    }
}

fn now() -> u64 {
    llmgui_core::now_unix().max(0) as u64
}
