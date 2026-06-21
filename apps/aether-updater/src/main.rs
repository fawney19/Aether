use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use std::{
    collections::VecDeque,
    env,
    io::{BufRead, BufReader},
    net::SocketAddr,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

const DEFAULT_BIND: &str = "0.0.0.0:8099";
const DEFAULT_SCRIPT: &str = "/aether-deploy/update.sh";
const DEFAULT_WORKDIR: &str = "/aether-deploy";
const DEFAULT_TAIL_LINES: usize = 200;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    run: Arc<Mutex<RunState>>,
}

struct Config {
    token: String,
    script: PathBuf,
    workdir: PathBuf,
    tail_lines: usize,
}

#[derive(Clone, Serialize)]
struct RunState {
    status: &'static str,
    started_at: Option<String>,
    finished_at: Option<String>,
    exit_code: Option<i32>,
    error: Option<String>,
    output_tail: VecDeque<String>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            status: "idle",
            started_at: None,
            finished_at: None,
            exit_code: None,
            error: None,
            output_tail: VecDeque::new(),
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aether_updater=info,tower_http=info".into()),
        )
        .init();

    let config = match load_config() {
        Ok(config) => Arc::new(config),
        Err(err) => {
            eprintln!("aether-updater configuration error: {err}");
            std::process::exit(2);
        }
    };
    let bind = env::var("AETHER_UPDATER_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_string());
    let addr: SocketAddr = bind
        .parse()
        .unwrap_or_else(|err| panic!("invalid AETHER_UPDATER_BIND {bind}: {err}"));

    let state = AppState {
        config,
        run: Arc::new(Mutex::new(RunState::default())),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/run", post(run_update))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind updater listener");
    tracing::info!(%addr, "aether updater listening");
    axum::serve(listener, app).await.expect("serve updater");
}

fn load_config() -> Result<Config, String> {
    let token = env::var("AETHER_UPDATER_TOKEN")
        .map_err(|_| "AETHER_UPDATER_TOKEN is required".to_string())?;
    if token.trim().is_empty() {
        return Err("AETHER_UPDATER_TOKEN must not be empty".to_string());
    }
    let tail_lines = env::var("AETHER_UPDATER_TAIL_LINES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TAIL_LINES);
    Ok(Config {
        token,
        script: env::var("AETHER_UPDATER_SCRIPT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_SCRIPT)),
        workdir: env::var("AETHER_UPDATER_WORKDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_WORKDIR)),
        tail_lines,
    })
}

async fn health() -> impl IntoResponse {
    Json(json!({ "ok": true }))
}

async fn status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    let run = state.run.lock().expect("run state lock").clone();
    Json(run).into_response()
}

async fn run_update(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }

    {
        let mut run = state.run.lock().expect("run state lock");
        if run.status == "running" {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "detail": "update_already_running" })),
            )
                .into_response();
        }
        *run = RunState {
            status: "running",
            started_at: Some(Utc::now().to_rfc3339()),
            finished_at: None,
            exit_code: None,
            error: None,
            output_tail: VecDeque::new(),
        };
    }

    let worker_state = state.clone();
    thread::spawn(move || execute_update(worker_state));

    Json(json!({ "started": true, "status": "running" })).into_response()
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    let explicit = headers
        .get("x-aether-updater-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    if bearer == Some(state.config.token.as_str()) || explicit == Some(state.config.token.as_str()) {
        return Ok(());
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({ "detail": "invalid_updater_token" })),
    )
        .into_response())
}

fn execute_update(state: AppState) {
    let result = run_fixed_script(&state);
    let mut run = state.run.lock().expect("run state lock");
    run.finished_at = Some(Utc::now().to_rfc3339());
    match result {
        Ok(code) if code == 0 => {
            run.status = "succeeded";
            run.exit_code = Some(code);
        }
        Ok(code) => {
            run.status = "failed";
            run.exit_code = Some(code);
            run.error = Some(format!("update script exited with code {code}"));
        }
        Err(err) => {
            run.status = "failed";
            run.error = Some(err);
        }
    }
}

fn run_fixed_script(state: &AppState) -> Result<i32, String> {
    if !state.config.script.is_file() {
        return Err(format!(
            "update script not found: {}",
            state.config.script.display()
        ));
    }
    if !state.config.workdir.is_dir() {
        return Err(format!(
            "update workdir not found: {}",
            state.config.workdir.display()
        ));
    }

    append_output(
        state,
        format!(">>> Starting {}", state.config.script.display()),
    );
    let mut child = Command::new(&state.config.script)
        .current_dir(&state.config.workdir)
        .env("AETHER_UPDATER_MANAGED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to spawn update script: {err}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let mut readers = Vec::new();
    if let Some(stdout) = stdout {
        let state = state.clone();
        readers.push(thread::spawn(move || read_stream_lines(state, stdout)));
    }
    if let Some(stderr) = stderr {
        let state = state.clone();
        readers.push(thread::spawn(move || read_stream_lines(state, stderr)));
    }

    let status = child
        .wait()
        .map_err(|err| format!("failed to wait for update script: {err}"))?;
    for reader in readers {
        let _ = reader.join();
    }
    Ok(status.code().unwrap_or(1))
}

fn read_stream_lines<R>(state: AppState, reader: R)
where
    R: std::io::Read,
{
    for line in BufReader::new(reader).lines() {
        match line {
            Ok(line) => append_output(&state, line),
            Err(err) => {
                append_output(&state, format!("failed to read update output: {err}"));
                break;
            }
        }
    }
}

fn append_output(state: &AppState, line: String) {
    let mut run = state.run.lock().expect("run state lock");
    run.output_tail.push_back(line);
    while run.output_tail.len() > state.config.tail_lines {
        run.output_tail.pop_front();
    }
}
