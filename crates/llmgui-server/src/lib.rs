//! OpenAI-compatible local HTTP server plus the static GUI.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::Stream;
use llmgui_backend_api::{
    Backend, GenerateRequest, GenerateStats, TokenEvent, default_chat_prompt,
};
use llmgui_core::api::{
    ChatChoice, ChatChunkChoice, ChatCompletionChunk, ChatCompletionRequest,
    ChatCompletionResponse, ChatDelta, ChatMessage, CompletionChoice, CompletionRequest,
    CompletionResponse, ModelList, ModelObject, OpenAiErrorBody, OpenAiErrorDetail, Usage,
};
use llmgui_core::config::AppConfig;
use llmgui_core::paths;
use llmgui_core::{LlmguiError, Result, now_unix};
use llmgui_scheduler::Scheduler;

pub mod admin;
pub mod catalog;
pub mod downloads;
pub mod library;
pub mod runtime;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedReceiver;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub scheduler: Arc<Scheduler>,
    pub model_id: String,
    pub backend_name: String,
    pub started: Instant,
    pub metrics: Arc<Metrics>,
    pub library: Arc<library::LibraryState>,
    pub runtime: Arc<runtime::Runtime>,
    pub downloads: Arc<downloads::DownloadManager>,
    pub catalog: Arc<catalog::CatalogStore>,
}

#[derive(Default)]
pub struct Metrics {
    pub requests: AtomicU64,
    pub errors: AtomicU64,
    pub prompt_tokens: AtomicU64,
    pub completion_tokens: AtomicU64,
    pub prefix_cache_hits: AtomicU64,
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
    backend: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    loaded: Option<String>,
    uptime_secs: u64,
    slots_available: usize,
}

pub fn router(state: AppState, gui_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/healthz", get(health))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/completions", post(completions))
        .route("/metrics", get(metrics))
        .merge(admin::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));
    let mut router = api.with_state(state);
    if let Some(dir) = gui_dir {
        router = router.fallback_service(ServeDir::new(dir).append_index_html_on_directories(true));
    }
    router.layer(TraceLayer::new_for_http())
}

/// Bind and serve until the process is terminated.
pub async fn serve(state: AppState, gui_dir: Option<PathBuf>) -> Result<()> {
    let addr = format!("{}:{}", state.config.server.host, state.config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| LlmguiError::Config(format!("bind {addr}: {e}")))?;
    tracing::info!(address = %addr, "llmgui server listening");
    let catalog = state.catalog.clone();
    tokio::spawn(async move {
        catalog.maybe_refresh().await;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
            catalog.maybe_refresh().await;
        }
    });
    let idle_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            match idle_state.runtime.maybe_unload_idle().await {
                Ok(Some(id)) => tracing::info!(model = %id, "unloaded idle model (TTL)"),
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "idle unload failed"),
            }
        }
    });
    axum::serve(listener, router(state, gui_dir))
        .await
        .map_err(|e| LlmguiError::Backend(format!("server error: {e}")))?;
    Ok(())
}

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = state.config.server.api_key.as_deref() else {
        return next.run(request).await;
    };
    if request.uri().path() == "/healthz" {
        return next.run(request).await;
    }
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if provided == Some(expected) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(OpenAiErrorBody {
                error: OpenAiErrorDetail {
                    message: "invalid API key".to_string(),
                    kind: "authentication_error".to_string(),
                    code: "invalid_api_key".to_string(),
                },
            }),
        )
            .into_response()
    }
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    let loaded = state.runtime.loaded_id().await;
    Json(Health {
        status: "ok",
        version: llmgui_core::VERSION,
        backend: state.backend_name.clone(),
        model: loaded.clone().unwrap_or_else(|| state.model_id.clone()),
        loaded,
        uptime_secs: state.started.elapsed().as_secs(),
        slots_available: state.scheduler.available_slots(),
    })
}

async fn list_models(State(state): State<AppState>) -> Json<ModelList> {
    let loaded = state.runtime.loaded().await;
    let id = loaded
        .as_ref()
        .map(|model| model.id.clone())
        .unwrap_or_else(|| state.model_id.clone());
    Json(ModelList {
        object: "list".to_string(),
        data: vec![ModelObject {
            id,
            object: "model".to_string(),
            created: now_unix(),
            owned_by: "llmgui".to_string(),
        }],
    })
}

async fn metrics(State(state): State<AppState>) -> String {
    let m = &state.metrics;
    format!(
        "# HELP llmgui_requests_total Total API requests\n\
         # TYPE llmgui_requests_total counter\n\
         llmgui_requests_total {}\n\
         # TYPE llmgui_errors_total counter\n\
         llmgui_errors_total {}\n\
         # TYPE llmgui_prompt_tokens_total counter\n\
         llmgui_prompt_tokens_total {}\n\
         # TYPE llmgui_completion_tokens_total counter\n\
         llmgui_completion_tokens_total {}\n\
         # TYPE llmgui_prefix_cache_hits_total counter\n\
         llmgui_prefix_cache_hits_total {}\n\
         # TYPE llmgui_slots_available gauge\n\
         llmgui_slots_available {}\n",
        m.requests.load(Ordering::Relaxed),
        m.errors.load(Ordering::Relaxed),
        m.prompt_tokens.load(Ordering::Relaxed),
        m.completion_tokens.load(Ordering::Relaxed),
        m.prefix_cache_hits.load(Ordering::Relaxed),
        state.scheduler.available_slots(),
    )
}

async fn chat_completions(
    State(state): State<AppState>,
    Json(request): Json<ChatCompletionRequest>,
) -> Response {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    if request.messages.is_empty() {
        return ApiError(LlmguiError::Config(
            "messages must not be empty".to_string(),
        ))
        .into_response();
    }
    let prompt = default_chat_prompt(&request.messages);
    let request_id = format!("chatcmpl-{}", Uuid::new_v4());
    let generate = GenerateRequest {
        request_id: request_id.clone(),
        prompt: prompt.clone(),
        max_tokens: request.token_limit(),
        temperature: request.temperature,
        top_p: request.top_p,
        stop: request.stops(),
        seed: request.seed,
        cache_key: cache_key(&prompt),
    };
    stream_or_collect(
        state,
        generate,
        request.model,
        request_id,
        request.stream,
        true,
    )
    .await
}

async fn completions(
    State(state): State<AppState>,
    Json(request): Json<CompletionRequest>,
) -> Response {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let prompt = request.prompt.first_text();
    if prompt.is_empty() {
        return ApiError(LlmguiError::Config("prompt must not be empty".to_string()))
            .into_response();
    }
    let request_id = format!("cmpl-{}", Uuid::new_v4());
    let generate = GenerateRequest {
        request_id: request_id.clone(),
        prompt: prompt.clone(),
        max_tokens: request.max_tokens.unwrap_or(512).clamp(1, 32768),
        temperature: request.temperature,
        top_p: request.top_p,
        stop: request
            .stop
            .as_ref()
            .map(|s| s.as_vec())
            .unwrap_or_default(),
        seed: request.seed,
        cache_key: cache_key(&prompt),
    };
    stream_or_collect(
        state,
        generate,
        request.model,
        request_id,
        request.stream,
        false,
    )
    .await
}

async fn resolve_model(
    state: &AppState,
    requested: &str,
) -> Result<Option<Arc<runtime::LoadedModel>>> {
    let requested = requested.trim();
    let registry = state.library.registry()?;
    let loaded = state.runtime.loaded().await;
    if let Some(loaded) = &loaded {
        let alias_matches = registry.alias_of(&loaded.id).as_deref() == Some(requested);
        if requested.is_empty() || requested == "default" || requested == loaded.id || alias_matches
        {
            loaded.touch();
            return Ok(Some(loaded.clone()));
        }
    }
    match registry.resolve(requested) {
        llmgui_models::library::Resolve::One(id) => {
            let entry = registry
                .get(&id)
                .cloned()
                .ok_or_else(|| LlmguiError::NotFound(format!("model {id}")))?;
            let defaults = registry.prefs(&id).load_defaults;
            let loaded = state
                .runtime
                .ensure_loaded(&entry, defaults, state.runtime.vram_budget_bytes())
                .await?;
            Ok(Some(loaded))
        }
        llmgui_models::library::Resolve::Ambiguous(matches) => Err(LlmguiError::Conflict(format!(
            "model name is ambiguous: {}",
            matches.join(", ")
        ))),
        llmgui_models::library::Resolve::None => {
            if loaded.is_none()
                && (requested.is_empty()
                    || requested == "default"
                    || requested == state.model_id
                    || state.backend_name == "mock"
                    || state.backend_name == "upstream")
            {
                Ok(None)
            } else {
                Err(LlmguiError::NotFound(format!("model {requested}")))
            }
        }
    }
}

async fn stream_or_collect(
    state: AppState,
    request: GenerateRequest,
    model: String,
    request_id: String,
    stream: bool,
    chat: bool,
) -> Response {
    let resolved = match resolve_model(&state, &model).await {
        Ok(resolved) => resolved,
        Err(error) => {
            state.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return ApiError(error).into_response();
        }
    };
    let model = resolved
        .as_ref()
        .map(|loaded| loaded.id.clone())
        .unwrap_or(model);
    let guard = match state.runtime.begin_request() {
        Ok(guard) => guard,
        Err(error) => {
            state.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return ApiError(error).into_response();
        }
    };
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut rx = state.scheduler.enqueue(request, cancel);
    if stream {
        let metrics = state.metrics.clone();
        let body = sse_stream(rx, model, request_id, chat, metrics, guard);
        Sse::new(body)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        let mut text = String::new();
        let mut stats: Option<GenerateStats> = None;
        let mut error: Option<LlmguiError> = None;
        while let Some(event) = rx.recv().await {
            match event {
                TokenEvent::Token { text: token, .. } => text.push_str(&token),
                TokenEvent::Done { stats: s } => stats = Some(s),
                TokenEvent::Error { message, code } => {
                    error = Some(LlmguiError::Backend(format!("{code}: {message}")));
                    break;
                }
            }
        }
        drop(guard);
        if let Some(error) = error {
            state.metrics.errors.fetch_add(1, Ordering::Relaxed);
            return ApiError(error).into_response();
        }
        let stats = stats.unwrap_or_else(|| {
            GenerateStats::finish(
                0,
                text.chars().count() as u32,
                Instant::now(),
                None,
                false,
                "stop",
            )
        });
        state
            .metrics
            .prompt_tokens
            .fetch_add(u64::from(stats.prompt_tokens), Ordering::Relaxed);
        state
            .metrics
            .completion_tokens
            .fetch_add(u64::from(stats.completion_tokens), Ordering::Relaxed);
        if stats.prefix_cache_hit {
            state
                .metrics
                .prefix_cache_hits
                .fetch_add(1, Ordering::Relaxed);
        }
        let usage = Usage {
            prompt_tokens: stats.prompt_tokens,
            completion_tokens: stats.completion_tokens,
            total_tokens: stats.prompt_tokens + stats.completion_tokens,
        };
        if chat {
            Json(ChatCompletionResponse {
                id: request_id,
                object: "chat.completion".to_string(),
                created: now_unix(),
                model,
                choices: vec![ChatChoice {
                    index: 0,
                    message: ChatMessage::new("assistant", text),
                    finish_reason: Some(stats.finish_reason),
                }],
                usage,
            })
            .into_response()
        } else {
            Json(CompletionResponse {
                id: request_id,
                object: "text_completion".to_string(),
                created: now_unix(),
                model,
                choices: vec![CompletionChoice {
                    index: 0,
                    text,
                    finish_reason: Some(stats.finish_reason),
                }],
                usage,
            })
            .into_response()
        }
    }
}

fn sse_stream(
    mut rx: UnboundedReceiver<TokenEvent>,
    model: String,
    request_id: String,
    chat: bool,
    metrics: Arc<Metrics>,
    _guard: runtime::RequestGuard,
) -> impl Stream<Item = std::result::Result<Event, Infallible>> {
    async_stream::stream! {
        let created = now_unix();
        if chat {
            let initial = ChatCompletionChunk {
                id: request_id.clone(),
                object: "chat.completion.chunk".to_string(),
                created,
                model: model.clone(),
                choices: vec![ChatChunkChoice {
                    index: 0,
                    delta: ChatDelta { role: Some("assistant".to_string()), content: None },
                    finish_reason: None,
                }],
                usage: None,
            };
            yield Ok(sse_json(&initial));
        }
        while let Some(event) = rx.recv().await {
            match event {
                TokenEvent::Token { text, .. } => {
                    if chat {
                        let chunk = ChatCompletionChunk {
                            id: request_id.clone(),
                            object: "chat.completion.chunk".to_string(),
                            created,
                            model: model.clone(),
                            choices: vec![ChatChunkChoice {
                                index: 0,
                                delta: ChatDelta { role: None, content: Some(text) },
                                finish_reason: None,
                            }],
                            usage: None,
                        };
                        yield Ok(sse_json(&chunk));
                    } else {
                        yield Ok(Event::default().data(text));
                    }
                }
                TokenEvent::Done { stats } => {
                    metrics.prompt_tokens.fetch_add(u64::from(stats.prompt_tokens), Ordering::Relaxed);
                    metrics.completion_tokens.fetch_add(u64::from(stats.completion_tokens), Ordering::Relaxed);
                    if stats.prefix_cache_hit {
                        metrics.prefix_cache_hits.fetch_add(1, Ordering::Relaxed);
                    }
                    if chat {
                        let chunk = ChatCompletionChunk {
                            id: request_id.clone(),
                            object: "chat.completion.chunk".to_string(),
                            created,
                            model: model.clone(),
                            choices: vec![ChatChunkChoice {
                                index: 0,
                                delta: ChatDelta::default(),
                                finish_reason: Some(stats.finish_reason),
                            }],
                            usage: Some(Usage {
                                prompt_tokens: stats.prompt_tokens,
                                completion_tokens: stats.completion_tokens,
                                total_tokens: stats.prompt_tokens + stats.completion_tokens,
                            }),
                        };
                        yield Ok(sse_json(&chunk));
                    }
                    yield Ok(Event::default().data("[DONE]"));
                    return;
                }
                TokenEvent::Error { message, code } => {
                    metrics.errors.fetch_add(1, Ordering::Relaxed);
                    let body = OpenAiErrorBody {
                        error: OpenAiErrorDetail {
                            message,
                            kind: "backend_error".to_string(),
                            code,
                        },
                    };
                    yield Ok(Event::default().data(serde_json::to_string(&body).unwrap_or_default()));
                    return;
                }
            }
        }
    }
}

fn sse_json<T: Serialize>(value: &T) -> Event {
    Event::default().data(serde_json::to_string(value).unwrap_or_default())
}

fn cache_key(prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hex::encode(hasher.finalize())
}

pub struct ApiError(pub LlmguiError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            LlmguiError::NotFound(_) => StatusCode::NOT_FOUND,
            LlmguiError::Config(_) => StatusCode::BAD_REQUEST,
            LlmguiError::Conflict(_) | LlmguiError::InsufficientMemory(_) => StatusCode::CONFLICT,
            LlmguiError::Busy(_) => StatusCode::SERVICE_UNAVAILABLE,
            LlmguiError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = OpenAiErrorBody {
            error: OpenAiErrorDetail {
                message: self.0.to_string(),
                kind: self.0.code().to_string(),
                code: self.0.code().to_string(),
            },
        };
        (status, Json(body)).into_response()
    }
}

/// Build a state object from a backend and config. Useful for tests.
pub fn app_state(
    backend: Arc<dyn Backend>,
    config: AppConfig,
    model_id: impl Into<String>,
) -> AppState {
    let backend_name = backend.info().name;
    let slots = config.server.max_concurrent;
    let config = Arc::new(config);
    let library = Arc::new(library::LibraryState::from_env(&config.model.extra_dirs));
    let downloads = Arc::new(downloads::DownloadManager::new(
        paths::llmgui_home(),
        library.clone(),
        config.clone(),
    ));
    let runtime = Arc::new(runtime::Runtime::new(backend.clone(), backend_name.clone()));
    let catalog = Arc::new(catalog::CatalogStore::new(
        &config.catalog,
        paths::llmgui_home(),
    ));
    AppState {
        config,
        scheduler: Arc::new(Scheduler::new(backend, slots)),
        model_id: model_id.into(),
        backend_name,
        started: Instant::now(),
        metrics: Arc::new(Metrics::default()),
        library,
        runtime,
        downloads,
        catalog,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use llmgui_backend_mock::MockBackend;
    use tower::ServiceExt;

    fn app() -> Router {
        let backend = Arc::new(MockBackend::new(0));
        let state = app_state(backend, AppConfig::default(), "mock");
        router(state, None)
    }

    #[tokio::test]
    async fn health_reports_ok() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("\"status\":\"ok\""));
        assert!(text.contains("\"backend\":\"mock\""));
    }

    #[tokio::test]
    async fn chat_completion_is_openai_compatible() {
        let payload = serde_json::json!({
            "model": "mock",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 12,
            "stream": false
        });
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "chat.completion");
        assert!(
            !value["choices"][0]["message"]["content"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert!(value["usage"]["completion_tokens"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn chat_stream_terminates_with_done() {
        let payload = serde_json::json!({
            "model": "mock",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 4,
            "stream": true
        });
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("chat.completion.chunk"));
        assert!(text.contains("[DONE]"));
    }

    #[tokio::test]
    async fn api_key_is_enforced() {
        let backend = Arc::new(MockBackend::new(0));
        let mut config = AppConfig::default();
        config.server.api_key = Some("secret".to_string());
        let state = app_state(backend, config, "mock");
        let app = router(state, None);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
