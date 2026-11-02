//! Model library: a registry of installed / imported models on this machine.
//!
//! The registry is an index plus user preferences. `manifest.json` inside a
//! model directory stays the authoritative description of a downloaded
//! artifact; the registry can always be rebuilt by scanning those manifests.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use llmgui_core::estimate::{KvCacheType, MemoryEstimate};
use llmgui_core::manifest::{ContextInfo, ModelFile, ModelManifest};
use llmgui_core::{LlmguiError, Result};
use serde::{Deserialize, Serialize};

use crate::GgufInfo;

pub const REGISTRY_SCHEMA: u32 = 1;
const LOCK_STALE: Duration = Duration::from_secs(30);
const LOCK_RETRY: Duration = Duration::from_millis(50);
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn default_schema() -> u32 {
    REGISTRY_SCHEMA
}

/// A single installed model. Paths are absolute so the registry works from the
/// GUI, CLI and server alike.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub kind: ModelKind,
    pub path: PathBuf,
    #[serde(default)]
    pub manifest_path: Option<PathBuf>,
    #[serde(default)]
    pub location: Location,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub quant: String,
    #[serde(default)]
    pub bits_per_weight: Option<f32>,
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub context: ContextInfo,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub files: Vec<LibraryFile>,
    #[serde(default)]
    pub installed_at: u64,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub fit_24gb: bool,
    #[serde(default)]
    pub fit_32gb: bool,
    #[serde(default)]
    pub vram_bytes_32k_q8: Option<u64>,
    #[serde(default)]
    pub derived_from: Option<String>,
    #[serde(default)]
    pub adapter_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    #[default]
    Inference,
    Train,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Location {
    #[default]
    Managed,
    External {
        #[serde(default)]
        root: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryFile {
    pub path: String,
    pub size: u64,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Per-model user preferences. Kept apart from the artifact manifest.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelPrefs {
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub last_used: Option<u64>,
    #[serde(default)]
    pub load_defaults: LoadDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadDefaults {
    pub ctx: u32,
    pub kv: String,
    pub gpu_layers: i32,
    pub slots: usize,
    pub ttl_secs: u64,
}

impl Default for LoadDefaults {
    fn default() -> Self {
        Self {
            ctx: 32768,
            kv: "q8_0".to_string(),
            gpu_layers: -1,
            slots: 1,
            ttl_secs: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default = "default_schema")]
    pub schema: u32,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub prefs: BTreeMap<String, ModelPrefs>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema: REGISTRY_SCHEMA,
            generation: 0,
            updated_at: 0,
            models: Vec::new(),
            prefs: BTreeMap::new(),
        }
    }
}

impl Registry {
    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|model| model.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ModelEntry> {
        self.models.iter_mut().find(|model| model.id == id)
    }

    pub fn prefs(&self, id: &str) -> ModelPrefs {
        self.prefs.get(id).cloned().unwrap_or_default()
    }

    pub fn alias_of(&self, id: &str) -> Option<String> {
        self.prefs.get(id).and_then(|prefs| prefs.alias.clone())
    }

    /// Resolve an id or alias, or a unique prefix of either.
    pub fn resolve(&self, key: &str) -> Resolve {
        let key = key.trim();
        if key.is_empty() {
            return Resolve::None;
        }
        if self.get(key).is_some() {
            return Resolve::One(key.to_string());
        }
        if let Some((id, _)) = self
            .prefs
            .iter()
            .find(|(_, prefs)| prefs.alias.as_deref() == Some(key))
        {
            return Resolve::One(id.clone());
        }
        let mut matches: BTreeSet<String> = self
            .models
            .iter()
            .filter(|model| model.id.starts_with(key))
            .map(|model| model.id.clone())
            .collect();
        for (id, prefs) in &self.prefs {
            if let Some(alias) = &prefs.alias
                && alias.starts_with(key)
            {
                matches.insert(id.clone());
            }
        }
        match matches.len() {
            0 => Resolve::None,
            1 => Resolve::One(matches.into_iter().next().unwrap_or_default()),
            _ => Resolve::Ambiguous(matches.into_iter().collect()),
        }
    }

    /// Models that reference `id` (derived models or adapters).
    pub fn dependents(&self, id: &str) -> Vec<String> {
        self.models
            .iter()
            .filter(|model| model.derived_from.as_deref() == Some(id))
            .map(|model| model.id.clone())
            .collect()
    }

    pub fn set_alias(&mut self, id: &str, alias: Option<String>) -> Result<()> {
        if self.get(id).is_none() {
            return Err(LlmguiError::NotFound(format!("model {id}")));
        }
        if let Some(alias) = alias.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            if alias == id {
                return Err(LlmguiError::Conflict(
                    "alias must differ from the model id".to_string(),
                ));
            }
            if self.get(alias).is_some() {
                return Err(LlmguiError::Conflict(format!(
                    "alias {alias} collides with a model id"
                )));
            }
            if let Some((other, _)) = self.prefs.iter().find(|(other, prefs)| {
                other.as_str() != id && prefs.alias.as_deref() == Some(alias)
            }) {
                return Err(LlmguiError::Conflict(format!(
                    "alias {alias} is already used by {other}"
                )));
            }
        }
        let prefs = self.prefs.entry(id.to_string()).or_default();
        prefs.alias = alias
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolve {
    One(String),
    Ambiguous(Vec<String>),
    None,
}

/// Registry file access with a process-level lock and atomic writes.
pub struct RegistryStore {
    home: PathBuf,
    path: PathBuf,
}

impl RegistryStore {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let path = home.join("registry.json");
        Self { home, path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn load(&self) -> Result<Registry> {
        if !self.path.exists() {
            return Ok(Registry::default());
        }
        let text = fs::read_to_string(&self.path)?;
        let mut registry: Registry = serde_json::from_str(&text)?;
        if registry.schema != REGISTRY_SCHEMA {
            return Err(LlmguiError::Model(format!(
                "unsupported registry schema {}, expected {}",
                registry.schema, REGISTRY_SCHEMA
            )));
        }
        registry.models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(registry)
    }

    /// Load the registry; when missing/corrupt, rebuild it from the given roots.
    /// The corrupt file is preserved as `registry.json.bad` for diagnostics.
    pub fn load_or_rebuild(&self, roots: &[PathBuf]) -> Result<Registry> {
        if !self.path.exists() {
            let mut registry = Registry::default();
            scan(&mut registry, roots)?;
            if !registry.models.is_empty() {
                self.save(&mut registry)?;
            }
            return Ok(registry);
        }
        match self.load() {
            Ok(registry) => Ok(registry),
            Err(error) => {
                tracing::warn!(%error, "registry unreadable; rebuilding from manifests");
                let bad = self.path.with_extension("json.bad");
                let _ = fs::rename(&self.path, bad);
                let mut registry = Registry::default();
                scan(&mut registry, roots)?;
                self.save(&mut registry)?;
                Ok(registry)
            }
        }
    }

    /// Run `f` under the registry lock and persist the result.
    pub fn update<F>(&self, f: F) -> Result<Registry>
    where
        F: FnOnce(&mut Registry) -> Result<()>,
    {
        let _lock = RegistryLock::acquire(&self.path)?;
        let mut registry = match self.load() {
            Ok(registry) => registry,
            Err(error) if !self.path.exists() => {
                let _ = error;
                Registry::default()
            }
            Err(error) => return Err(error),
        };
        f(&mut registry)?;
        self.save(&mut registry)?;
        Ok(registry)
    }

    pub fn save(&self, registry: &mut Registry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        registry.schema = REGISTRY_SCHEMA;
        registry.generation = registry.generation.saturating_add(1);
        registry.updated_at = now_secs();
        registry.models.sort_by(|a, b| a.id.cmp(&b.id));
        let mut tmp = self.path.clone();
        tmp.set_extension("json.tmp");
        {
            let mut file = File::create(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(registry)?)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

struct RegistryLock {
    path: PathBuf,
}

impl RegistryLock {
    fn acquire(registry_path: &Path) -> Result<Self> {
        let path = registry_path.with_extension("json.lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let started = std::time::Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{} {}", std::process::id(), now_secs());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() > LOCK_TIMEOUT {
                        return Err(LlmguiError::Conflict(
                            "registry is locked by another process".to_string(),
                        ));
                    }
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let modified = meta.modified().unwrap_or(SystemTime::now());
    SystemTime::now()
        .duration_since(modified)
        .map(|age| age > LOCK_STALE)
        .unwrap_or(false)
}

#[derive(Debug, Default, Serialize)]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FitPoint {
    pub ctx: u32,
    pub kv: String,
    pub total_bytes: u64,
    pub fits_24gb: bool,
    pub fits_32gb: bool,
}

/// Scan roots for model directories / GGUF files and merge them into registry.
pub fn scan(registry: &mut Registry, roots: &[PathBuf]) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        scan_dir(&canonical, &canonical, registry, &mut report, &mut seen, 0);
    }
    Ok(report)
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    registry: &mut Registry,
    report: &mut ScanReport,
    seen: &mut BTreeSet<PathBuf>,
    depth: usize,
) {
    if depth > 4 {
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut gguf_files = Vec::new();
    let mut manifest = None;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(root, &path, registry, report, seen, depth + 1);
            continue;
        }
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("gguf") => gguf_files.push(path),
            Some("json") if path.file_name().and_then(|n| n.to_str()) == Some("manifest.json") => {
                manifest = Some(path)
            }
            _ => {}
        }
    }
    if gguf_files.is_empty() && manifest.is_none() {
        return;
    }
    if let Some(manifest_path) = manifest {
        match entry_from_manifest(root, &manifest_path) {
            Ok(entry) => merge_entry(registry, entry, report, seen),
            Err(error) => report
                .errors
                .push(format!("{}: {error}", manifest_path.display())),
        }
        return;
    }
    // A bare GGUF (or a directory of shards) becomes one local model.
    let mut files = gguf_files;
    files.sort();
    let total_size: u64 = files
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum();
    let first = files[0].clone();
    match entry_from_gguf(root, &first, &files, total_size) {
        Ok(entry) => merge_entry(registry, entry, report, seen),
        Err(error) => report.errors.push(format!("{}: {error}", first.display())),
    }
}

fn merge_entry(
    registry: &mut Registry,
    mut entry: ModelEntry,
    report: &mut ScanReport,
    seen: &mut BTreeSet<PathBuf>,
) {
    let path = entry.path.clone();
    if !seen.insert(path.clone()) {
        report.skipped += 1;
        return;
    }
    if let Some(existing) = registry.models.iter_mut().find(|model| model.path == path) {
        let id = existing.id.clone();
        let installed_at = existing.installed_at;
        entry.id = id;
        entry.installed_at = installed_at;
        *existing = entry;
        report.updated += 1;
        return;
    }
    // Keep ids unique: append a numeric suffix rather than merging unrelated models.
    let base = entry.id.clone();
    let mut suffix = 1u32;
    while registry.models.iter().any(|model| model.id == entry.id) {
        suffix += 1;
        entry.id = format!("{base}-{suffix}");
    }
    registry.models.push(entry);
    report.added += 1;
}

fn entry_from_manifest(root: &Path, manifest_path: &Path) -> Result<ModelEntry> {
    let manifest = ModelManifest::load(manifest_path)?;
    let dir = manifest_path
        .parent()
        .ok_or_else(|| LlmguiError::Model("manifest has no parent directory".to_string()))?;
    let first_gguf = manifest
        .files
        .iter()
        .find(|file| file.path.ends_with(".gguf"))
        .map(|file| dir.join(&file.path));
    let info = first_gguf
        .as_deref()
        .filter(|path| path.exists())
        .and_then(|path| GgufInfo::open(path).ok());
    let mut entry = entry_from_manifest_meta(root, dir, &manifest, info.as_ref());
    if let Some(info) = &info {
        apply_fit(&mut entry, info);
    }
    entry.verified = manifest.files.iter().all(|file| !file.sha256.is_empty());
    Ok(entry)
}

fn entry_from_manifest_meta(
    root: &Path,
    dir: &Path,
    manifest: &ModelManifest,
    info: Option<&GgufInfo>,
) -> ModelEntry {
    let id = sanitize_id(
        dir.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&manifest.id),
    );
    let display_name = info
        .and_then(|info| info.name.clone())
        .unwrap_or_else(|| id.clone());
    let files = manifest
        .files
        .iter()
        .map(|file| LibraryFile {
            path: file.path.clone(),
            size: file.size,
            sha256: Some(file.sha256.clone()),
        })
        .collect();
    ModelEntry {
        id,
        display_name,
        kind: ModelKind::Inference,
        path: dir.to_path_buf(),
        manifest_path: Some(dir.join("manifest.json")),
        location: location_for(root, dir),
        repo: Some(manifest.source.repo.clone()),
        source_kind: Some(manifest.source.kind.clone()),
        revision: Some(manifest.source.revision.clone()),
        quant: manifest.quant.kind.clone(),
        bits_per_weight: manifest.quant.bits_per_weight,
        architecture: info
            .and_then(|info| info.architecture.clone())
            .or_else(|| manifest.architecture.clone()),
        context: manifest.context.clone(),
        license: manifest.license.clone(),
        size_bytes: manifest.total_size(),
        files,
        installed_at: now_secs(),
        verified: false,
        fit_24gb: false,
        fit_32gb: false,
        vram_bytes_32k_q8: None,
        derived_from: None,
        adapter_id: None,
    }
}

fn entry_from_gguf(
    root: &Path,
    first: &Path,
    files: &[PathBuf],
    total_size: u64,
) -> Result<ModelEntry> {
    let info = GgufInfo::open(first)?;
    let dir = first.parent().unwrap_or(root).to_path_buf();
    let stem = first
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("model");
    let id = sanitize_id(stem);
    let quant = infer_quant_from_name(stem);
    let mut entry = ModelEntry {
        id,
        display_name: info.name.clone().unwrap_or_else(|| stem.to_string()),
        kind: ModelKind::Inference,
        path: dir.clone(),
        manifest_path: None,
        location: location_for(root, &dir),
        repo: None,
        source_kind: None,
        revision: None,
        quant,
        bits_per_weight: None,
        architecture: info.architecture.clone(),
        context: ContextInfo {
            train: info.n_ctx_train,
            max: info.n_ctx_train,
        },
        license: None,
        size_bytes: total_size,
        files: files
            .iter()
            .map(|path| LibraryFile {
                path: path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string(),
                size: fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
                sha256: None,
            })
            .collect(),
        installed_at: now_secs(),
        verified: false,
        fit_24gb: false,
        fit_32gb: false,
        vram_bytes_32k_q8: None,
        derived_from: None,
        adapter_id: None,
    };
    apply_fit(&mut entry, &info);
    Ok(entry)
}

fn apply_fit(entry: &mut ModelEntry, info: &GgufInfo) {
    let estimate = info.estimate(32768, KvCacheType::Q8_0, 1);
    entry.fit_24gb = estimate.fits_24gb;
    entry.fit_32gb = estimate.fits_32gb;
    entry.vram_bytes_32k_q8 = Some(estimate.total_bytes);
}

fn location_for(root: &Path, path: &Path) -> Location {
    if path.starts_with(root) {
        Location::Managed
    } else {
        Location::External {
            root: Some(root.to_path_buf()),
        }
    }
}

/// Fit matrix for a model at common context / KV combinations.
pub fn fit_matrix(entry: &ModelEntry) -> Result<Vec<FitPoint>> {
    let gguf = entry
        .files
        .iter()
        .find(|file| file.path.ends_with(".gguf"))
        .map(|file| entry.path.join(&file.path))
        .ok_or_else(|| LlmguiError::Model("model has no GGUF file".to_string()))?;
    let info = GgufInfo::open(&gguf)?;
    let mut points = Vec::new();
    for ctx in [8192u32, 16384, 32768] {
        for (name, kv) in [
            ("f16", KvCacheType::F16),
            ("q8_0", KvCacheType::Q8_0),
            ("q4_0", KvCacheType::Q4_0),
        ] {
            let estimate: MemoryEstimate = info.estimate(ctx, kv, 1);
            points.push(FitPoint {
                ctx,
                kv: name.to_string(),
                total_bytes: estimate.total_bytes,
                fits_24gb: estimate.fits_24gb,
                fits_32gb: estimate.fits_32gb,
            });
        }
    }
    Ok(points)
}

fn sanitize_id(raw: &str) -> String {
    let mut id = String::new();
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            id.push(ch);
        } else {
            id.push('-');
        }
    }
    let trimmed = id.trim_matches('-').to_string();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "model".to_string()
    } else {
        trimmed
    }
}

/// Small filename-based quant detector for imported bare GGUF files.
pub fn infer_quant_from_name(name: &str) -> String {
    const QUANTS: &[&str] = &[
        "IQ1_S", "IQ1_M", "IQ2_XXS", "IQ2_XS", "IQ2_S", "IQ3_XXS", "IQ3_XS", "IQ3_S", "IQ3_M",
        "IQ4_XS", "IQ4_NL", "Q2_K", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_0", "Q4_1", "Q4_K_S",
        "Q4_K_M", "Q5_0", "Q5_1", "Q5_K_S", "Q5_K_M", "Q6_K", "Q8_0", "F16", "BF16", "F32",
    ];
    let upper = name.to_uppercase();
    let mut best: Option<&str> = None;
    for quant in QUANTS {
        if upper.contains(quant) && best.is_none_or(|current| quant.len() > current.len()) {
            best = Some(quant);
        }
    }
    best.unwrap_or("unknown").to_string()
}

/// Import a GGUF file or a directory containing GGUF files into the registry.
pub fn import_path(registry: &mut Registry, root: &Path, path: &Path) -> Result<ModelEntry> {
    let path = path
        .canonicalize()
        .map_err(|_| LlmguiError::NotFound(path.display().to_string()))?;
    let mut seen = BTreeSet::new();
    let mut report = ScanReport::default();
    if path.is_dir() {
        scan_dir(root, &path, registry, &mut report, &mut seen, 0);
    } else {
        let files = vec![path.clone()];
        let size = fs::metadata(&path)?.len();
        let entry = entry_from_gguf(root, &path, &files, size)?;
        merge_entry(registry, entry, &mut report, &mut seen);
    }
    if let Some(error) = report.errors.first() {
        return Err(LlmguiError::Model(error.clone()));
    }
    registry
        .models
        .iter()
        .find(|model| {
            model.path == path
                || path.is_dir() && model.path.starts_with(&path)
                || model.path == path.parent().unwrap_or(root)
        })
        .cloned()
        .ok_or_else(|| LlmguiError::NotFound(format!("no model found in {}", path.display())))
}

/// Register a derived (merged / quantized / trained) GGUF with lineage.
pub fn register_derived(
    registry: &mut Registry,
    gguf: &Path,
    base_id: Option<&str>,
    adapter_id: Option<&str>,
) -> Result<ModelEntry> {
    let gguf = gguf
        .canonicalize()
        .map_err(|_| LlmguiError::NotFound(gguf.display().to_string()))?;
    if base_id.is_some_and(|id| registry.get(id).is_none()) {
        return Err(LlmguiError::NotFound(format!(
            "base model {}",
            base_id.unwrap_or_default()
        )));
    }
    let dir = gguf
        .parent()
        .ok_or_else(|| LlmguiError::Model("GGUF has no parent directory".to_string()))?;
    let entries = registry
        .models
        .iter()
        .filter(|model| model.path == dir)
        .count();
    let size = fs::metadata(&gguf)?.len();
    let mut entry = entry_from_gguf(dir, &gguf, std::slice::from_ref(&gguf), size)?;
    if entries > 0 {
        entry.id = format!("{}-derived", entry.id);
    }
    entry.derived_from = base_id.map(str::to_string);
    entry.adapter_id = adapter_id.map(str::to_string);
    let base = entry.id.clone();
    let mut suffix = 1u32;
    while registry.models.iter().any(|model| model.id == entry.id) {
        suffix += 1;
        entry.id = format!("{base}-{suffix}");
    }
    registry.models.push(entry.clone());
    Ok(entry)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_top_p")]
    pub top_p: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_temperature() -> f32 {
    0.7
}
fn default_top_p() -> f32 {
    0.9
}
fn default_max_tokens() -> u32 {
    1024
}

/// Presets live outside the registry so they can be exported/imported as JSON.
pub struct PresetStore {
    dir: PathBuf,
}

impl PresetStore {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            dir: home.into().join("presets"),
        }
    }

    fn path(&self, model_id: &str) -> Result<PathBuf> {
        if model_id.contains(['/', '\\', ':']) || model_id.contains("..") {
            return Err(LlmguiError::Config("invalid model id".to_string()));
        }
        Ok(self.dir.join(format!("{model_id}.json")))
    }

    pub fn list(&self, model_id: &str) -> Result<Vec<Preset>> {
        let path = self.path(model_id)?;
        if !path.exists() {
            return Ok(Vec::new());
        }
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    }

    pub fn save(&self, model_id: &str, preset: Preset) -> Result<Vec<Preset>> {
        if preset.name.trim().is_empty() {
            return Err(LlmguiError::Config("preset name is empty".to_string()));
        }
        let mut presets = self.list(model_id)?;
        if let Some(existing) = presets.iter_mut().find(|item| item.name == preset.name) {
            *existing = preset;
        } else {
            presets.push(preset);
        }
        fs::create_dir_all(&self.dir)?;
        let path = self.path(model_id)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&presets)?)?;
        fs::rename(tmp, path)?;
        Ok(presets)
    }

    pub fn delete(&self, model_id: &str, name: &str) -> Result<Vec<Preset>> {
        let mut presets = self.list(model_id)?;
        presets.retain(|preset| preset.name != name);
        fs::create_dir_all(&self.dir)?;
        fs::write(self.path(model_id)?, serde_json::to_vec_pretty(&presets)?)?;
        Ok(presets)
    }
}

/// Convert a manifest file entry into a [`LibraryFile`].
pub fn library_file(file: &ModelFile) -> LibraryFile {
    LibraryFile {
        path: file.path.clone(),
        size: file.size,
        sha256: Some(file.sha256.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn alias_conflicts_are_rejected() {
        let mut registry = Registry::default();
        registry.models.push(sample_entry("a"));
        registry.models.push(sample_entry("b"));
        registry.set_alias("a", Some("chat".into())).unwrap();
        assert_eq!(registry.resolve("chat"), Resolve::One("a".into()));
        assert!(registry.set_alias("b", Some("chat".into())).is_err());
        assert!(registry.set_alias("a", Some("b".into())).is_err());
        assert_eq!(registry.resolve("a"), Resolve::One("a".into()));
    }

    #[test]
    fn resolve_prefix_and_ambiguity() {
        let mut registry = Registry::default();
        registry.models.push(sample_entry("qwen3-14b"));
        registry.models.push(sample_entry("qwen3-32b"));
        assert_eq!(
            registry.resolve("qwen3-1"),
            Resolve::One("qwen3-14b".into())
        );
        assert!(matches!(registry.resolve("qwen3-"), Resolve::Ambiguous(_)));
        assert_eq!(registry.resolve("missing"), Resolve::None);
    }

    #[test]
    fn registry_roundtrip_and_rebuild() {
        let home = temp_home();
        let store = RegistryStore::new(home.path());
        let mut registry = Registry::default();
        registry.models.push(sample_entry("a"));
        store.save(&mut registry).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.models.len(), 1);
        assert!(loaded.generation >= 1);
        fs::remove_file(store.path()).unwrap();
        let rebuilt = store.load_or_rebuild(&[]).unwrap();
        assert!(rebuilt.models.is_empty());
    }

    #[test]
    fn corrupt_registry_is_backed_up() {
        let home = temp_home();
        let store = RegistryStore::new(home.path());
        fs::create_dir_all(home.path()).unwrap();
        fs::write(store.path(), "{not json").unwrap();
        let registry = store.load_or_rebuild(&[]).unwrap();
        assert!(registry.models.is_empty());
        assert!(home.path().join("registry.json.bad").exists());
    }

    #[test]
    fn quant_detection_prefers_longest_token() {
        assert_eq!(
            infer_quant_from_name("Qwen3-14B-Q4_K_M"),
            "Q4_K_M".to_string()
        );
        assert_eq!(infer_quant_from_name("model-IQ4_XS"), "IQ4_XS".to_string());
        assert_eq!(infer_quant_from_name("model"), "unknown".to_string());
    }

    #[test]
    fn preset_store_roundtrip() {
        let home = temp_home();
        let store = PresetStore::new(home.path());
        store
            .save(
                "m",
                Preset {
                    name: "review".into(),
                    system_prompt: "be strict".into(),
                    temperature: 0.2,
                    top_p: 0.9,
                    max_tokens: 256,
                },
            )
            .unwrap();
        let presets = store.list("m").unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "review");
        store.delete("m", "review").unwrap();
        assert!(store.list("m").unwrap().is_empty());
        assert!(store.path("../../etc").is_err());
    }

    #[test]
    fn scan_finds_bare_gguf_and_rebuilds() {
        let home = temp_home();
        let models = home.path().join("models").join("Qwen--Qwen3-14B-GGUF");
        fs::create_dir_all(&models).unwrap();
        let gguf = models.join("Qwen3-14B-Q4_K_M.gguf");
        crate::write_test_gguf(&gguf, "qwen3", 40, 40, 8, 5120).unwrap();
        let store = RegistryStore::new(home.path());
        let mut registry = store
            .load_or_rebuild(std::slice::from_ref(&models))
            .unwrap();
        assert_eq!(registry.models.len(), 1);
        let entry = &registry.models[0];
        assert_eq!(entry.quant, "Q4_K_M");
        assert_eq!(entry.architecture.as_deref(), Some("qwen3"));
        assert!(entry.vram_bytes_32k_q8.unwrap() > 0);
        assert!(store.path().exists());
        store.save(&mut registry).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.models[0].path, models);
    }

    #[test]
    fn import_and_derived_registration() {
        let home = temp_home();
        let src = home.path().join("incoming");
        fs::create_dir_all(&src).unwrap();
        let gguf = src.join("MyModel-Q5_K_M.gguf");
        crate::write_test_gguf(&gguf, "llama", 32, 32, 8, 4096).unwrap();
        let mut registry = Registry::default();
        let imported = import_path(&mut registry, home.path(), &gguf).unwrap();
        assert_eq!(imported.quant, "Q5_K_M");
        let derived = register_derived(&mut registry, &gguf, Some(&imported.id), None).unwrap();
        assert_eq!(derived.derived_from.as_deref(), Some(imported.id.as_str()));
        assert_ne!(derived.id, imported.id);
        assert_eq!(registry.dependents(&imported.id), vec![derived.id.clone()]);
        assert!(register_derived(&mut registry, &gguf, Some("missing"), None).is_err());
    }

    fn sample_entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            display_name: id.to_string(),
            kind: ModelKind::Inference,
            path: PathBuf::from("/tmp").join(id),
            manifest_path: None,
            location: Location::Managed,
            repo: None,
            source_kind: None,
            revision: None,
            quant: "Q4_K_M".into(),
            bits_per_weight: None,
            architecture: None,
            context: ContextInfo::default(),
            license: None,
            size_bytes: 0,
            files: Vec::new(),
            installed_at: 0,
            verified: false,
            fit_24gb: false,
            fit_32gb: false,
            vram_bytes_32k_q8: None,
            derived_from: None,
            adapter_id: None,
        }
    }
}
