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
    /// List locally available models.
    List,
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
        Commands::Serve {
            model,
            backend,
            host,
            port,
            ctx,
            slots,
            api_key,
            gui_dir,
        } => {
            serve(
                config, model, backend, host, port, ctx, slots, api_key, gui_dir,
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
    }
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
    Ok(())
}

fn list_models() -> Result<()> {
    let dir = paths::models_dir();
    std::fs::create_dir_all(&dir)?;
    let mut found = false;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path().join("manifest.json");
        if path.exists()
            && let Ok(manifest) = llmgui_core::manifest::ModelManifest::load(&path)
        {
            found = true;
            println!(
                "{:<40} {:<12} {:<10} {}",
                manifest.id,
                manifest.quant.kind,
                human_bytes(manifest.total_size()),
                entry.path().display()
            );
        }
    }
    if !found {
        println!("no models installed under {}", dir.display());
    }
    Ok(())
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
) -> Result<()> {
    let config = load_config_for_serve(config, ctx, slots, host, port, api_key);
    config.validate_bind()?;
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
    let state = llmgui_server::app_state(backend_impl, config, model_id);
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
