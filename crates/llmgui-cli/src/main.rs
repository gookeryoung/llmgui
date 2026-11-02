use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand};
use llmgui_backend_api::{Backend, GenerateRequest, LoadConfig, TokenEvent};
use llmgui_backend_mock::MockBackend;
use llmgui_core::config::AppConfig;
use llmgui_core::estimate::{KvCacheType, human_bytes};
use llmgui_core::paths;
use llmgui_core::{LlmguiError, Result};
use llmgui_download::Source as _;
use llmgui_models::GgufInfo;
use llmgui_scheduler::Scheduler;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "llmgui", version, about = "Local LLM runtime + fine-tuning")]
struct Cli {
    /// Override LLMGUI_HOME.
    #[arg(long, global = true, env = "LLMGUI_HOME")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum ModelsCommand {
    /// List installed models.
    List,
    /// Show one model's details.
    Show { id: String },
    /// Rescan the model directories and update the registry.
    Scan,
    /// Import a GGUF file or directory (optionally as a derived model).
    Import {
        path: PathBuf,
        #[arg(long)]
        base_id: Option<String>,
        #[arg(long)]
        adapter_id: Option<String>,
    },
    /// Remove a model from the library.
    Remove {
        id: String,
        /// Also delete unreferenced CAS blobs.
        #[arg(long)]
        purge: bool,
    },
    /// Verify recorded SHA256 hashes.
    Verify { id: Option<String> },
    /// Search the curated catalog.
    Search {
        #[arg(default_value = "")]
        query: String,
    },
}

#[derive(Subcommand)]
enum Commands {
    /// Download a model from a domestic source (ModelScope first).
    Pull {
        repo: String,
        #[arg(long, default_value = "master")]
        revision: String,
        #[arg(long, default_value = "auto")]
        source: String,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    /// List locally available models (legacy alias for `models list`).
    List,
    /// Manage the local model library.
    Models {
        #[command(subcommand)]
        command: ModelsCommand,
        /// Print machine-readable JSON.
        #[arg(long, global = true)]
        json: bool,
    },
    /// Load an installed model through a running server.
    Load {
        model: String,
        #[arg(long)]
        ctx: Option<u32>,
        #[arg(long)]
        kv: Option<String>,
        #[arg(long)]
        slots: Option<usize>,
        #[arg(long)]
        ttl: Option<u64>,
        #[arg(long)]
        alias: Option<String>,
    },
    /// Unload the current model (or all).
    Unload {
        #[arg(long)]
        all: bool,
        model: Option<String>,
    },
    /// Show models loaded in the running server.
    Ps,
    /// Start the OpenAI-compatible server.
    Serve {
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value = "mock")]
        backend: String,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        ctx: Option<u32>,
        #[arg(long)]
        slots: Option<usize>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        gui_dir: Option<PathBuf>,
        /// Start a WSL llama-server for the `upstream` backend if not running.
        #[arg(long)]
        start_wsl: bool,
        #[arg(long, default_value = "Ubuntu-24.04-Dev")]
        wsl_distro: String,
        #[arg(long, default_value = "/tmp/llama.cpp/build/bin/llama-server")]
        wsl_server: String,
        #[arg(long, default_value_t = 18080)]
        wsl_port: u16,
    },
    /// Run one prompt and print the response.
    Run {
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value = "mock")]
        backend: String,
        prompt: String,
        #[arg(long, default_value_t = 128)]
        max_tokens: u32,
        #[arg(long, default_value_t = 0)]
        ctx: u32,
    },
    /// Estimate KV/weights memory from GGUF metadata.
    Estimate {
        model: PathBuf,
        #[arg(long, default_value_t = 32768)]
        ctx: u32,
        #[arg(long, default_value = "q8_0")]
        kv: String,
        #[arg(long, default_value_t = 1)]
        slots: u32,
    },
    /// Run a concurrency benchmark.
    Bench {
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long, default_value = "mock")]
        backend: String,
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
        #[arg(long, default_value_t = 4)]
        iterations: usize,
        #[arg(long, default_value_t = 64)]
        max_tokens: u32,
    },
    /// Launch the Python training worker.
    Train {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "python")]
        python: String,
    },
    /// Print environment/installation diagnostics.
    Doctor,
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_env("LLMGUI_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    if let Err(error) = dispatch().await {
        eprintln!("error[{}]: {error}", error.code());
        std::process::exit(1);
    }
}

async fn dispatch() -> Result<()> {
    let cli = Cli::parse();
    if let Some(home) = cli.home {
        // SAFETY: single-threaded startup before any worker threads use it.
        unsafe { std::env::set_var("LLMGUI_HOME", home) };
    }
    let config = AppConfig::load(&paths::config_path())?;
    match cli.command {
        Commands::Pull {
            repo,
            revision,
            source,
            file,
            cache_dir,
        } => pull(&config, &repo, &revision, &source, file, cache_dir).await,
        Commands::List => list_models(),
        Commands::Models { command, json } => models_command(&config, command, json).await,
        Commands::Load {
            model,
            ctx,
            kv,
            slots,
            ttl,
            alias,
        } => admin_json(
            &config,
            "POST",
            &format!("/admin/models/{model}/load"),
            Some(serde_json::json!({
                "ctx": ctx, "kv": kv, "slots": slots, "ttl_secs": ttl, "alias": alias
            })),
        ),
        Commands::Unload { all, model } => {
            if all || model.is_none() {
                admin_json(&config, "POST", "/admin/models/unload", None)
            } else {
                admin_json(
                    &config,
                    "POST",
                    &format!("/admin/models/{}/unload", model.unwrap_or_default()),
                    None,
                )
            }
        }
        Commands::Ps => admin_json(&config, "GET", "/admin/models/loaded", None),
        Commands::Serve {
            model,
            backend,
            host,
            port,
            ctx,
            slots,
            api_key,
            gui_dir,
            start_wsl,
            wsl_distro,
            wsl_server,
            wsl_port,
        } => {
            serve(
                config, model, backend, host, port, ctx, slots, api_key, gui_dir, start_wsl,
                wsl_distro, wsl_server, wsl_port,
            )
            .await
        }
        Commands::Run {
            model,
            backend,
            prompt,
            max_tokens,
            ctx,
        } => run_one(model, backend, prompt, max_tokens, ctx).await,
        Commands::Estimate {
            model,
            ctx,
            kv,
            slots,
        } => estimate(&model, ctx, &kv, slots),
        Commands::Bench {
            model,
            backend,
            concurrency,
            iterations,
            max_tokens,
        } => bench(model, backend, concurrency, iterations, max_tokens).await,
        Commands::Train { config, python } => train(&config, &python).await,
        Commands::Doctor => doctor(),
    }
}

fn doctor() -> Result<()> {
    println!("llmgui {}", llmgui_core::VERSION);
    println!("home:    {}", paths::llmgui_home().display());
    println!("cache:   {}", paths::cache_dir().display());
    println!("models:  {}", paths::models_dir().display());
    let models = std::fs::read_dir(paths::models_dir())
        .map(|rd| rd.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    println!("model dirs: {models}");
    let python = std::process::Command::new("python")
        .args(["-c", "import llmgui_train; print(llmgui_train.__version__)"])
        .output();
    match python {
        Ok(out) if out.status.success() => println!(
            "python llmgui_train: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        ),
        _ => println!("python llmgui_train: not installed"),
    }
    let wsl = std::process::Command::new("wsl")
        .args(["-l", "-q"])
        .output();
    match wsl {
        Ok(out) if out.status.success() => println!(
            "wsl distros: {}",
            String::from_utf8_lossy(&out.stdout)
                .replace('\u{0}', "")
                .trim()
        ),
        _ => println!("wsl: unavailable"),
    }
    let upstream = std::env::var("LLMGUI_UPSTREAM_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:18080".to_string());
    match ureq::get(&format!("{}/health", upstream.trim_end_matches('/'))).call() {
        Ok(resp) if resp.status() < 400 => println!("upstream: healthy ({upstream})"),
        _ => println!("upstream: offline ({upstream})"),
    }
    Ok(())
}

async fn pull(
    config: &AppConfig,
    repo: &str,
    revision: &str,
    source: &str,
    file: Option<String>,
    cache_dir: Option<PathBuf>,
) -> Result<()> {
    let options = llmgui_download::DownloadOptions {
        revision: revision.to_string(),
        file,
        files: Vec::new(),
        concurrency: config.download.concurrency,
        chunk_size: config.download.chunk_size_mb * 1024 * 1024,
        cache_dir: cache_dir.or_else(|| config.download.cache_dir.clone()),
    };
    let source_impl: Box<dyn llmgui_download::Source> = match source {
        "modelscope" => Box::new(llmgui_download::ModelScopeSource::new()?),
        "hf-mirror" | "gitee-ai" => Box::new(llmgui_download::HfSource::new(source)?),
        "auto" => {
            let modelscope = llmgui_download::ModelScopeSource::new()?;
            match modelscope.list(repo, revision).await {
                Ok(_) => Box::new(modelscope),
                Err(error) => {
                    eprintln!("ModelScope unavailable ({error}); falling back to hf-mirror");
                    Box::new(llmgui_download::HfSource::new("hf-mirror")?)
                }
            }
        }
        other => {
            return Err(LlmguiError::Config(format!(
                "unknown source {other}; use auto|modelscope|hf-mirror|gitee-ai"
            )));
        }
    };
    eprintln!("pulling {repo}@{revision} from {source}...");
    let manifest = llmgui_download::pull(source_impl.as_ref(), repo, &options, None).await?;
    eprintln!(
        "downloaded {} ({} files, {})",
        manifest.id,
        manifest.files.len(),
        human_bytes(manifest.total_size())
    );
    let library = llmgui_server::library::LibraryState::from_env(&config.model.extra_dirs);
    match library.refresh() {
        Ok((_, report)) => {
            eprintln!(
                "registered model library (+{} ~{})",
                report.added, report.updated
            );
        }
        Err(error) => eprintln!("warning: downloaded but library refresh failed: {error}"),
    }
    Ok(())
}

fn list_models() -> Result<()> {
    let config = AppConfig::load(&paths::config_path()).unwrap_or_default();
    let library = llmgui_server::library::LibraryState::from_env(&config.model.extra_dirs);
    let registry = library.registry()?;
    if registry.models.is_empty() {
        println!(
            "no models installed under {}",
            paths::models_dir().display()
        );
        return Ok(());
    }
    for model in &registry.models {
        let alias = registry
            .prefs(&model.id)
            .alias
            .map(|alias| format!(" ({alias})"))
            .unwrap_or_default();
        println!(
            "{:<40} {:<12} {:<10} {}{}",
            model.id,
            model.quant,
            human_bytes(model.size_bytes),
            model.path.display(),
            alias
        );
    }
    Ok(())
}

async fn models_command(config: &AppConfig, command: ModelsCommand, json: bool) -> Result<()> {
    let library = llmgui_server::library::LibraryState::from_env(&config.model.extra_dirs);
    match command {
        ModelsCommand::List => {
            let registry = library.registry()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&registry)?);
            } else {
                for model in &registry.models {
                    let loaded = "";
                    println!(
                        "{:<44} {:<10} {:<10} {}{}",
                        model.id,
                        model.quant,
                        human_bytes(model.size_bytes),
                        if model.fit_32gb { "32G" } else { "-" },
                        loaded
                    );
                }
            }
            Ok(())
        }
        ModelsCommand::Show { id } => {
            let registry = library.registry()?;
            let model = registry
                .models
                .iter()
                .find(|model| model.id == id)
                .or_else(|| registry.get(&id))
                .ok_or_else(|| LlmguiError::NotFound(format!("model {id}")))?;
            let mut value = serde_json::to_value(model)?;
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "alias".into(),
                    serde_json::json!(registry.prefs(&model.id).alias),
                );
                object.insert(
                    "load_defaults".into(),
                    serde_json::json!(registry.prefs(&model.id).load_defaults),
                );
                object.insert(
                    "fit_matrix".into(),
                    serde_json::json!(
                        llmgui_models::library::fit_matrix(model).unwrap_or_default()
                    ),
                );
            }
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        ModelsCommand::Scan => {
            let (registry, report) = library.refresh()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "scan complete: +{} ~{} ={} ({} models)",
                    report.added,
                    report.updated,
                    report.skipped,
                    registry.models.len()
                );
            }
            Ok(())
        }
        ModelsCommand::Import {
            path,
            base_id,
            adapter_id,
        } => {
            let entry = if base_id.is_some() || adapter_id.is_some() {
                library.register_derived(&path, base_id.as_deref(), adapter_id.as_deref())?
            } else {
                library.import(&path)?
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&entry)?);
            } else {
                println!("imported {} -> {}", entry.path.display(), entry.id);
            }
            Ok(())
        }
        ModelsCommand::Remove { id, purge } => {
            let removal = library.remove(&id, purge)?;
            println!(
                "removed {}{}",
                removal.entry.id,
                if removal.files_removed {
                    " (files deleted)"
                } else {
                    ""
                }
            );
            Ok(())
        }
        ModelsCommand::Verify { id } => {
            let registry = library.registry()?;
            let targets: Vec<_> = registry
                .models
                .iter()
                .filter(|model| id.as_deref().is_none_or(|id| model.id == id))
                .cloned()
                .collect();
            if targets.is_empty() {
                return Err(LlmguiError::NotFound("no matching models".to_string()));
            }
            let mut failed = 0;
            for model in targets {
                for file in &model.files {
                    let Some(expected) = file.sha256.as_deref().filter(|hash| !hash.is_empty())
                    else {
                        continue;
                    };
                    let path = model.path.join(&file.path);
                    let actual = sha256_file(&path)?;
                    if actual == expected {
                        println!("ok   {} {}", model.id, file.path);
                    } else {
                        failed += 1;
                        println!(
                            "FAIL {} {} (expected {expected}, got {actual})",
                            model.id, file.path
                        );
                    }
                }
            }
            if failed > 0 {
                return Err(LlmguiError::Checksum {
                    path: "models".to_string(),
                    expected: "all hashes match".to_string(),
                    actual: format!("{failed} mismatch(es)"),
                });
            }
            Ok(())
        }
        ModelsCommand::Search { query } => {
            #[derive(serde::Deserialize, serde::Serialize)]
            struct Seed {
                #[serde(default)]
                models: Vec<SeedModel>,
            }
            #[derive(serde::Deserialize, serde::Serialize)]
            struct SeedModel {
                id: String,
                repo: String,
                #[serde(default)]
                quant: String,
                #[serde(default)]
                license: String,
                #[serde(default)]
                size_bytes: u64,
                #[serde(default)]
                role: String,
            }
            let seed: Seed = toml::from_str(include_str!("../../../models/v0.toml"))
                .unwrap_or(Seed { models: vec![] });
            let needle = query.to_lowercase();
            for model in seed.models.into_iter().filter(|model| {
                needle.is_empty()
                    || format!("{} {} {} {}", model.id, model.repo, model.quant, model.role)
                        .to_lowercase()
                        .contains(&needle)
            }) {
                println!(
                    "{:<32} {:<46} {:<10} {:<12} {}",
                    model.id,
                    model.repo,
                    model.quant,
                    human_bytes(model.size_bytes),
                    model.license
                );
            }
            Ok(())
        }
    }
}

fn sha256_file(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn api_base(config: &AppConfig) -> String {
    std::env::var("LLMGUI_API_BASE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("http://{}:{}", config.server.host, config.server.port))
}

fn admin_json(
    config: &AppConfig,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<()> {
    let url = format!("{}{}", api_base(config).trim_end_matches('/'), path);
    let mut request = ureq::request(method, &url);
    if let Some(key) = config.server.api_key.as_deref() {
        request = request.set("Authorization", &format!("Bearer {key}"));
    }
    let response = match body {
        Some(body) => request.send_json(body),
        None => request.call(),
    };
    match response {
        Ok(response) => {
            let text = response.into_string().unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{text}");
            }
            Ok(())
        }
        Err(ureq::Error::Status(code, response)) => {
            let detail = response.into_string().unwrap_or_default();
            Err(LlmguiError::Backend(format!(
                "server returned {code}: {detail}"
            )))
        }
        Err(error) => Err(LlmguiError::Backend(format!(
            "cannot reach {}: {error}; start `llmgui serve` first",
            api_base(config)
        ))),
    }
}

fn make_backend(backend: &str) -> Result<Arc<dyn Backend>> {
    match backend {
        "mock" => Ok(Arc::new(MockBackend::default())),
        "upstream" | "llama-server" => Ok(Arc::new(
            llmgui_backend_upstream::UpstreamBackend::from_env()?,
        )),
        "llama" | "llama.cpp" => Ok(Arc::new(llmgui_backend_llamacpp::LlamaCppBackend::new())),
        other => Err(LlmguiError::Config(format!(
            "unknown backend {other}; use mock|upstream|llama"
        ))),
    }
}

fn load_config_for_serve(
    mut config: AppConfig,
    ctx: Option<u32>,
    slots: Option<usize>,
    host: Option<String>,
    port: Option<u16>,
    api_key: Option<String>,
) -> AppConfig {
    if let Some(ctx) = ctx {
        config.model.ctx = ctx;
    }
    if let Some(slots) = slots {
        config.server.max_concurrent = slots;
    }
    if let Some(host) = host {
        config.server.host = host;
    }
    if let Some(port) = port {
        config.server.port = port;
    }
    if api_key.is_some() {
        config.server.api_key = api_key;
    }
    config
}

/// Ensure a WSL `llama-server` is healthy on `port`, starting it if necessary.
async fn ensure_wsl_server(
    model: &std::path::Path,
    distro: &str,
    server: &str,
    port: u16,
    ctx: u32,
) -> Result<String> {
    use std::process::Command;
    let url = format!("http://127.0.0.1:{port}");
    if ureq::get(&format!("{url}/health")).call().is_ok() {
        return Ok(url);
    }
    let converted = Command::new("wsl")
        .args(["-d", distro, "--", "wslpath", "-a"])
        .arg(model)
        .output()
        .map_err(|e| LlmguiError::Config(format!("wslpath: {e}")))?;
    if !converted.status.success() {
        return Err(LlmguiError::Config(format!(
            "wslpath failed: {}",
            String::from_utf8_lossy(&converted.stderr)
        )));
    }
    let model_wsl = String::from_utf8_lossy(&converted.stdout)
        .trim()
        .to_string();
    if model_wsl.is_empty() {
        return Err(LlmguiError::Config(
            "wslpath returned an empty model path".to_string(),
        ));
    }
    let log_dir = llmgui_core::paths::llmgui_home().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("wsl-llama-server-{port}.log"));
    let log_file = std::fs::File::create(&log_path)?;
    let stderr_file = log_file.try_clone()?;
    let mut child = Command::new("wsl")
        .args(["-d", distro, "--", "setsid", "nohup", server])
        .arg("-m")
        .arg(&model_wsl)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "-c",
            &ctx.to_string(),
            "-t",
            "8",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| LlmguiError::Config(format!("start wsl llama-server: {e}")))?;
    eprintln!(
        "starting WSL llama-server (wsl pid {}), log {}",
        child.id(),
        log_path.display()
    );
    for _ in 0..120 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if ureq::get(&format!("{url}/health")).call().is_ok() {
            return Ok(url);
        }
        if let Ok(Some(_)) = child.try_wait() {
            // `setsid` normally detaches; if wsl itself exited, keep polling:
            // the Linux server may still be loading the model.
        }
    }
    let _ = child.kill();
    let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();
    Err(LlmguiError::Backend(format!(
        "WSL llama-server did not become healthy on {url}; log {}: {}",
        log_path.display(),
        log_text.chars().take(800).collect::<String>()
    )))
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    config: AppConfig,
    model: Option<PathBuf>,
    backend: String,
    host: Option<String>,
    port: Option<u16>,
    ctx: Option<u32>,
    slots: Option<usize>,
    api_key: Option<String>,
    gui_dir: Option<PathBuf>,
    start_wsl: bool,
    wsl_distro: String,
    wsl_server: String,
    wsl_port: u16,
) -> Result<()> {
    let config = load_config_for_serve(config, ctx, slots, host, port, api_key);
    config.validate_bind()?;
    if start_wsl {
        if backend != "upstream" && backend != "llama-server" {
            return Err(LlmguiError::Config(
                "--start-wsl requires --backend upstream".to_string(),
            ));
        }
        let model_path = model.as_ref().ok_or_else(|| {
            LlmguiError::Config("--start-wsl requires --model <gguf>".to_string())
        })?;
        let url = ensure_wsl_server(
            model_path,
            &wsl_distro,
            &wsl_server,
            wsl_port,
            config.model.ctx,
        )
        .await?;
        // SAFETY: called during single-threaded CLI startup, before serving.
        unsafe { std::env::set_var("LLMGUI_UPSTREAM_URL", &url) };
        eprintln!("upstream ready at {url}");
    }
    let backend_impl = make_backend(&backend)?;
    let model_id = model
        .as_ref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or(if backend == "upstream" || backend == "llama-server" {
            "upstream"
        } else {
            "mock"
        })
        .to_string();
    let load = LoadConfig {
        n_ctx: config.model.ctx,
        n_batch: config.model.n_batch,
        n_gpu_layers: config.model.gpu_layers,
        n_threads: config.model.n_threads,
        kv_type: config.model.kv_type.clone(),
        slots: config.server.max_concurrent,
    };
    if let Some(path) = &model {
        backend_impl.load(path, &load)?;
    } else if backend == "upstream" || backend == "llama-server" {
        backend_impl.load(std::path::Path::new("upstream"), &load)?;
    }
    let gui = gui_dir
        .or_else(|| std::env::var("LLMGUI_GUI_DIR").ok().map(PathBuf::from))
        .or_else(|| {
            let candidate = PathBuf::from("gui");
            candidate.exists().then_some(candidate)
        })
        .or_else(|| config.server.gui_dir.clone());
    let state = llmgui_server::app_state(backend_impl, config.clone(), model_id.clone());
    if let Some(path) = &model {
        let defaults = llmgui_models::library::LoadDefaults {
            ctx: config.model.ctx,
            kv: config.model.kv_type.clone(),
            gpu_layers: config.model.gpu_layers,
            slots: config.server.max_concurrent,
            ttl_secs: config.model.idle_ttl_secs,
        };
        state
            .runtime
            .adopt_loaded(model_id.clone(), model_id.clone(), path.clone(), &defaults)
            .await;
    }
    llmgui_server::serve(state, gui).await
}

async fn run_one(
    model: Option<PathBuf>,
    backend: String,
    prompt: String,
    max_tokens: u32,
    ctx: u32,
) -> Result<()> {
    let backend_impl = make_backend(&backend)?;
    if let Some(path) = &model {
        let mut load = LoadConfig::default();
        if ctx > 0 {
            load.n_ctx = ctx;
        }
        backend_impl.load(path, &load)?;
    } else if backend == "upstream" || backend == "llama-server" {
        let mut load = LoadConfig::default();
        if ctx > 0 {
            load.n_ctx = ctx;
        }
        backend_impl.load(std::path::Path::new("upstream"), &load)?;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let request = GenerateRequest {
        request_id: "run".to_string(),
        prompt: prompt.clone(),
        max_tokens,
        temperature: 0.7,
        top_p: 0.95,
        stop: vec![],
        seed: None,
        cache_key: String::new(),
    };
    let backend_clone = backend_impl.clone();
    tokio::task::spawn_blocking(move || backend_clone.generate(request, cancel, tx))
        .await
        .map_err(|e| LlmguiError::Backend(format!("join error: {e}")))??;
    while let Some(event) = rx.recv().await {
        match event {
            TokenEvent::Token { text, .. } => print!("{text}"),
            TokenEvent::Done { stats } => {
                println!(
                    "\n[{} tokens, {:.1} tok/s, ttft {} ms]",
                    stats.completion_tokens, stats.tokens_per_second, stats.ttft_ms
                );
            }
            TokenEvent::Error { message, code } => {
                return Err(LlmguiError::Backend(format!("{code}: {message}")));
            }
        }
    }
    Ok(())
}

fn estimate(model: &std::path::Path, ctx: u32, kv: &str, slots: u32) -> Result<()> {
    let info = GgufInfo::open(model)?;
    let estimate = info.estimate(ctx, KvCacheType::parse(kv), slots);
    println!("{}", info.summary());
    println!("weights:       {}", human_bytes(estimate.weights_bytes));
    println!(
        "kv cache:      {} ({} B/token × {} ctx × {} slots)",
        human_bytes(estimate.kv_bytes),
        estimate.kv_bytes_per_token,
        ctx,
        slots
    );
    println!("compute:       {}", human_bytes(estimate.compute_bytes));
    println!("runtime:       {}", human_bytes(estimate.runtime_bytes));
    println!("TOTAL:         {}", human_bytes(estimate.total_bytes));
    println!(
        "fits 24GB: {} | fits 32GB: {}",
        estimate.fits_24gb, estimate.fits_32gb
    );
    Ok(())
}

async fn bench(
    model: Option<PathBuf>,
    backend: String,
    concurrency: usize,
    iterations: usize,
    max_tokens: u32,
) -> Result<()> {
    let backend_impl = make_backend(&backend)?;
    if let Some(path) = &model {
        backend_impl.load(path, &LoadConfig::default())?;
    } else if backend == "upstream" || backend == "llama-server" {
        backend_impl.load(std::path::Path::new("upstream"), &LoadConfig::default())?;
    }
    let scheduler = Arc::new(Scheduler::new(backend_impl, concurrency));
    let report = llmgui_bench::run(
        scheduler,
        "local",
        "请用一句话介绍你自己。",
        max_tokens,
        concurrency,
        iterations,
    )
    .await;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

async fn train(config: &std::path::Path, python: &str) -> Result<()> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut child = tokio::process::Command::new(python)
        .args(["-m", "llmgui_train.worker"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| LlmguiError::Config(format!("spawn python worker: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| LlmguiError::Config("worker stdin unavailable".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| LlmguiError::Config("worker stdout unavailable".to_string()))?;
    let hello = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "hello-1",
        "method": "hello",
        "params": {"protocol": 1}
    });
    stdin
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .map_err(|e| LlmguiError::Config(format!("write hello: {e}")))?;
    stdin
        .flush()
        .await
        .map_err(|e| LlmguiError::Config(format!("flush hello: {e}")))?;
    let cmd = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "train-1",
        "method": "train.start",
        "params": {"config_path": config.to_string_lossy()}
    });
    stdin
        .write_all(format!("{cmd}\n").as_bytes())
        .await
        .map_err(|e| LlmguiError::Config(format!("write train.start: {e}")))?;
    stdin
        .flush()
        .await
        .map_err(|e| LlmguiError::Config(format!("flush train.start: {e}")))?;
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| LlmguiError::Config(format!("read worker: {e}")))?
    {
        if line.contains("\"event\":\"hello_ok\"") {
            continue;
        }
        println!("{line}");
        if line.contains("\"done\"") || line.contains("\"error\"") {
            break;
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|e| LlmguiError::Config(format!("wait worker: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(LlmguiError::Backend(format!(
            "training worker exited: {status}"
        )))
    }
}
