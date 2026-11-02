//! Local admin API used by the GUI and CLI: library, download jobs, catalog,
//! presets, runtime load/unload, storage and cache GC.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::downloads::DownloadRequest;
use crate::{ApiError, AppState};
use axum::extract::{Path as AxumPath, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use llmgui_core::estimate::{KvCacheType, human_bytes};
use llmgui_core::{LlmguiError, Result, paths};
use llmgui_download::{CasStore, Source};
use llmgui_models::GgufInfo;
use llmgui_models::library::{FitPoint, LoadDefaults, Preset, Registry, Resolve, fit_matrix};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/models", get(list_models))
        .route("/admin/models/scan", post(scan_models))
        .route("/admin/models/import", post(import_model))
        .route("/admin/models/register", post(register_model))
        .route("/admin/models/loaded", get(loaded_models))
        .route("/admin/models/unload", post(unload_all))
        .route("/admin/models/{id}", get(get_model).delete(delete_model))
        .route("/admin/models/{id}/estimate", get(estimate_model))
        .route("/admin/models/{id}/alias", put(set_alias))
        .route("/admin/models/{id}/defaults", put(set_defaults))
        .route(
            "/admin/models/{id}/preset",
            get(list_presets).put(save_preset),
        )
        .route("/admin/models/{id}/preset/{name}", delete(delete_preset))
        .route("/admin/models/{id}/load", post(load_model))
        .route("/admin/models/{id}/unload", post(unload_model))
        .route("/admin/models/{id}/verify", post(verify_model))
        .route("/admin/models/{id}/descriptor", get(export_descriptor))
        .route("/admin/models/descriptor", post(import_descriptor))
        .route("/admin/catalog", get(catalog))
        .route("/admin/catalog/search", get(catalog_search))
        .route("/admin/catalog/resolve", get(catalog_resolve))
        .route(
            "/admin/downloads",
            get(list_downloads).post(create_download),
        )
        .route("/admin/downloads/{id}", get(get_download))
        .route("/admin/downloads/{id}/events", get(download_events))
        .route("/admin/downloads/{id}/{action}", post(download_action))
        .route("/admin/storage", get(storage))
        .route("/admin/cache/gc", post(gc_cache))
}

#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    query: String,
    #[serde(default)]
    fit: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    sort: Option<String>,
}

async fn list_models(State(state): State<AppState>, Query(query): Query<ListQuery>) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    let loaded = state.runtime.loaded().await.map(|model| model.id.clone());
    let needle = query.query.trim().to_lowercase();
    let mut entries: Vec<&llmgui_models::library::ModelEntry> = registry
        .models
        .iter()
        .filter(|entry| {
            if needle.is_empty() {
                return true;
            }
            let haystack = format!(
                "{} {} {} {} {}",
                entry.id,
                entry.display_name,
                entry.repo.clone().unwrap_or_default(),
                entry.quant,
                registry.prefs(&entry.id).alias.clone().unwrap_or_default()
            )
            .to_lowercase();
            haystack.contains(&needle)
        })
        .filter(|entry| match query.fit.as_deref() {
            Some("32gb") => entry.fit_32gb,
            Some("24gb") => entry.fit_24gb,
            Some("tight") => entry.fit_32gb && !entry.fit_24gb,
            _ => true,
        })
        .filter(|entry| match query.kind.as_deref() {
            Some(kind) => format!("{:?}", entry.kind).eq_ignore_ascii_case(kind),
            None => true,
        })
        .collect();
    match query.sort.as_deref() {
        Some("size") => entries.sort_by_key(|entry| std::cmp::Reverse(entry.size_bytes)),
        Some("name") => entries.sort_by(|a, b| a.id.cmp(&b.id)),
        Some("recent") => entries.sort_by(|a, b| {
            let a = registry.prefs(&a.id).last_used.unwrap_or(a.installed_at);
            let b = registry.prefs(&b.id).last_used.unwrap_or(b.installed_at);
            b.cmp(&a)
        }),
        _ => entries.sort_by(|a, b| a.id.cmp(&b.id)),
    }
    let data: Vec<Value> = entries
        .iter()
        .map(|entry| entry_value(&registry, entry, loaded.as_deref()))
        .collect();
    Json(json!({
        "ok": true,
        "data": data,
        "total": data.len(),
        "generation": registry.generation,
        "loaded": loaded,
    }))
    .into_response()
}

fn entry_value(
    registry: &Registry,
    entry: &llmgui_models::library::ModelEntry,
    loaded: Option<&str>,
) -> Value {
    let prefs = registry.prefs(&entry.id);
    let mut value = serde_json::to_value(entry).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("alias".into(), json!(prefs.alias));
        object.insert("favorite".into(), json!(prefs.favorite));
        object.insert("tags".into(), json!(prefs.tags));
        object.insert("last_used".into(), json!(prefs.last_used));
        object.insert("load_defaults".into(), json!(prefs.load_defaults));
        object.insert("loaded".into(), json!(loaded == Some(entry.id.as_str())));
        object.insert(
            "vram_human".into(),
            json!(entry.vram_bytes_32k_q8.map(human_bytes)),
        );
    }
    value
}

async fn get_model(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    let Some(entry) = registry.get(&id).cloned() else {
        return ApiError(LlmguiError::NotFound(format!("model {id}"))).into_response();
    };
    let loaded = state.runtime.loaded().await.map(|model| model.id.clone());
    let mut value = entry_value(&registry, &entry, loaded.as_deref());
    let fit = fit_matrix(&entry).unwrap_or_default();
    let presets = state.library.presets().list(&id).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.insert("fit_matrix".into(), json!(fit));
        object.insert("presets".into(), json!(presets));
    }
    Json(value).into_response()
}

async fn scan_models(State(state): State<AppState>) -> Response {
    match state.library.refresh() {
        Ok((registry, report)) => Json(json!({
            "ok": true,
            "report": report,
            "total": registry.models.len(),
            "generation": registry.generation,
        }))
        .into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct ImportRequest {
    path: String,
    #[serde(default)]
    base_id: Option<String>,
    #[serde(default)]
    adapter_id: Option<String>,
}

async fn import_model(
    State(state): State<AppState>,
    Json(request): Json<ImportRequest>,
) -> Response {
    let path = PathBuf::from(&request.path);
    let result = if request.base_id.is_some() || request.adapter_id.is_some() {
        state.library.register_derived(
            &path,
            request.base_id.as_deref(),
            request.adapter_id.as_deref(),
        )
    } else {
        state.library.import(&path)
    };
    match result {
        Ok(entry) => Json(json!({"ok": true, "data": entry})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn register_model(
    State(state): State<AppState>,
    Json(request): Json<ImportRequest>,
) -> Response {
    match state.library.register_derived(
        Path::new(&request.path),
        request.base_id.as_deref(),
        request.adapter_id.as_deref(),
    ) {
        Ok(entry) => Json(json!({"ok": true, "data": entry})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn loaded_models(State(state): State<AppState>) -> Response {
    let loaded = state.runtime.loaded().await;
    Json(json!({
        "ok": true,
        "data": loaded.as_ref().map(|model| json!({
            "id": model.id,
            "display_name": model.display_name,
            "path": model.path,
            "ctx": model.ctx,
            "kv": model.kv,
            "slots": model.slots,
            "ttl_secs": model.ttl_secs,
            "loaded_at": model.loaded_at,
        })),
        "active_requests": state.runtime.active_requests(),
        "vram_budget_bytes": state.runtime.vram_budget_bytes(),
    }))
    .into_response()
}

async fn unload_all(State(state): State<AppState>) -> Response {
    match state.runtime.unload().await {
        Ok(id) => Json(json!({"ok": true, "unloaded": id})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    purge: bool,
}

async fn delete_model(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<DeleteQuery>,
) -> Response {
    if state.runtime.loaded_id().await.as_deref() == Some(id.as_str()) {
        return ApiError(LlmguiError::Conflict(format!(
            "model {id} is loaded; unload it first"
        )))
        .into_response();
    }
    match state.library.remove(&id, query.purge) {
        Ok(removal) => {
            let mut gc = Value::Null;
            if query.purge
                && let Ok(report) = gc_inner(&state, false, 0)
            {
                gc = json!(report);
            }
            Json(json!({
                "ok": true,
                "id": id,
                "files_removed": removal.files_removed,
                "gc": gc,
            }))
            .into_response()
        }
        Err(error) => ApiError(error).into_response(),
    }
}

#[derive(Debug, Deserialize, Default)]
struct EstimateQuery {
    #[serde(default)]
    ctx: Option<u32>,
    #[serde(default)]
    kv: Option<String>,
    #[serde(default)]
    slots: Option<u32>,
}

async fn estimate_model(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<EstimateQuery>,
) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    let Some(entry) = registry.get(&id).cloned() else {
        return ApiError(LlmguiError::NotFound(format!("model {id}"))).into_response();
    };
    let ctx = query.ctx.unwrap_or(32768);
    let kv = query.kv.unwrap_or_else(|| "q8_0".to_string());
    let slots = query.slots.unwrap_or(1);
    let Some(gguf) = entry
        .files
        .iter()
        .find(|file| file.path.ends_with(".gguf"))
        .map(|file| entry.path.join(&file.path))
    else {
        return ApiError(LlmguiError::Model("model has no GGUF file".to_string())).into_response();
    };
    match GgufInfo::open(&gguf) {
        Ok(info) => {
            let estimate = info.estimate(ctx, KvCacheType::parse(&kv), slots);
            let points: Vec<FitPoint> = fit_matrix(&entry).unwrap_or_default();
            Json(json!({
                "ok": true,
                "data": estimate,
                "ctx": ctx,
                "kv": kv,
                "slots": slots,
                "budget_bytes": state.runtime.vram_budget_bytes(),
                "fits_budget": estimate.total_bytes <= state.runtime.vram_budget_bytes(),
                "matrix": points,
            }))
            .into_response()
        }
        Err(error) => ApiError(error).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AliasRequest {
    #[serde(default)]
    alias: Option<String>,
}

async fn set_alias(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<AliasRequest>,
) -> Response {
    match state.library.set_alias(&id, request.alias) {
        Ok(registry) => Json(json!({
            "ok": true,
            "generation": registry.generation,
            "alias": registry.prefs(&id).alias,
        }))
        .into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DefaultsRequest {
    #[serde(default)]
    ctx: Option<u32>,
    #[serde(default)]
    kv: Option<String>,
    #[serde(default)]
    gpu_layers: Option<i32>,
    #[serde(default)]
    slots: Option<usize>,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

fn merged_defaults(base: LoadDefaults, request: &DefaultsRequest) -> LoadDefaults {
    LoadDefaults {
        ctx: request.ctx.unwrap_or(base.ctx),
        kv: request.kv.clone().unwrap_or(base.kv),
        gpu_layers: request.gpu_layers.unwrap_or(base.gpu_layers),
        slots: request.slots.unwrap_or(base.slots).max(1),
        ttl_secs: request.ttl_secs.unwrap_or(base.ttl_secs),
    }
}

async fn set_defaults(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<DefaultsRequest>,
) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    if registry.get(&id).is_none() {
        return ApiError(LlmguiError::NotFound(format!("model {id}"))).into_response();
    }
    let defaults = merged_defaults(registry.prefs(&id).load_defaults, &request);
    match state.library.set_defaults(&id, defaults.clone()) {
        Ok(registry) => Json(json!({
            "ok": true,
            "generation": registry.generation,
            "load_defaults": defaults,
        }))
        .into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn list_presets(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    match state.library.presets().list(&id) {
        Ok(presets) => Json(json!({"ok": true, "data": presets})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn save_preset(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(preset): Json<Preset>,
) -> Response {
    match state.library.presets().save(&id, preset) {
        Ok(presets) => Json(json!({"ok": true, "data": presets})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn delete_preset(
    State(state): State<AppState>,
    AxumPath((id, name)): AxumPath<(String, String)>,
) -> Response {
    match state.library.presets().delete(&id, &name) {
        Ok(presets) => Json(json!({"ok": true, "data": presets})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

#[derive(Debug, Deserialize, Default)]
struct LoadRequest {
    #[serde(default)]
    ctx: Option<u32>,
    #[serde(default)]
    kv: Option<String>,
    #[serde(default)]
    gpu_layers: Option<i32>,
    #[serde(default)]
    slots: Option<usize>,
    #[serde(default)]
    ttl_secs: Option<u64>,
    #[serde(default)]
    alias: Option<String>,
}

async fn load_model(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<LoadRequest>,
) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    let id = match registry.resolve(&id) {
        Resolve::One(id) => id,
        Resolve::Ambiguous(ids) => {
            return ApiError(LlmguiError::Conflict(format!(
                "model name is ambiguous: {}",
                ids.join(", ")
            )))
            .into_response();
        }
        Resolve::None => {
            return ApiError(LlmguiError::NotFound(format!("model {id}"))).into_response();
        }
    };
    let Some(entry) = registry.get(&id).cloned() else {
        return ApiError(LlmguiError::NotFound(format!("model {id}"))).into_response();
    };
    if let Some(alias) = request.alias.clone()
        && let Err(error) = state.library.set_alias(&id, Some(alias))
    {
        return ApiError(error).into_response();
    }
    let defaults = merged_defaults(
        registry.prefs(&id).load_defaults,
        &DefaultsRequest {
            ctx: request.ctx,
            kv: request.kv,
            gpu_layers: request.gpu_layers,
            slots: request.slots,
            ttl_secs: request.ttl_secs,
        },
    );
    match state
        .runtime
        .load(&entry, defaults, state.runtime.vram_budget_bytes())
        .await
    {
        Ok(model) => Json(json!({
            "ok": true,
            "data": {
                "id": model.id,
                "ctx": model.ctx,
                "kv": model.kv,
                "slots": model.slots,
                "ttl_secs": model.ttl_secs,
                "vram_budget_bytes": state.runtime.vram_budget_bytes(),
            }
        }))
        .into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn unload_model(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    let loaded = state.runtime.loaded_id().await;
    if loaded.as_deref() != Some(id.as_str()) {
        return ApiError(LlmguiError::Conflict(format!(
            "model {id} is not the loaded model"
        )))
        .into_response();
    }
    match state.runtime.unload().await {
        Ok(unloaded) => Json(json!({"ok": true, "unloaded": unloaded})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn verify_model(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    let Some(entry) = registry.get(&id).cloned() else {
        return ApiError(LlmguiError::NotFound(format!("model {id}"))).into_response();
    };
    match tokio::task::spawn_blocking(move || verify_entry(&entry)).await {
        Ok(Ok(results)) => {
            let ok = results.iter().all(|result| result.1);
            let _ = state.library.store.update(|registry| {
                if let Some(entry) = registry.get_mut(&id) {
                    entry.verified = ok;
                }
                Ok(())
            });
            Json(json!({
                "ok": ok,
                "data": results
                    .into_iter()
                    .map(|(path, ok, detail)| json!({"path": path, "ok": ok, "detail": detail}))
                    .collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Ok(Err(error)) => ApiError(error).into_response(),
        Err(error) => {
            ApiError(LlmguiError::Backend(format!("verify task failed: {error}"))).into_response()
        }
    }
}

fn verify_entry(entry: &llmgui_models::library::ModelEntry) -> Result<Vec<(String, bool, String)>> {
    use sha2::{Digest, Sha256};
    let mut results = Vec::new();
    for file in &entry.files {
        let Some(expected) = file.sha256.as_deref() else {
            continue;
        };
        if expected.is_empty() {
            continue;
        }
        let path = entry.path.join(&file.path);
        let Ok(mut handle) = std::fs::File::open(&path) else {
            results.push((file.path.clone(), false, "missing".to_string()));
            continue;
        };
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        use std::io::Read;
        let mut ok = true;
        loop {
            match handle.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => hasher.update(&buffer[..read]),
                Err(error) => {
                    results.push((file.path.clone(), false, error.to_string()));
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let actual = hex::encode(hasher.finalize());
        results.push((
            file.path.clone(),
            actual == expected,
            format!("expected {expected}, got {actual}"),
        ));
    }
    if results.is_empty() {
        return Ok(vec![(
            entry.id.clone(),
            true,
            "no hashes recorded; nothing to verify".to_string(),
        )]);
    }
    Ok(results)
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct SeedCatalog {
    #[serde(default)]
    models: Vec<SeedModel>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SeedModel {
    id: String,
    repo: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    quant: String,
    #[serde(default)]
    license: String,
    #[serde(default)]
    size_bytes: u64,
    #[serde(default)]
    role: String,
}

const SEED_CATALOG: &str = include_str!("../../../models/v0.toml");

fn seed_catalog() -> SeedCatalog {
    toml::from_str(SEED_CATALOG).unwrap_or_default()
}

async fn catalog(State(state): State<AppState>) -> Response {
    let registry = state.library.registry().unwrap_or_default();
    let loaded = state.runtime.loaded().await.map(|model| model.id.clone());
    let installed: BTreeSet<&str> = registry
        .models
        .iter()
        .map(|entry| entry.repo.as_deref().unwrap_or_default())
        .collect();
    let data: Vec<Value> = seed_catalog()
        .models
        .into_iter()
        .map(|model| {
            json!({
                "id": model.id,
                "repo": model.repo,
                "file": model.file,
                "quant": model.quant,
                "license": model.license,
                "size_bytes": model.size_bytes,
                "size_human": human_bytes(model.size_bytes),
                "role": model.role,
                "installed": installed.contains(model.repo.as_str()),
                "downloaded": registry.models.iter().any(|entry| entry.repo.as_deref() == Some(model.repo.as_str())),
            })
        })
        .collect();
    Json(json!({"ok": true, "data": data, "loaded": loaded})).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn catalog_search(Query(query): Query<SearchQuery>) -> Response {
    let needle = query.q.trim().to_lowercase();
    let data: Vec<SeedModel> = seed_catalog()
        .models
        .into_iter()
        .filter(|model| {
            needle.is_empty()
                || format!("{} {} {} {}", model.id, model.repo, model.quant, model.role)
                    .to_lowercase()
                    .contains(&needle)
        })
        .collect();
    Json(json!({"ok": true, "data": data})).into_response()
}

#[derive(Debug, Deserialize, Default)]
struct ResolveQuery {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

async fn catalog_resolve(Query(query): Query<ResolveQuery>) -> Response {
    let repo = normalize_repo(&query.repo);
    if repo.is_empty() {
        return ApiError(LlmguiError::Config("repo is empty".to_string())).into_response();
    }
    let revision = query
        .revision
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("master");
    let source_name = query
        .source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("auto");
    let source: Box<dyn Source> = match source_name {
        "modelscope" => match llmgui_download::ModelScopeSource::new() {
            Ok(source) => Box::new(source),
            Err(error) => return ApiError(error).into_response(),
        },
        "hf-mirror" | "gitee-ai" => match llmgui_download::HfSource::new(source_name) {
            Ok(source) => Box::new(source),
            Err(error) => return ApiError(error).into_response(),
        },
        _ => match llmgui_download::ModelScopeSource::new() {
            Ok(source) => Box::new(source),
            Err(error) => return ApiError(error).into_response(),
        },
    };
    match source.list(&repo, revision).await {
        Ok(files) => {
            let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for file in files {
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                if !name.ends_with(".gguf") {
                    continue;
                }
                let quant = llmgui_models::library::infer_quant_from_name(name);
                groups.entry(quant).or_default().push(json!({
                    "path": file.path,
                    "size": file.size,
                    "size_human": human_bytes(file.size),
                }));
            }
            let license = seed_catalog()
                .models
                .iter()
                .find(|model| model.repo == repo)
                .map(|model| model.license.clone());
            let quants: Vec<Value> = groups
                .into_iter()
                .map(|(quant, files)| {
                    let total: u64 = files
                        .iter()
                        .filter_map(|file| file.get("size").and_then(Value::as_u64))
                        .sum();
                    json!({
                        "quant": quant,
                        "files": files,
                        "total_size": total,
                        "total_human": human_bytes(total),
                    })
                })
                .collect();
            Json(json!({
                "ok": true,
                "repo": repo,
                "revision": revision,
                "license": license,
                "quants": quants,
            }))
            .into_response()
        }
        Err(error) => ApiError(error).into_response(),
    }
}

fn normalize_repo(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches(".git");
    let path = if let Some((_, rest)) = trimmed.split_once("://") {
        rest.split_once('/').map(|(_, path)| path).unwrap_or(rest)
    } else {
        trimmed
    };
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let start = segments
        .iter()
        .position(|part| *part == "models")
        .map(|index| index + 1)
        .unwrap_or_else(|| segments.len().saturating_sub(2));
    segments
        .get(start..)
        .map(|parts| parts.join("/"))
        .unwrap_or_default()
}

async fn create_download(
    State(state): State<AppState>,
    Json(request): Json<DownloadRequest>,
) -> Response {
    match state.downloads.create(request) {
        Ok(job) => Json(json!({"ok": true, "data": job})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn list_downloads(State(state): State<AppState>) -> Response {
    Json(json!({"ok": true, "data": state.downloads.list()})).into_response()
}

async fn get_download(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    match state.downloads.get(&id) {
        Some(job) => Json(json!({"ok": true, "data": job})).into_response(),
        None => ApiError(LlmguiError::NotFound(format!("download job {id}"))).into_response(),
    }
}

async fn download_action(
    State(state): State<AppState>,
    AxumPath((id, action)): AxumPath<(String, String)>,
) -> Response {
    match state.downloads.control(&id, &action) {
        Ok(job) => Json(json!({"ok": true, "data": job})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

async fn download_events(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if state.downloads.get(&id).is_none() {
        return ApiError(LlmguiError::NotFound(format!("download job {id}"))).into_response();
    }
    let stream = async_stream::stream! {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        loop {
            ticker.tick().await;
            match state.downloads.get(&id) {
                Some(job) => {
                    let terminal = job.state.is_terminal();
                    yield Ok::<Event, std::convert::Infallible>(
                        Event::default()
                            .event("state")
                            .data(serde_json::to_string(&job).unwrap_or_default()),
                    );
                    if terminal {
                        return;
                    }
                }
                None => {
                    yield Ok(Event::default()
                        .event("error")
                        .data(json!({"message": "job not found"}).to_string()));
                    return;
                }
            }
        }
    };
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

async fn storage(State(state): State<AppState>) -> Response {
    let registry = state.library.registry().unwrap_or_default();
    let models_bytes: u64 = registry.models.iter().map(|entry| entry.size_bytes).sum();
    let cache_root = state
        .config
        .download
        .cache_dir
        .clone()
        .unwrap_or_else(paths::cache_dir);
    let cache_bytes = dir_size(&cache_root);
    let reclaimable = gc_inner(&state, true, 24)
        .map(|report| report.freed_bytes)
        .unwrap_or(0);
    Json(json!({
        "ok": true,
        "models_bytes": models_bytes,
        "models_human": human_bytes(models_bytes),
        "cache_bytes": cache_bytes,
        "cache_human": human_bytes(cache_bytes),
        "reclaimable_bytes": reclaimable,
        "reclaimable_human": human_bytes(reclaimable),
        "cache_root": cache_root,
        "models_root": paths::models_dir(),
        "total": models_bytes + cache_bytes,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct GcRequest {
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    grace_hours: Option<u64>,
}

async fn gc_cache(State(state): State<AppState>, Json(request): Json<GcRequest>) -> Response {
    match gc_inner(&state, request.dry_run, request.grace_hours.unwrap_or(24)) {
        Ok(report) => Json(json!({"ok": true, "data": report})).into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

fn gc_inner(
    state: &AppState,
    dry_run: bool,
    grace_hours: u64,
) -> Result<llmgui_download::gc::GcReport> {
    let cache_root = state
        .config
        .download
        .cache_dir
        .clone()
        .unwrap_or_else(paths::cache_dir);
    let store = CasStore::new(cache_root)?;
    let refs = llmgui_download::gc::referenced_hashes(&paths::models_dir())?;
    llmgui_download::gc::gc_unreferenced(
        &store,
        &refs,
        dry_run,
        Duration::from_secs(grace_hours * 3600),
    )
}

async fn export_descriptor(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    let Some(entry) = registry.get(&id) else {
        return ApiError(LlmguiError::NotFound(format!("model {id}"))).into_response();
    };
    let prefs = registry.prefs(&id);
    Json(json!({
        "model": id,
        "base": [{
            "key": id,
            "sources": [{
                "type": match entry.source_kind.as_deref() {
                    Some("modelscope") => "modelscope",
                    _ => "huggingface",
                },
                "repo": entry.repo.clone().unwrap_or_default(),
            }],
        }],
        "metadataOverrides": {
            "paramsStrings": [],
            "contextLengths": [entry.context.max],
            "license": entry.license,
            "minMemoryUsageBytes": entry.size_bytes,
            "quantization": entry.quant,
            "architecture": entry.architecture,
        },
        "config": {
            "load": {
                "fields": [
                    {"key": "llm.load.contextLength", "value": prefs.load_defaults.ctx},
                    {"key": "llm.load.kvCacheType", "value": prefs.load_defaults.kv},
                    {"key": "llm.load.numGpuLayers", "value": prefs.load_defaults.gpu_layers},
                    {"key": "llm.load.maxParallelPredictions", "value": prefs.load_defaults.slots},
                    {"key": "llm.load.ttlSeconds", "value": prefs.load_defaults.ttl_secs}
                ]
            },
            "operation": {"fields": []}
        }
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct Descriptor {
    model: String,
    #[serde(default)]
    base: Vec<DescriptorBase>,
    #[serde(default)]
    #[serde(rename = "metadataOverrides")]
    metadata: Option<Value>,
    #[serde(default)]
    config: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct DescriptorBase {
    #[serde(default)]
    key: String,
    #[serde(default)]
    sources: Vec<DescriptorSource>,
}

#[derive(Debug, Deserialize)]
struct DescriptorSource {
    #[serde(default)]
    repo: String,
}

async fn import_descriptor(
    State(state): State<AppState>,
    Json(descriptor): Json<Descriptor>,
) -> Response {
    let registry = match state.library.registry() {
        Ok(registry) => registry,
        Err(error) => return ApiError(error).into_response(),
    };
    let mut matched = registry
        .get(&descriptor.model)
        .map(|entry| entry.id.clone());
    if matched.is_none() {
        for base in &descriptor.base {
            if let Some(entry) = registry.get(&base.key) {
                matched = Some(entry.id.clone());
                break;
            }
            for source in &base.sources {
                if let Some(entry) = registry
                    .models
                    .iter()
                    .find(|entry| entry.repo.as_deref() == Some(source.repo.as_str()))
                {
                    matched = Some(entry.id.clone());
                    break;
                }
            }
            if matched.is_some() {
                break;
            }
        }
    }
    let Some(id) = matched else {
        return ApiError(LlmguiError::NotFound(
            "descriptor does not match an installed model".to_string(),
        ))
        .into_response();
    };
    let mut defaults = registry.prefs(&id).load_defaults;
    if let Some(fields) = descriptor
        .config
        .as_ref()
        .and_then(|config| config.pointer("/load/fields"))
        .and_then(Value::as_array)
    {
        for field in fields {
            let key = field.get("key").and_then(Value::as_str).unwrap_or_default();
            let value = field.get("value").cloned().unwrap_or(Value::Null);
            match key {
                "llm.load.contextLength" => {
                    if let Some(ctx) = value.as_u64() {
                        defaults.ctx = ctx as u32;
                    }
                }
                "llm.load.kvCacheType" => {
                    if let Some(kv) = value.as_str() {
                        defaults.kv = kv.to_string();
                    }
                }
                "llm.load.numGpuLayers" => {
                    if let Some(layers) = value.as_i64() {
                        defaults.gpu_layers = layers as i32;
                    }
                }
                "llm.load.maxParallelPredictions" => {
                    if let Some(slots) = value.as_u64() {
                        defaults.slots = slots as usize;
                    }
                }
                "llm.load.ttlSeconds" => {
                    if let Some(ttl) = value.as_u64() {
                        defaults.ttl_secs = ttl;
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(metadata) = descriptor.metadata.as_ref() {
        let _ = state.library.store.update(|registry| {
            if let Some(entry) = registry.get_mut(&id) {
                if let Some(license) = metadata.get("license").and_then(Value::as_str) {
                    entry.license = Some(license.to_string());
                }
                if let Some(name) = metadata.get("displayName").and_then(Value::as_str) {
                    entry.display_name = name.to_string();
                }
            }
            Ok(())
        });
    }
    if let Err(error) = state.library.set_defaults(&id, defaults.clone()) {
        return ApiError(error).into_response();
    }
    if let Err(error) = state.library.set_alias(&id, Some(descriptor.model.clone())) {
        return ApiError(error).into_response();
    }
    Json(json!({"ok": true, "id": id, "load_defaults": defaults})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use llmgui_backend_mock::MockBackend;
    use llmgui_core::config::AppConfig;
    use llmgui_models::write_test_gguf;
    use llmgui_scheduler::Scheduler;
    use tower::ServiceExt;

    use crate::downloads::DownloadManager;
    use crate::library::LibraryState;
    use crate::runtime::Runtime;
    use crate::{AppState, Metrics, router};

    fn temp_app() -> (tempfile::TempDir, axum::Router) {
        let home = tempfile::tempdir().unwrap();
        let config = Arc::new(AppConfig::default());
        let library = Arc::new(LibraryState::new(home.path().to_path_buf(), &[]));
        let downloads = Arc::new(DownloadManager::new(
            home.path().to_path_buf(),
            library.clone(),
            config.clone(),
        ));
        let backend = Arc::new(MockBackend::new(0));
        let runtime = Arc::new(Runtime::new(backend.clone(), "mock"));
        let state = AppState {
            config,
            scheduler: Arc::new(Scheduler::new(backend, 2)),
            model_id: "mock".to_string(),
            backend_name: "mock".to_string(),
            started: Instant::now(),
            metrics: Arc::new(Metrics::default()),
            library,
            runtime,
            downloads,
        };
        (home, router(state, None))
    }

    fn seed_model(home: &std::path::Path) -> String {
        let dir = home.join("models").join("Tiny--Q4_K_M");
        std::fs::create_dir_all(&dir).unwrap();
        write_test_gguf(&dir.join("Tiny-Q4_K_M.gguf"), "qwen2", 4, 4, 2, 512).unwrap();
        "Tiny-Q4_K_M".to_string()
    }

    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = if let Some(body) = body {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        } else {
            Body::empty()
        };
        let response = app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn scan_list_and_detail() {
        let (home, app) = temp_app();
        let id = seed_model(home.path());
        let (status, body) = call(&app, "POST", "/admin/models/scan", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["report"]["added"], 1);
        let (status, body) = call(&app, "GET", "/admin/models", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total"], 1);
        assert_eq!(body["data"][0]["id"], id);
        assert_eq!(body["data"][0]["fit_32gb"], true);
        let (status, body) = call(&app, "GET", &format!("/admin/models/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["fit_matrix"].as_array().unwrap().len() >= 9);
    }

    #[tokio::test]
    async fn alias_defaults_and_load() {
        let (home, app) = temp_app();
        let id = seed_model(home.path());
        let _ = call(&app, "POST", "/admin/models/scan", None).await;
        let (status, _) = call(
            &app,
            "PUT",
            &format!("/admin/models/{id}/alias"),
            Some(json!({"alias": "tiny"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = call(
            &app,
            "PUT",
            &format!("/admin/models/{id}/defaults"),
            Some(json!({"ctx": 512, "slots": 1, "ttl_secs": 0})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = call(
            &app,
            "POST",
            "/admin/models/tiny/load",
            Some(json!({"ctx": 512})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["id"], id);
        let (_, body) = call(&app, "GET", "/admin/models/loaded", None).await;
        assert_eq!(body["data"]["id"], id);
        let (status, body) = call(&app, "GET", "/v1/models", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"][0]["id"], id);
        let (status, _) = call(&app, "DELETE", &format!("/admin/models/{id}"), None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = call(&app, "POST", &format!("/admin/models/{id}/unload"), None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(
            &app,
            "DELETE",
            &format!("/admin/models/{id}?purge=true"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = call(&app, "GET", "/admin/models", None).await;
        assert_eq!(body["total"], 0);
    }

    #[tokio::test]
    async fn presets_roundtrip() {
        let (home, app) = temp_app();
        let id = seed_model(home.path());
        let _ = call(&app, "POST", "/admin/models/scan", None).await;
        let payload = json!({"name": "review", "system_prompt": "be strict", "temperature": 0.2, "top_p": 0.9, "max_tokens": 128});
        let (status, body) = call(
            &app,
            "PUT",
            &format!("/admin/models/{id}/preset"),
            Some(payload),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = call(&app, "GET", &format!("/admin/models/{id}/preset"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"][0]["name"], "review");
        let (status, _) = call(
            &app,
            "DELETE",
            &format!("/admin/models/{id}/preset/review"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn catalog_and_storage() {
        let (_home, app) = temp_app();
        let (status, body) = call(&app, "GET", "/admin/catalog", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"].as_array().unwrap().len() >= 5);
        let (status, body) = call(&app, "GET", "/admin/catalog/search?q=qwen3-32b", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
        let (status, body) = call(&app, "GET", "/admin/storage", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["models_bytes"].as_u64().is_some());
    }

    #[tokio::test]
    async fn chat_jit_loads_installed_model() {
        let (home, app) = temp_app();
        let id = seed_model(home.path());
        let _ = call(&app, "POST", "/admin/models/scan", None).await;
        let (status, body) = call(
            &app,
            "POST",
            "/v1/chat/completions",
            Some(json!({
                "model": id,
                "messages": [{"role": "user", "content": "hello"}],
                "max_tokens": 4,
                "stream": false
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["model"], id);
        let (_, loaded) = call(&app, "GET", "/admin/models/loaded", None).await;
        assert_eq!(loaded["data"]["id"], id);
    }

    #[tokio::test]
    async fn descriptor_roundtrip() {
        let (home, app) = temp_app();
        let id = seed_model(home.path());
        let _ = call(&app, "POST", "/admin/models/scan", None).await;
        let (status, descriptor) =
            call(&app, "GET", &format!("/admin/models/{id}/descriptor"), None).await;
        assert_eq!(status, StatusCode::OK, "{descriptor}");
        assert_eq!(descriptor["model"], id);
        let mut imported = descriptor.clone();
        imported["model"] = json!("tiny-descriptor");
        imported["config"]["load"]["fields"][0]["value"] = json!(4096);
        let (status, body) = call(&app, "POST", "/admin/models/descriptor", Some(imported)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["load_defaults"]["ctx"], 4096);
        let (_, list) = call(&app, "GET", "/admin/models", None).await;
        assert_eq!(list["data"][0]["alias"], "tiny-descriptor");
    }

    #[tokio::test]
    async fn invalid_source_download_fails_cleanly() {
        let (_home, app) = temp_app();
        let (status, body) = call(
            &app,
            "POST",
            "/admin/downloads",
            Some(json!({"repo": "Qwen/Tiny", "source": "bogus"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["data"]["id"].as_str().unwrap().to_string();
        let mut state = String::new();
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            let (_, job) = call(&app, "GET", &format!("/admin/downloads/{id}"), None).await;
            state = job["data"]["state"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if state == "failed" {
                break;
            }
        }
        assert_eq!(state, "failed");
        let (status, job) = call(&app, "GET", &format!("/admin/downloads/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(job["data"]["error"].as_str().unwrap().contains("bogus"));
    }

    #[tokio::test]
    async fn unknown_model_is_404_and_duplicate_alias_409() {
        let (home, app) = temp_app();
        let id = seed_model(home.path());
        let _ = call(&app, "POST", "/admin/models/scan", None).await;
        let (status, _) = call(&app, "GET", "/admin/models/nope", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let _ = call(
            &app,
            "PUT",
            &format!("/admin/models/{id}/alias"),
            Some(json!({"alias": "tiny"})),
        )
        .await;
        let other = home.path().join("models").join("Other");
        std::fs::create_dir_all(&other).unwrap();
        write_test_gguf(&other.join("Other-Q4_K_M.gguf"), "llama", 4, 4, 2, 512).unwrap();
        let _ = call(&app, "POST", "/admin/models/scan", None).await;
        let (status, _) = call(
            &app,
            "PUT",
            &format!("/admin/models/{id}/alias"),
            Some(json!({"alias": "tiny"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = call(
            &app,
            "PUT",
            "/admin/models/Other-Q4_K_M/alias",
            Some(json!({"alias": "tiny"})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
}
