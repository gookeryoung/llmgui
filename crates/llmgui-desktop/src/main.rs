//! `llmgui-desktop`: Tauri 2 shell that starts the local runtime and shows the
//! Web GUI, plus an automated end-to-end acceptance mode (`--selftest`).

use std::io::Write;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use llmgui_desktop::{
    Cli, SelftestConfig, api_url, read_http_body, resolve_server_bin, selftest_script,
    wait_for_health,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// How long the shell waits for a freshly spawned runtime to answer `/healthz`.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);

fn main() {
    if let Err(error) = run() {
        eprintln!("llmgui-desktop: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let api_base = cli.api_base();

    let child = if cli.no_spawn {
        None
    } else {
        Some(spawn_runtime(&cli)?)
    };
    let child = Arc::new(Mutex::new(child));

    if !wait_for_health(&api_base, HEALTH_TIMEOUT) {
        eprintln!(
            "llmgui-desktop: {api_base} did not become healthy within {}s (set --no-spawn to attach to a running runtime)",
            HEALTH_TIMEOUT.as_secs()
        );
        kill(&child);
        return Err("runtime health check failed".into());
    }

    let exit_code = Arc::new(AtomicI32::new(0));
    let selftest = cli.selftest();

    let app = tauri::Builder::default()
        .setup({
            let api_base = api_base.clone();
            let exit_code = Arc::clone(&exit_code);
            move |app| {
                let reporter = match selftest.clone() {
                    Some(config) => Some(Reporter::start(
                        app.handle().clone(),
                        config.clone(),
                        Arc::clone(&exit_code),
                    )?),
                    None => None,
                };

                let url = tauri::Url::parse(&api_url(&api_base, "/"))?;
                let eval_script = reporter.as_ref().map(|reporter| {
                    selftest_script(
                        &reporter.report_url,
                        &config_prompt(&selftest),
                        config_timeout(&selftest),
                        config_final_view(&selftest),
                    )
                });

                let mut builder =
                    tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
                        .title("llmgui · 本地大模型控制台")
                        .inner_size(1280.0, 860.0)
                        .min_inner_size(960.0, 640.0);

                if let Some(script) = eval_script {
                    builder = builder.on_page_load(move |window, payload| {
                        if payload.event() == tauri::webview::PageLoadEvent::Finished {
                            let _ = window.eval(script.clone());
                        }
                    });
                }

                builder.build()?;
                Ok(())
            }
        })
        .build(tauri::generate_context!())?;

    let exit_code_after_run = Arc::clone(&exit_code);
    app.run(move |_app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            kill(&child);
        }
    });

    let code = exit_code_after_run.load(Ordering::SeqCst);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn config_prompt(selftest: &Option<SelftestConfig>) -> String {
    selftest
        .as_ref()
        .map(|config| config.prompt.clone())
        .unwrap_or_else(|| llmgui_desktop::DEFAULT_SELFTEST_PROMPT.to_string())
}

fn config_final_view(selftest: &Option<SelftestConfig>) -> llmgui_desktop::FinalView {
    selftest
        .as_ref()
        .map(|config| config.final_view)
        .unwrap_or(llmgui_desktop::FinalView::Models)
}

fn config_timeout(selftest: &Option<SelftestConfig>) -> Duration {
    selftest
        .as_ref()
        .map(|config| config.timeout)
        .unwrap_or_else(|| Duration::from_secs(llmgui_desktop::DEFAULT_SELFTEST_TIMEOUT_SECS))
}

fn spawn_runtime(cli: &Cli) -> Result<Child> {
    let cwd = std::env::current_dir()?;
    let env_bin = cli.server_bin.clone();
    let binary = resolve_server_bin(None, env_bin.as_deref(), &cwd).ok_or_else(|| {
        "llmgui binary not found: pass --server-bin, set LLMGUI_BIN, or build `cargo build -p llmgui-cli`"
            .to_string()
    })?;
    let args = cli.serve_args();
    eprintln!(
        "llmgui-desktop: starting {} {}",
        binary.display(),
        args.join(" ")
    );
    let child = Command::new(&binary)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok(child)
}

fn kill(child: &Arc<Mutex<Option<Child>>>) {
    let Ok(mut guard) = child.lock() else {
        return;
    };
    let Some(mut process) = guard.take() else {
        return;
    };
    let _ = process.kill();
    let _ = process.wait();
}

/// Loopback endpoint the injected script posts its result to.
struct Reporter {
    report_url: String,
}

impl Reporter {
    fn start(
        app: tauri::AppHandle,
        config: SelftestConfig,
        exit_code: Arc<AtomicI32>,
    ) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let deadline = Instant::now() + config.timeout + Duration::from_secs(15);

        std::thread::spawn(move || {
            let mut received = None;
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let value = read_http_body(&mut stream)
                            .ok()
                            .and_then(|body| llmgui_desktop::parse_report(&body));
                        if let Some(value) = value {
                            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
                            received = Some(value);
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }

            let (report, ok) = match received {
                Some(value) => {
                    let ok = value["ok"].as_bool().unwrap_or(false);
                    (value, ok)
                }
                None => (
                    serde_json::json!({
                        "ok": false,
                        "errors": ["no result received from the webview within the timeout"],
                    }),
                    false,
                ),
            };

            let payload =
                serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string());
            println!("SELFTEST_RESULT {payload}");
            if let Some(path) = &config.report {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(path, &payload);
            }
            exit_code.store(if ok { 0 } else { 1 }, Ordering::SeqCst);

            if !config.hold.is_zero() {
                std::thread::sleep(config.hold);
            }
            app.exit(exit_code.load(Ordering::SeqCst));
        });

        Ok(Self {
            report_url: format!("http://127.0.0.1:{port}/report"),
        })
    }
}
