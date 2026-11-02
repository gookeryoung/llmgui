//! Tauri 2 desktop shell for the llmgui local runtime.
//!
//! The shell deliberately contains no product logic: it (optionally) starts
//! `llmgui serve`, waits for `/healthz`, and then loads the zero-build Web GUI
//! from the server origin so the page's relative `/v1/...` calls keep working.
//!
//! `--selftest` turns the shell into an end-to-end acceptance driver: the
//! injected script (`assets/selftest.js`) drives the real UI in the real
//! webview (chat round trip through the HTTP API) and reports the DOM state
//! back over a loopback socket.

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default OpenAI-compatible endpoint served by `llmgui serve`.
pub const DEFAULT_API_BASE: &str = "http://127.0.0.1:11435";
/// Prompt used by `--selftest` when none is given.
pub const DEFAULT_SELFTEST_PROMPT: &str = "用一句话介绍你自己。";
/// Default budget for a full `--selftest` round trip (model load can be slow).
pub const DEFAULT_SELFTEST_TIMEOUT_SECS: u64 = 180;

/// Command line interface of the desktop shell.
#[derive(Debug, Clone, clap::Parser)]
#[command(
    name = "llmgui-desktop",
    version,
    about = "llmgui 桌面壳（Tauri 2）：启动本地运行时并加载 Web GUI"
)]
pub struct Cli {
    /// API base of the local runtime; the window loads this origin.
    #[arg(long, env = "LLMGUI_API_BASE", default_value = DEFAULT_API_BASE)]
    pub api_base: String,
    /// `llmgui` binary used to start the runtime (default: PATH or target/).
    #[arg(long, env = "LLMGUI_BIN")]
    pub server_bin: Option<PathBuf>,
    /// Do not start a runtime; only attach to an already running one.
    #[arg(long)]
    pub no_spawn: bool,
    /// Acceptance mode: drive the GUI and report the result (exits non-zero on failure).
    #[arg(long)]
    pub selftest: bool,
    /// Prompt typed into the chat composer by `--selftest`.
    #[arg(long, default_value = DEFAULT_SELFTEST_PROMPT)]
    pub selftest_prompt: String,
    /// Write the `--selftest` JSON result to this file as well as stdout.
    #[arg(long)]
    pub selftest_report: Option<PathBuf>,
    /// Overall `--selftest` budget in seconds.
    #[arg(long, default_value_t = DEFAULT_SELFTEST_TIMEOUT_SECS)]
    pub selftest_timeout_secs: u64,
    /// Keep the window open for this many seconds after `--selftest` finishes.
    #[arg(long, default_value_t = 0)]
    pub selftest_hold_secs: u64,
    /// Page left on screen when `--selftest` finishes (for screenshots/eyeballing).
    #[arg(long, value_enum, default_value_t = FinalView::Models)]
    pub selftest_final_view: FinalView,
    /// Arguments passed to the `llmgui` binary verbatim, e.g.
    /// `llmgui-desktop -- serve --backend upstream --model model.gguf`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub serve_args: Vec<String>,
}

/// Page the shell leaves on screen after a `--selftest` run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FinalView {
    Chat,
    Models,
}

impl FinalView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Models => "models",
        }
    }
}

/// Resolved `--selftest` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelftestConfig {
    pub prompt: String,
    pub timeout: Duration,
    pub hold: Duration,
    pub report: Option<PathBuf>,
    pub final_view: FinalView,
}

impl Cli {
    /// Normalised API base (no trailing slash, default when empty).
    pub fn api_base(&self) -> String {
        normalize_api_base(&self.api_base)
    }

    /// Port of [`Cli::api_base`], used for the default `serve` arguments.
    pub fn api_port(&self) -> u16 {
        parse_api_port(&self.api_base()).unwrap_or(11435)
    }

    /// Arguments for the spawned `llmgui` binary.
    pub fn serve_args(&self) -> Vec<String> {
        if !self.serve_args.is_empty() {
            return self.serve_args.clone();
        }
        vec![
            "serve".to_string(),
            "--backend".to_string(),
            "mock".to_string(),
            "--port".to_string(),
            self.api_port().to_string(),
        ]
    }

    /// `Some(config)` when `--selftest` is enabled.
    pub fn selftest(&self) -> Option<SelftestConfig> {
        self.selftest.then(|| SelftestConfig {
            prompt: self.selftest_prompt.clone(),
            timeout: Duration::from_secs(self.selftest_timeout_secs.max(1)),
            hold: Duration::from_secs(self.selftest_hold_secs),
            report: self.selftest_report.clone(),
            final_view: self.selftest_final_view,
        })
    }
}

/// Trim whitespace and trailing slashes; empty input falls back to the default.
pub fn normalize_api_base(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        DEFAULT_API_BASE.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Extract the port from an API base (`https` defaults to 443, others to 80).
pub fn parse_api_port(base: &str) -> Option<u16> {
    let without_scheme = match base.split_once("://") {
        Some((scheme, rest)) => {
            if scheme.eq_ignore_ascii_case("https") {
                return parse_host_port(rest, 443);
            }
            rest
        }
        None => base,
    };
    parse_host_port(without_scheme, 80)
}

fn parse_host_port(authority: &str, default_port: u16) -> Option<u16> {
    let authority = authority.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let is_port = authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()));
    if is_port {
        return authority
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse().ok());
    }
    Some(default_port)
}

/// Join an API base and a path (`"/healthz"` or `"healthz"`).
pub fn api_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        normalize_api_base(base),
        path.trim_start_matches('/')
    )
}

/// Locate the `llmgui` binary: explicit flag, env var, `PATH`, then `target/`.
pub fn resolve_server_bin(
    explicit: Option<&Path>,
    env_bin: Option<&Path>,
    cwd: &Path,
) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    if let Some(path) = env_bin {
        return Some(path.to_path_buf());
    }
    let exe = if cfg!(windows) {
        "llmgui.exe"
    } else {
        "llmgui"
    };
    if let Some(found) = find_on_path(exe) {
        return Some(found);
    }
    for profile in ["release", "debug"] {
        let candidate = cwd.join("target").join(profile).join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

/// Poll `/healthz` until it answers or `timeout` elapses.
pub fn wait_for_health(base: &str, timeout: Duration) -> bool {
    let url = api_url(base, "/healthz");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(500))
        .timeout(Duration::from_secs(2))
        .build();
    let deadline = Instant::now() + timeout;
    loop {
        if agent.get(&url).call().is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Build the injected acceptance script with the report URL and prompt baked in.
pub fn selftest_script(
    report_url: &str,
    prompt: &str,
    timeout: Duration,
    final_view: FinalView,
) -> String {
    let prompt_json = serde_json::to_string(prompt).unwrap_or_else(|_| "\"\"".to_string());
    include_str!("../assets/selftest.js")
        .replace("__REPORT_URL__", report_url)
        .replace("__PROMPT_JSON__", &prompt_json)
        .replace("__TIMEOUT_MS__", &timeout.as_millis().to_string())
        .replace("__FINAL_VIEW__", final_view.as_str())
}

/// Read one HTTP request from `stream` and return its body (empty when none).
pub fn read_http_body(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut content_length = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            content_length = if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse().unwrap_or(0)
            } else {
                content_length
            };
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        std::io::Read::read_exact(&mut reader, &mut body)?;
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Parse the JSON payload posted by the injected script.
pub fn parse_report(body: &str) -> Option<serde_json::Value> {
    serde_json::from_str(body.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn cli(args: &[&str]) -> Cli {
        use clap::Parser;
        let mut argv = vec!["llmgui-desktop"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).expect("cli parses")
    }

    #[test]
    fn normalizes_api_base() {
        assert_eq!(normalize_api_base("  "), DEFAULT_API_BASE);
        assert_eq!(
            normalize_api_base("http://127.0.0.1:9000///"),
            "http://127.0.0.1:9000"
        );
        assert_eq!(
            normalize_api_base(DEFAULT_API_BASE),
            DEFAULT_API_BASE.to_string()
        );
    }

    #[test]
    fn parses_ports() {
        assert_eq!(parse_api_port("http://127.0.0.1:11435"), Some(11435));
        assert_eq!(parse_api_port("http://127.0.0.1"), Some(80));
        assert_eq!(parse_api_port("https://llmgui.local"), Some(443));
        assert_eq!(parse_api_port("https://llmgui.local:8443/v1"), Some(8443));
        assert_eq!(parse_api_port("127.0.0.1:1234"), Some(1234));
        assert_eq!(parse_api_port("http://[::1]:11435"), Some(11435));
    }

    #[test]
    fn joins_api_urls() {
        assert_eq!(
            api_url("http://127.0.0.1:11435/", "/healthz"),
            "http://127.0.0.1:11435/healthz"
        );
        assert_eq!(
            api_url("http://127.0.0.1:11435", "v1/models"),
            "http://127.0.0.1:11435/v1/models"
        );
    }

    #[test]
    fn default_serve_args_follow_api_base_port() {
        let parsed = cli(&["--api-base", "http://127.0.0.1:19435"]);
        assert_eq!(parsed.api_port(), 19435);
        assert_eq!(
            parsed.serve_args(),
            vec!["serve", "--backend", "mock", "--port", "19435"]
        );
    }

    #[test]
    fn serve_args_are_passed_through_verbatim() {
        let parsed = cli(&[
            "--",
            "serve",
            "--backend",
            "upstream",
            "--model",
            "/models/merged-7b-q4km.gguf",
        ]);
        assert_eq!(
            parsed.serve_args(),
            vec![
                "serve",
                "--backend",
                "upstream",
                "--model",
                "/models/merged-7b-q4km.gguf"
            ]
        );
    }

    #[test]
    fn selftest_defaults_are_off() {
        assert!(cli(&[]).selftest().is_none());
        let config = cli(&[
            "--selftest",
            "--selftest-prompt",
            "你好",
            "--selftest-timeout-secs",
            "7",
            "--selftest-hold-secs",
            "3",
            "--selftest-final-view",
            "chat",
        ])
        .selftest()
        .expect("selftest enabled");
        assert_eq!(config.prompt, "你好");
        assert_eq!(config.timeout, Duration::from_secs(7));
        assert_eq!(config.hold, Duration::from_secs(3));
        assert_eq!(config.final_view, FinalView::Chat);
        assert!(config.report.is_none());
        assert_eq!(
            cli(&["--selftest"]).selftest().expect("enabled").final_view,
            FinalView::Models
        );
    }

    #[test]
    fn selftest_timeout_never_zero() {
        let config = cli(&["--selftest", "--selftest-timeout-secs", "0"])
            .selftest()
            .expect("selftest enabled");
        assert_eq!(config.timeout, Duration::from_secs(1));
    }

    #[test]
    fn explicit_server_bin_wins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let explicit = tmp.path().join("llmgui-explicit");
        let from_env = tmp.path().join("llmgui-env");
        assert_eq!(
            resolve_server_bin(Some(&explicit), Some(&from_env), tmp.path()),
            Some(explicit)
        );
        assert_eq!(
            resolve_server_bin(None, Some(&from_env), tmp.path()),
            Some(from_env)
        );
    }

    #[test]
    fn falls_back_to_target_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let exe = if cfg!(windows) {
            "llmgui.exe"
        } else {
            "llmgui"
        };
        let debug = tmp.path().join("target").join("debug").join(exe);
        std::fs::create_dir_all(debug.parent().expect("parent")).expect("mkdir");
        std::fs::write(&debug, b"stub").expect("write");
        let resolved = resolve_server_bin(None, None, tmp.path());
        if let Some(path) = resolved {
            assert!(path.ends_with(Path::new("target").join("debug").join(exe)));
        }
    }

    #[test]
    fn selftest_script_embeds_inputs() {
        let script = selftest_script(
            "http://127.0.0.1:4321/report",
            "包含 \"引号\" 的提示",
            Duration::from_secs(9),
            FinalView::Chat,
        );
        assert!(script.contains("http://127.0.0.1:4321/report"));
        assert!(script.contains("9000"));
        assert!(script.contains("var FINAL_VIEW = 'chat'"));
        assert!(!script.contains("__PROMPT_JSON__"));
        assert!(script.contains("包含"));
        assert!(script.contains("\\\"引号\\\""));
    }

    #[test]
    fn reads_http_body_and_parses_report() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            read_http_body(&mut stream).expect("body")
        });
        let body = "{\"ok\":true}";
        let request = format!(
            "POST /report HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut client = TcpStream::connect(addr).expect("connect");
        std::io::Write::write_all(&mut client, request.as_bytes()).expect("write");
        let received = handle.join().expect("join");
        assert_eq!(received, body);
        assert_eq!(
            parse_report(&received).and_then(|value| value["ok"].as_bool()),
            Some(true)
        );
        assert!(parse_report("not json").is_none());
    }

    #[test]
    fn health_wait_times_out_on_dead_port() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let started = Instant::now();
        assert!(!wait_for_health(
            &format!("http://127.0.0.1:{port}"),
            Duration::from_millis(300)
        ));
        assert!(started.elapsed() >= Duration::from_millis(250));
    }
}
