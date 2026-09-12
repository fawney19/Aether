use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use aether_contracts::ExecutionPlan;
use aether_runtime::{
    maybe_hold_axum_response_permit, prometheus_response, service_up_sample, AdmissionPermit,
    ConcurrencyError, ConcurrencyGate, ConcurrencySnapshot, MetricKind, MetricLabel, MetricSample,
};
use aether_runtime_state::{RuntimeSemaphore, RuntimeSemaphoreError, RuntimeSemaphoreSnapshot};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder as HyperServerBuilder;
use hyper_util::service::TowerToHyperService;
use serde_json::json;
use thiserror::Error;
use tower::{Service as _, ServiceExt as _};

use crate::execution_runtime::{
    build_direct_execution_frame_stream, DirectSyncExecutionRuntime, ExecutionRuntimeTransportError,
};
use crate::middleware;

const EXECUTION_RUNTIME_COMPONENT: &str = "aether-gateway-execution-runtime";
const REQUEST_GATE_NAME: &str = "execution_runtime_requests";
const DISTRIBUTED_REQUEST_GATE_NAME: &str = "execution_runtime_requests_distributed";

// These limits protect only connection metadata.  Once the first complete
// request reaches the service, request and response bodies remain fully
// streaming (including long-lived streaming responses).  Keep the HTTP/2
// stream default high enough for high-concurrency hosts; this is not a body
// admission limit.
const EXECUTION_RUNTIME_HTTP2_MAX_CONCURRENT_STREAMS: u32 = 16_384;
const EXECUTION_RUNTIME_HTTP_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
const EXECUTION_RUNTIME_HTTP_HEADER_MAX_BYTES: usize = 64 * 1024;
const EXECUTION_RUNTIME_HTTP_MAX_HEADERS: usize = 256;
const EXECUTION_RUNTIME_REQUEST_BODY_HARD_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// Coordinates the connection-level deadline that covers protocol detection
/// and the first request header block. Hyper's HTTP/1 timer starts only after
/// the auto protocol detector has finished, while HTTP/2 has no equivalent
/// header timer. Keeping this gate outside the parser closes that initial gap
/// without imposing a deadline on request or response bodies.
#[derive(Clone)]
struct ExecutionRuntimeFirstRequestGate {
    seen: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl ExecutionRuntimeFirstRequestGate {
    fn new() -> Self {
        Self {
            seen: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn mark_seen(&self) {
        if !self.seen.swap(true, Ordering::Release) {
            self.notify.notify_one();
        }
    }

    fn is_seen(&self) -> bool {
        self.seen.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct ExecutionRuntimeFirstRequestService<S> {
    inner: S,
    gate: ExecutionRuntimeFirstRequestGate,
}

impl<S, Req> tower::Service<Req> for ExecutionRuntimeFirstRequestService<S>
where
    S: tower::Service<Req>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Req) -> Self::Future {
        self.gate.mark_seen();
        self.inner.call(request)
    }
}

/// Drive one Hyper connection while enforcing the deadline for its first
/// request.  The timeout is deliberately limited to protocol detection and
/// initial headers; after `gate.mark_seen()` body and response streaming are
/// not interrupted by this helper.
async fn drive_execution_runtime_connection<F, E>(
    connection: F,
    gate: ExecutionRuntimeFirstRequestGate,
    timeout: Duration,
) -> Result<(), E>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    if gate.is_seen() {
        return connection.await;
    }

    let mut connection = Box::pin(connection);
    let timeout = tokio::time::sleep(timeout);
    tokio::pin!(timeout);
    let notified = gate.notify.notified();
    tokio::pin!(notified);

    tokio::select! {
        result = &mut connection => result,
        _ = &mut timeout => {
            if gate.is_seen() {
                (&mut connection).await
            } else {
                tracing::debug!(
                    "execution runtime connection closed before the first request header completed"
                );
                Ok(())
            }
        }
        _ = &mut notified => (&mut connection).await,
    }
}

fn execution_runtime_http_builder() -> HyperServerBuilder<TokioExecutor> {
    let mut builder = HyperServerBuilder::new(TokioExecutor::new());

    // Hyper's HTTP/1 header timer is opt-in when using the custom connection
    // builder. Configure both protocol parsers explicitly: HTTP/1 gets a
    // slow-header deadline and bounded parser buffer; HTTP/2 gets a
    // decompressed header-list limit. These settings apply to metadata only.
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(EXECUTION_RUNTIME_HTTP_HEADER_READ_TIMEOUT)
        .max_buf_size(EXECUTION_RUNTIME_HTTP_HEADER_MAX_BYTES)
        .max_headers(EXECUTION_RUNTIME_HTTP_MAX_HEADERS);
    builder
        .http2()
        .timer(TokioTimer::new())
        .enable_connect_protocol()
        .max_concurrent_streams(EXECUTION_RUNTIME_HTTP2_MAX_CONCURRENT_STREAMS)
        .max_header_list_size(EXECUTION_RUNTIME_HTTP_HEADER_MAX_BYTES as u32);

    builder
}

#[derive(Debug, Clone, Default)]
struct ExecutionRuntimeAppState {
    execution_runtime: DirectSyncExecutionRuntime,
    request_gate: Option<Arc<ConcurrencyGate>>,
    distributed_request_gate: Option<Arc<RuntimeSemaphore>>,
}

impl ExecutionRuntimeAppState {
    fn with_request_concurrency_limit(limit: Option<usize>) -> Self {
        Self {
            execution_runtime: DirectSyncExecutionRuntime::new(),
            request_gate: limit
                .filter(|limit| *limit > 0)
                .map(|limit| Arc::new(ConcurrencyGate::new(REQUEST_GATE_NAME, limit))),
            distributed_request_gate: None,
        }
    }

    fn with_distributed_request_gate(mut self, gate: RuntimeSemaphore) -> Self {
        self.distributed_request_gate = Some(Arc::new(gate));
        self
    }

    fn request_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.request_gate.as_ref().map(|gate| gate.snapshot())
    }

    async fn distributed_request_concurrency_snapshot(
        &self,
    ) -> Result<Option<RuntimeSemaphoreSnapshot>, RuntimeSemaphoreError> {
        match self.distributed_request_gate.as_ref() {
            Some(gate) => gate.snapshot().await.map(Some),
            None => Ok(None),
        }
    }

    async fn metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![service_up_sample(EXECUTION_RUNTIME_COMPONENT)];
        if let Some(snapshot) = self.request_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples(REQUEST_GATE_NAME));
        }
        if let Some(gate) = self.distributed_request_gate.as_ref() {
            match gate.snapshot().await {
                Ok(snapshot) => {
                    samples.extend(snapshot.to_metric_samples(DISTRIBUTED_REQUEST_GATE_NAME));
                }
                Err(_) => samples.push(
                    MetricSample::new(
                        "concurrency_unavailable",
                        "Whether the distributed concurrency gate is currently unavailable.",
                        MetricKind::Gauge,
                        1,
                    )
                    .with_labels(vec![MetricLabel::new(
                        "gate",
                        DISTRIBUTED_REQUEST_GATE_NAME,
                    )]),
                ),
            }
        }
        samples
    }

    async fn try_acquire_request_permit(
        &self,
    ) -> Result<Option<AdmissionPermit>, RequestAdmissionError> {
        let local = self
            .request_gate
            .as_ref()
            .map(|gate| gate.try_acquire())
            .transpose()
            .map_err(RequestAdmissionError::Local)?;
        let distributed = match self.distributed_request_gate.as_ref() {
            Some(gate) => Some(
                gate.try_acquire()
                    .await
                    .map_err(RequestAdmissionError::Distributed)?,
            ),
            None => None,
        };
        Ok(AdmissionPermit::from_parts(local, distributed))
    }
}

pub fn build_execution_runtime_router() -> Router {
    build_execution_runtime_router_with_request_concurrency_limit(None)
}

pub fn build_execution_runtime_router_with_request_concurrency_limit(
    limit: Option<usize>,
) -> Router {
    build_execution_runtime_router_with_request_gates(limit, None)
}

pub fn build_execution_runtime_router_with_request_gates(
    limit: Option<usize>,
    distributed_gate: Option<RuntimeSemaphore>,
) -> Router {
    let state = match distributed_gate {
        Some(gate) => ExecutionRuntimeAppState::with_request_concurrency_limit(limit)
            .with_distributed_request_gate(gate),
        None => ExecutionRuntimeAppState::with_request_concurrency_limit(limit),
    };
    middleware::apply_cf_header_stripping(
        Router::new()
            .route("/health", get(health))
            .route("/metrics", get(metrics))
            .route("/v1/execute/sync", post(execute_sync))
            .route("/v1/execute/stream", post(execute_stream))
            .with_state(state),
    )
}

pub async fn serve_execution_runtime_tcp(
    bind: &str,
    max_in_flight_requests: Option<usize>,
    distributed_request_gate: Option<RuntimeSemaphore>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The execution runtime accepts plans containing upstream credentials and
    // can issue arbitrary provider requests.  It has no network
    // authentication layer, so a TCP listener must remain local-only.
    let bind_addr = validate_execution_runtime_tcp_bind(bind)?;
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let router = build_execution_runtime_router_with_request_gates(
        max_in_flight_requests,
        distributed_request_gate,
    );
    let mut make_service = router.into_make_service();

    loop {
        let (io, _remote_addr) = listener.accept().await?;
        let tower_service = make_service
            .call(())
            .await
            .unwrap_or_else(|error| match error {})
            .map_request(|request: http::Request<Incoming>| request.map(Body::new));
        let first_request_gate = ExecutionRuntimeFirstRequestGate::new();
        let hyper_service = TowerToHyperService::new(ExecutionRuntimeFirstRequestService {
            inner: tower_service,
            gate: first_request_gate.clone(),
        });
        let io = TokioIo::new(io);

        tokio::spawn(async move {
            let builder = execution_runtime_http_builder();
            let result = drive_execution_runtime_connection(
                builder.serve_connection_with_upgrades(io, hyper_service),
                first_request_gate,
                EXECUTION_RUNTIME_HTTP_HEADER_READ_TIMEOUT,
            )
            .await;
            if let Err(error) = result {
                tracing::trace!(error = ?error, "execution runtime TCP connection closed with error");
            }
        });
    }
}

fn validate_execution_runtime_tcp_bind(bind: &str) -> io::Result<SocketAddr> {
    let address = bind.parse::<SocketAddr>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "execution runtime TCP bind must be a literal loopback socket address",
        )
    })?;
    if !address.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "execution runtime TCP bind must use a loopback address",
        ));
    }
    Ok(address)
}

#[cfg(unix)]
pub async fn serve_execution_runtime_unix(
    socket_path: &Path,
    max_in_flight_requests: Option<usize>,
    distributed_request_gate: Option<RuntimeSemaphore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = bind_secure_execution_runtime_socket(socket_path).await?;
    let router = build_execution_runtime_router_with_request_gates(
        max_in_flight_requests,
        distributed_request_gate,
    );
    let mut make_service = router.into_make_service();

    loop {
        let (io, _peer_addr) = listener.accept().await?;
        let tower_service = make_service
            .call(())
            .await
            .unwrap_or_else(|error| match error {})
            .map_request(|request: http::Request<Incoming>| request.map(Body::new));
        let first_request_gate = ExecutionRuntimeFirstRequestGate::new();
        let hyper_service = TowerToHyperService::new(ExecutionRuntimeFirstRequestService {
            inner: tower_service,
            gate: first_request_gate.clone(),
        });
        let io = TokioIo::new(io);

        tokio::spawn(async move {
            let builder = execution_runtime_http_builder();
            let result = drive_execution_runtime_connection(
                builder.serve_connection_with_upgrades(io, hyper_service),
                first_request_gate,
                EXECUTION_RUNTIME_HTTP_HEADER_READ_TIMEOUT,
            )
            .await;
            if let Err(error) = result {
                tracing::trace!(error = ?error, "execution runtime Unix connection closed with error");
            }
        });
    }
}

/// Bind the execution-runtime UDS without exposing an unauthenticated socket
/// to other local users. In particular, do not unlink an arbitrary path before
/// binding: a symlink/regular file could otherwise be replaced during that
/// gap. An occupied path is removed only after it is proven to be a stale
/// current-user socket.
#[cfg(unix)]
async fn bind_secure_execution_runtime_socket(
    requested_path: &Path,
) -> io::Result<tokio::net::UnixListener> {
    let socket_path = prepare_execution_runtime_socket_path(requested_path)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&socket_path) {
        validate_existing_execution_runtime_socket(&metadata)?;
    }

    // Try the requested path first. If it is occupied, only a current-user
    // socket that is demonstrably stale may be removed; every other path
    // fails closed. This removes the old unlink-before-bind TOCTOU window.
    match bind_execution_runtime_listener(&socket_path) {
        Ok(listener) => {
            harden_execution_runtime_socket(&listener, &socket_path)?;
            Ok(listener)
        }
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            remove_stale_execution_runtime_socket(&socket_path).await?;
            let listener = bind_execution_runtime_listener(&socket_path)?;
            harden_execution_runtime_socket(&listener, &socket_path)?;
            Ok(listener)
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn bind_execution_runtime_listener(socket_path: &Path) -> io::Result<tokio::net::UnixListener> {
    // Unix bind derives the socket mode from the process umask. Darwin does
    // not support fchmod on a Unix-socket fd, so make the inode private at
    // creation time instead of briefly publishing a world-accessible socket.
    // Serialize the temporary process-wide umask change with other binds made
    // by this component and restore it before returning.
    static BIND_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _lock = BIND_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| io::Error::other("execution runtime socket bind lock poisoned"))?;
    let previous_umask = unsafe { libc::umask(0o177) };
    let result = tokio::net::UnixListener::bind(socket_path);
    unsafe {
        libc::umask(previous_umask);
    }
    result
}

#[cfg(unix)]
fn prepare_execution_runtime_socket_path(requested_path: &Path) -> io::Result<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let file_name = requested_path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "execution runtime socket path must name a file",
        )
    })?;
    if file_name == OsStr::new(".") || file_name == OsStr::new("..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "execution runtime socket path has an invalid file name",
        ));
    }

    let requested_parent = requested_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Do not let a caller-controlled symlink redirect directory creation (or
    // the eventual socket) into an unrelated tree.  Root-owned compatibility
    // links such as macOS `/tmp -> /private/tmp` and Linux `/var/run -> /run`
    // remain allowed; links owned by an unprivileged user fail closed.
    validate_requested_execution_runtime_parent_components(requested_parent)?;
    let mut directory_builder = std::fs::DirBuilder::new();
    directory_builder.recursive(true).mode(0o700);
    directory_builder.create(requested_parent)?;
    validate_requested_execution_runtime_parent_components(requested_parent)?;

    let parent = std::fs::canonicalize(requested_parent)?;
    validate_execution_runtime_socket_parent(&parent)?;
    let metadata = std::fs::symlink_metadata(&parent)?;
    if metadata.mode() & 0o022 != 0
        && metadata.mode() & 0o1000 == 0
        && metadata.uid() == unsafe { libc::geteuid() }
    {
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))?;
    }

    Ok(parent.join(file_name))
}

#[cfg(unix)]
fn validate_requested_execution_runtime_parent_components(parent: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    let mut prefix = PathBuf::new();
    for component in parent.components() {
        match component {
            Component::Prefix(prefix_component) => prefix.push(prefix_component.as_os_str()),
            Component::RootDir => prefix.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => prefix.push(".."),
            Component::Normal(name) => {
                prefix.push(name);
                let metadata = match std::fs::symlink_metadata(&prefix) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                    Err(error) => return Err(error),
                };
                if metadata.file_type().is_symlink() {
                    if metadata.uid() != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "execution runtime socket parent contains an untrusted symlink",
                        ));
                    }
                } else if !metadata.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "execution runtime socket parent contains a non-directory component",
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn validate_execution_runtime_socket_parent(parent: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let effective_uid = unsafe { libc::geteuid() };
    let mut current = Some(parent);
    let mut is_immediate_parent = true;
    while let Some(path) = current {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "execution runtime socket parent contains an unsafe path component",
            ));
        }
        let mode = metadata.mode();
        if metadata.uid() != effective_uid && metadata.uid() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "execution runtime socket parent has untrusted ownership",
            ));
        }
        if mode & 0o022 != 0 && mode & 0o1000 == 0 && metadata.uid() != effective_uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "execution runtime socket parent must not be writable without sticky protection",
            ));
        }
        if !is_immediate_parent && mode & 0o022 != 0 && mode & 0o1000 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "execution runtime socket ancestor is writable without sticky protection",
            ));
        }
        is_immediate_parent = false;
        current = path.parent();
    }
    Ok(())
}

#[cfg(unix)]
fn harden_execution_runtime_socket(
    listener: &tokio::net::UnixListener,
    socket_path: &Path,
) -> io::Result<()> {
    use std::mem::MaybeUninit;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::os::unix::io::AsRawFd;

    let metadata = std::fs::symlink_metadata(socket_path)?;
    let effective_uid = unsafe { libc::geteuid() };
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(listener.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    let fd_mode = stat.st_mode as libc::mode_t;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
        // A pathname Unix socket and its connected sockfs inode do not have
        // portable pathname identity.  In particular, Linux can report a
        // different inode number (and always reports a different device) for
        // the descriptor than for the dentry.  Validate the descriptor's own
        // type and owner instead; the canonical, owner-checked
        // parent and the path checks above prevent an untrusted user from
        // replacing this private socket.
        || (fd_mode & libc::S_IFMT) != libc::S_IFSOCK
        || stat.st_uid != effective_uid
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "execution runtime socket path changed or has unsafe ownership",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_existing_execution_runtime_socket(metadata: &std::fs::Metadata) -> io::Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid
        || metadata.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "occupied execution runtime socket path is not a current-user socket",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn remove_stale_execution_runtime_socket(socket_path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::time::Duration;

    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    validate_existing_execution_runtime_socket(&metadata)?;
    match tokio::time::timeout(
        Duration::from_millis(100),
        tokio::net::UnixStream::connect(socket_path),
    )
    .await
    {
        Ok(Ok(_stream)) => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "execution runtime socket is already serving requests",
            ));
        }
        Ok(Err(error))
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) => {}
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "could not determine whether execution runtime socket is active",
            ));
        }
    }

    let latest = std::fs::symlink_metadata(socket_path)?;
    validate_existing_execution_runtime_socket(&latest)?;
    if latest.ino() != metadata.ino() || latest.dev() != metadata.dev() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "occupied execution runtime socket path changed during cleanup",
        ));
    }
    std::fs::remove_file(socket_path)
}

#[cfg(not(unix))]
pub async fn serve_execution_runtime_unix(
    _socket_path: &Path,
    _max_in_flight_requests: Option<usize>,
    _distributed_request_gate: Option<RuntimeSemaphore>,
) -> Result<(), Box<dyn std::error::Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix sockets are not supported on this platform",
    )
    .into())
}

async fn health(State(state): State<ExecutionRuntimeAppState>) -> impl IntoResponse {
    let request_concurrency = state.request_concurrency_snapshot().map(|snapshot| {
        json!({
            "limit": snapshot.limit,
            "in_flight": snapshot.in_flight,
            "available_permits": snapshot.available_permits,
            "high_watermark": snapshot.high_watermark,
            "rejected": snapshot.rejected,
        })
    });
    let distributed_request_concurrency = state
        .distributed_request_concurrency_snapshot()
        .await
        .ok()
        .flatten()
        .map(|snapshot| {
            json!({
                "limit": snapshot.limit,
                "in_flight": snapshot.in_flight,
                "available_permits": snapshot.available_permits,
                "high_watermark": snapshot.high_watermark,
                "rejected": snapshot.rejected,
            })
        });
    Json(json!({
        "status": "ok",
        "component": EXECUTION_RUNTIME_COMPONENT,
        "request_concurrency": request_concurrency,
        "distributed_request_concurrency": distributed_request_concurrency,
    }))
}

async fn metrics(State(state): State<ExecutionRuntimeAppState>) -> Response {
    prometheus_response(&state.metric_samples().await)
}

async fn execute_sync(
    State(state): State<ExecutionRuntimeAppState>,
    request: Request,
) -> Result<Response, ExecutionRuntimeAppError> {
    let request_permit = acquire_request_permit(&state).await?;
    let plan = parse_request_json::<ExecutionPlan>(request).await?;
    let result = state
        .execution_runtime
        .execute_sync(&plan)
        .await
        .map_err(|err| ExecutionRuntimeAppError(ExecutionRuntimeServerError::Transport(err)))?;
    Ok(maybe_hold_axum_response_permit(
        Json(result).into_response(),
        request_permit,
    ))
}

async fn execute_stream(
    State(state): State<ExecutionRuntimeAppState>,
    request: Request,
) -> Result<Response, ExecutionRuntimeAppError> {
    let request_permit = acquire_request_permit(&state).await?;
    let plan = parse_request_json::<ExecutionPlan>(request).await?;
    let execution = state
        .execution_runtime
        .execute_stream(&plan)
        .await
        .map_err(|err| ExecutionRuntimeAppError(ExecutionRuntimeServerError::Transport(err)))?;

    let mut response = Response::new(Body::from_stream(build_direct_execution_frame_stream(
        execution,
    )));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/x-ndjson"),
    );
    Ok(maybe_hold_axum_response_permit(response, request_permit))
}

async fn acquire_request_permit(
    state: &ExecutionRuntimeAppState,
) -> Result<Option<AdmissionPermit>, ExecutionRuntimeAppError> {
    match state.try_acquire_request_permit().await {
        Ok(permit) => Ok(permit),
        Err(RequestAdmissionError::Local(ConcurrencyError::Saturated { gate, limit }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Saturated {
            gate,
            limit,
        }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Unavailable {
            gate,
            limit,
            ..
        })) => Err(ExecutionRuntimeAppError(
            ExecutionRuntimeServerError::Overloaded { gate, limit },
        )),
        Err(RequestAdmissionError::Local(ConcurrencyError::Closed { gate })) => Err(
            ExecutionRuntimeAppError(ExecutionRuntimeServerError::RequestRead(format!(
                "execution runtime request concurrency gate {gate} is closed"
            ))),
        ),
        Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::InvalidConfiguration(
            message,
        ))) => Err(ExecutionRuntimeAppError(
            ExecutionRuntimeServerError::RequestRead(message),
        )),
    }
}

#[derive(Debug)]
enum RequestAdmissionError {
    Local(ConcurrencyError),
    Distributed(RuntimeSemaphoreError),
}

async fn parse_request_json<T>(request: Request) -> Result<T, ExecutionRuntimeAppError>
where
    T: serde::de::DeserializeOwned,
{
    let request_body_limit =
        execution_runtime_request_body_limit_bytes(crate::headers::max_request_body_bytes());
    let body = to_bytes(request.into_body(), request_body_limit)
        .await
        .map_err(|err| {
            ExecutionRuntimeAppError(ExecutionRuntimeServerError::RequestRead(err.to_string()))
        })?;
    serde_json::from_slice(&body).map_err(|err| {
        ExecutionRuntimeAppError(ExecutionRuntimeServerError::InvalidRequestJson(err))
    })
}

fn execution_runtime_request_body_limit_bytes(configured_limit: u64) -> usize {
    usize::try_from(configured_limit)
        .unwrap_or(EXECUTION_RUNTIME_REQUEST_BODY_HARD_LIMIT_BYTES)
        .min(EXECUTION_RUNTIME_REQUEST_BODY_HARD_LIMIT_BYTES)
}

fn build_overloaded_response(message: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": {
                "type": "overloaded",
                "message": message,
            }
        })),
    )
        .into_response()
}

#[derive(Debug, Error)]
enum ExecutionRuntimeServerError {
    #[error("failed to read execution runtime request body: {0}")]
    RequestRead(String),
    #[error("execution runtime request body is not valid JSON: {0}")]
    InvalidRequestJson(serde_json::Error),
    #[error("execution runtime overloaded: gate {gate} saturated at {limit}")]
    Overloaded { gate: &'static str, limit: usize },
    #[error(transparent)]
    Transport(#[from] ExecutionRuntimeTransportError),
}

#[derive(Debug)]
struct ExecutionRuntimeAppError(ExecutionRuntimeServerError);

impl IntoResponse for ExecutionRuntimeAppError {
    fn into_response(self) -> Response {
        let status_code = match &self.0 {
            ExecutionRuntimeServerError::RequestRead(_)
            | ExecutionRuntimeServerError::InvalidRequestJson(_) => StatusCode::BAD_REQUEST,
            ExecutionRuntimeServerError::Overloaded { .. } => {
                return build_overloaded_response(&self.0.to_string());
            }
            ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::RequestBodyRequired
                | ExecutionRuntimeTransportError::RequestBodyAmbiguous
                | ExecutionRuntimeTransportError::BodyDecode(_)
                | ExecutionRuntimeTransportError::BodyTooLarge { .. }
                | ExecutionRuntimeTransportError::UnsupportedContentEncoding(_)
                | ExecutionRuntimeTransportError::ProxyUnsupported
                | ExecutionRuntimeTransportError::InvalidMethod(_)
                | ExecutionRuntimeTransportError::InvalidHeaderName(_)
                | ExecutionRuntimeTransportError::InvalidHeaderValue(_)
                | ExecutionRuntimeTransportError::InvalidProxy(_)
                | ExecutionRuntimeTransportError::UnsupportedTransportProfile(_)
                | ExecutionRuntimeTransportError::BodyEncode(_),
            ) => StatusCode::BAD_REQUEST,
            ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::UpstreamHttpStatus { status_code, .. },
            ) => StatusCode::from_u16(*status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::ClientBuild(_)
                | ExecutionRuntimeTransportError::BrowserClientBuild(_)
                | ExecutionRuntimeTransportError::BrowserBody(_)
                | ExecutionRuntimeTransportError::UpstreamRequest(_)
                | ExecutionRuntimeTransportError::UpstreamResponseTooLarge { .. }
                | ExecutionRuntimeTransportError::UpstreamResponseDecode { .. }
                | ExecutionRuntimeTransportError::RelayError(_)
                | ExecutionRuntimeTransportError::InvalidJson(_),
            ) => StatusCode::BAD_GATEWAY,
        };
        let message = match &self.0 {
            ExecutionRuntimeServerError::RequestRead(_)
            | ExecutionRuntimeServerError::InvalidRequestJson(_)
            | ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::RequestBodyRequired
                | ExecutionRuntimeTransportError::RequestBodyAmbiguous
                | ExecutionRuntimeTransportError::BodyDecode(_)
                | ExecutionRuntimeTransportError::BodyTooLarge { .. }
                | ExecutionRuntimeTransportError::UnsupportedContentEncoding(_)
                | ExecutionRuntimeTransportError::ProxyUnsupported
                | ExecutionRuntimeTransportError::InvalidMethod(_)
                | ExecutionRuntimeTransportError::InvalidHeaderName(_)
                | ExecutionRuntimeTransportError::InvalidHeaderValue(_)
                | ExecutionRuntimeTransportError::InvalidProxy(_)
                | ExecutionRuntimeTransportError::UnsupportedTransportProfile(_)
                | ExecutionRuntimeTransportError::BodyEncode(_),
            ) => "Invalid execution runtime request".to_string(),
            ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::UpstreamHttpStatus { status_code, .. },
            ) => format!("Upstream request returned HTTP {status_code}"),
            ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::ClientBuild(_)
                | ExecutionRuntimeTransportError::BrowserClientBuild(_)
                | ExecutionRuntimeTransportError::BrowserBody(_)
                | ExecutionRuntimeTransportError::UpstreamRequest(_)
                | ExecutionRuntimeTransportError::UpstreamResponseTooLarge { .. }
                | ExecutionRuntimeTransportError::UpstreamResponseDecode { .. }
                | ExecutionRuntimeTransportError::RelayError(_)
                | ExecutionRuntimeTransportError::InvalidJson(_),
            ) => "Upstream request failed".to_string(),
            ExecutionRuntimeServerError::Overloaded { .. } => unreachable!(),
        };

        (
            status_code,
            Json(json!({
                "error": message,
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_execution_runtime_router_with_request_concurrency_limit,
        build_execution_runtime_router_with_request_gates,
        execution_runtime_request_body_limit_bytes, validate_execution_runtime_tcp_bind,
        ExecutionRuntimeAppError, ExecutionRuntimeServerError, DISTRIBUTED_REQUEST_GATE_NAME,
    };
    use aether_contracts::{
        ExecutionPlan, ExecutionTimeouts, RequestBody, StreamFrame, StreamFrameType,
    };
    use aether_runtime_state::{
        MemoryRuntimeStateConfig, RuntimeSemaphore, RuntimeSemaphoreConfig, RuntimeState,
    };
    use axum::body::{Body, Bytes};
    use axum::response::{IntoResponse, Response};
    use axum::routing::any;
    use axum::{extract::Request, Router};
    use http::StatusCode;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming as HyperIncoming;
    use hyper::{Request as HyperRequest, Response as HyperResponse};
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use std::convert::Infallible;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    #[cfg(unix)]
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::service_fn;

    use crate::execution_runtime::ExecutionRuntimeTransportError;

    fn distributed_gate(gate: &'static str, limit: usize) -> RuntimeSemaphore {
        RuntimeState::memory(MemoryRuntimeStateConfig::default())
            .semaphore(gate, limit, RuntimeSemaphoreConfig::default())
            .expect("distributed semaphore")
    }

    async fn start_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });
        (format!("http://{addr}"), handle)
    }

    #[test]
    fn execution_runtime_tcp_bind_accepts_only_literal_loopback_addresses() {
        for bind in ["127.0.0.1:0", "127.42.17.9:5219", "[::1]:0"] {
            let address = validate_execution_runtime_tcp_bind(bind)
                .unwrap_or_else(|error| panic!("{bind} should be accepted: {error}"));
            assert!(address.ip().is_loopback());
        }
    }

    #[test]
    fn execution_runtime_tcp_bind_rejects_wildcard_non_loopback_and_unparseable_addresses() {
        for bind in [
            "0.0.0.0:5219",
            "[::]:5219",
            "10.0.0.1:5219",
            "192.168.1.10:5219",
            "localhost:5219",
            "not-a-socket-address",
            "127.0.0.1",
        ] {
            assert!(
                validate_execution_runtime_tcp_bind(bind).is_err(),
                "{bind} must be rejected"
            );
        }
    }

    #[test]
    fn execution_runtime_parser_defaults_preserve_high_http2_concurrency() {
        assert_eq!(
            super::EXECUTION_RUNTIME_HTTP2_MAX_CONCURRENT_STREAMS,
            16_384
        );
        assert_eq!(
            super::EXECUTION_RUNTIME_HTTP_HEADER_READ_TIMEOUT,
            std::time::Duration::from_secs(30)
        );
        assert_eq!(super::EXECUTION_RUNTIME_HTTP_HEADER_MAX_BYTES, 64 * 1024);
        assert_eq!(super::EXECUTION_RUNTIME_HTTP_MAX_HEADERS, 256);
    }

    #[test]
    fn execution_runtime_request_body_limit_never_accepts_unbounded_sentinel() {
        assert_eq!(
            execution_runtime_request_body_limit_bytes(u64::MAX),
            super::EXECUTION_RUNTIME_REQUEST_BODY_HARD_LIMIT_BYTES
        );
        assert_eq!(
            execution_runtime_request_body_limit_bytes(512 * 1024 * 1024),
            super::EXECUTION_RUNTIME_REQUEST_BODY_HARD_LIMIT_BYTES
        );
        assert_eq!(execution_runtime_request_body_limit_bytes(1024), 1024);
    }

    #[tokio::test]
    async fn execution_runtime_first_request_deadline_covers_partial_protocol_input() {
        let prefixes: &[&[u8]] = &[
            b"G",
            b"GET /health HTTP/1.1\r\nHost: localhost\r\n",
            b"PRI * HTTP/2.0\r\n\r\nSM\r\n",
        ];

        for prefix in prefixes {
            let (mut client, server) = tokio::io::duplex(16 * 1024);
            client
                .write_all(prefix)
                .await
                .expect("fixture prefix should be writable");

            let gate = super::ExecutionRuntimeFirstRequestGate::new();
            let service = service_fn(|_request: HyperRequest<HyperIncoming>| async {
                Ok::<_, Infallible>(HyperResponse::new(Full::new(bytes::Bytes::from_static(
                    b"ok",
                ))))
            });
            let builder = super::execution_runtime_http_builder();
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                super::drive_execution_runtime_connection(
                    builder.serve_connection_with_upgrades(
                        TokioIo::new(server),
                        super::TowerToHyperService::new(
                            super::ExecutionRuntimeFirstRequestService {
                                inner: service,
                                gate: gate.clone(),
                            },
                        ),
                    ),
                    gate,
                    std::time::Duration::from_millis(5),
                ),
            )
            .await
            .expect("partial protocol input should hit the first-request deadline");
            assert!(result.is_ok());

            let mut byte = [0u8; 1];
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(1), client.read(&mut byte))
                    .await
                    .expect("timed-out connection should close its peer");
            assert!(matches!(read, Ok(0) | Err(_)));
        }
    }

    #[tokio::test]
    async fn execution_runtime_first_request_deadline_does_not_cut_off_body_streaming() {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let gate = super::ExecutionRuntimeFirstRequestGate::new();
        let (headers_seen_tx, mut headers_seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let service = service_fn(move |request: HyperRequest<HyperIncoming>| {
            let _ = headers_seen_tx.send(());
            async move {
                let body = request
                    .into_body()
                    .collect()
                    .await
                    .expect("test body should decode")
                    .to_bytes();
                Ok::<_, Infallible>(HyperResponse::new(Full::new(body)))
            }
        });
        let builder = super::execution_runtime_http_builder();
        let server_task = tokio::spawn(async move {
            super::drive_execution_runtime_connection(
                builder.serve_connection_with_upgrades(
                    TokioIo::new(server),
                    super::TowerToHyperService::new(super::ExecutionRuntimeFirstRequestService {
                        inner: service,
                        gate: gate.clone(),
                    }),
                ),
                gate,
                std::time::Duration::from_millis(20),
            )
            .await
        });

        let large_body = vec![b'x'; 128 * 1024];
        let request_headers = format!(
            "POST /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            large_body.len()
        );
        client
            .write_all(request_headers.as_bytes())
            .await
            .expect("request headers should be writable");
        tokio::time::timeout(std::time::Duration::from_secs(1), headers_seen_rx.recv())
            .await
            .expect("request headers should reach the service")
            .expect("service notification should remain available");

        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        // The parser's 64 KiB metadata buffer must not become a body-size or
        // body-throughput limit. Send a body larger than that buffer after the
        // first-request gate has opened and verify it is echoed intact.
        client
            .write_all(&large_body)
            .await
            .expect("body should remain writable after the header deadline");
        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.read_to_end(&mut response),
        )
        .await
        .expect("streaming response should complete")
        .expect("response should be readable");
        let result = server_task
            .await
            .expect("server connection task should join");
        assert!(result.is_ok());
        assert!(response
            .windows(large_body.len())
            .any(|window| window == large_body.as_slice()));
    }

    fn stream_plan(url: String) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".into(),
            candidate_id: Some("cand-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "GET".into(),
            url,
            headers: std::collections::BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-4.1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(30_000),
                ..ExecutionTimeouts::default()
            }),
        }
    }

    #[cfg(unix)]
    fn test_socket_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("ar{}", uuid::Uuid::new_v4().simple()))
            .join("n")
            .join("s.sock")
    }

    #[cfg(unix)]
    fn remove_test_socket_path(socket_path: &std::path::Path) {
        if let Some(parent) = socket_path.parent() {
            if let Some(root) = parent.parent() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_runtime_unix_socket_and_new_parent_are_private() {
        let socket_path = test_socket_path();
        let listener = super::bind_secure_execution_runtime_socket(&socket_path)
            .await
            .expect("socket should bind");

        let socket_metadata = fs::symlink_metadata(&socket_path).expect("socket should exist");
        assert!(socket_metadata.file_type().is_socket());
        assert_eq!(socket_metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(socket_metadata.mode() & 0o777, 0o600);

        let parent = socket_path.parent().expect("socket should have a parent");
        let parent_mode = fs::symlink_metadata(parent)
            .expect("parent should exist")
            .mode()
            & 0o777;
        assert_eq!(parent_mode, 0o700);

        drop(listener);
        remove_test_socket_path(&socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_runtime_unix_socket_rejects_regular_file_path() {
        let socket_path = test_socket_path();
        let parent = socket_path.parent().expect("socket should have a parent");
        fs::create_dir_all(parent).expect("parent should be created");
        fs::write(&socket_path, b"do not replace this file").expect("file should be created");

        let error = super::bind_secure_execution_runtime_socket(&socket_path)
            .await
            .expect_err("regular file must not be replaced");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            fs::read(&socket_path).expect("file should remain"),
            b"do not replace this file"
        );

        remove_test_socket_path(&socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_runtime_unix_socket_rejects_symlink_path() {
        use std::os::unix::fs::symlink;

        let socket_path = test_socket_path();
        let parent = socket_path.parent().expect("socket should have a parent");
        fs::create_dir_all(parent).expect("parent should be created");
        let target = parent.join("target");
        fs::write(&target, b"target must not be followed").expect("target should be created");
        symlink(&target, &socket_path).expect("symlink should be created");

        let error = super::bind_secure_execution_runtime_socket(&socket_path)
            .await
            .expect_err("symlink must not be replaced or followed");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            fs::read(&target).expect("target should remain"),
            b"target must not be followed"
        );

        remove_test_socket_path(&socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_runtime_unix_socket_rejects_untrusted_parent_symlink() {
        use std::os::unix::fs::symlink;

        let socket_path = test_socket_path();
        let parent_root = socket_path
            .parent()
            .and_then(std::path::Path::parent)
            .expect("socket should have a test root");
        fs::create_dir_all(parent_root).expect("test root should be created");
        let target = parent_root.join("target");
        fs::create_dir_all(&target).expect("symlink target should be created");
        let link = parent_root.join("link");
        symlink(&target, &link).expect("parent symlink should be created");
        let linked_socket_path = link.join("runtime.sock");

        let error = super::bind_secure_execution_runtime_socket(&linked_socket_path)
            .await
            .expect_err("untrusted parent symlink must not be followed");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!target.join("runtime.sock").exists());

        remove_test_socket_path(&socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_runtime_unix_socket_does_not_replace_active_listener() {
        let socket_path = test_socket_path();
        let first_listener = super::bind_secure_execution_runtime_socket(&socket_path)
            .await
            .expect("first socket should bind");

        let error = super::bind_secure_execution_runtime_socket(&socket_path)
            .await
            .expect_err("active socket must not be replaced");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        assert!(fs::symlink_metadata(&socket_path)
            .expect("active socket should remain")
            .file_type()
            .is_socket());

        drop(first_listener);
        remove_test_socket_path(&socket_path);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execution_runtime_unix_socket_rebinds_stale_current_user_socket() {
        let socket_path = test_socket_path();
        let parent = socket_path.parent().expect("socket should have a parent");
        fs::create_dir_all(parent).expect("parent should be created");
        let stale_listener = StdUnixListener::bind(&socket_path).expect("stale socket should bind");
        drop(stale_listener);

        let listener = super::bind_secure_execution_runtime_socket(&socket_path)
            .await
            .expect("stale socket should be replaced");
        let rebound_metadata =
            fs::symlink_metadata(&socket_path).expect("rebound socket should exist");
        assert!(rebound_metadata.file_type().is_socket());
        assert_eq!(rebound_metadata.mode() & 0o777, 0o600);

        drop(listener);
        remove_test_socket_path(&socket_path);
    }

    #[tokio::test]
    async fn execution_runtime_stream_endpoint_carries_non_stream_upstream_plan() {
        let upstream = Router::new().route(
            "/sync-json",
            any(|| async { axum::Json(serde_json::json!({"ok": true})) }),
        );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let runtime = build_execution_runtime_router_with_request_concurrency_limit(None);
        let (runtime_url, runtime_handle) = start_server(runtime).await;
        let mut plan = stream_plan(format!("{upstream_url}/sync-json"));
        plan.stream = false;

        let response = reqwest::Client::new()
            .post(format!("{runtime_url}/v1/execute/stream"))
            .json(&plan)
            .send()
            .await
            .expect("execution request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.expect("frame body should read");
        let frame_types = body
            .lines()
            .map(|line| {
                serde_json::from_str::<StreamFrame>(line)
                    .expect("execution runtime frame should decode")
                    .frame_type
            })
            .collect::<Vec<_>>();
        assert!(frame_types.contains(&StreamFrameType::Headers));
        assert!(frame_types.contains(&StreamFrameType::Data));
        assert!(frame_types.contains(&StreamFrameType::Eof));

        runtime_handle.abort();
        upstream_handle.abort();
    }

    #[tokio::test]
    async fn execution_runtime_rejects_second_in_flight_stream_request_with_overload() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_clone = Arc::clone(&upstream_hits);
        let upstream = Router::new().route(
            "/slow",
            any(move |_request: Request| {
                let upstream_hits = Arc::clone(&upstream_hits_clone);
                async move {
                    upstream_hits.fetch_add(1, Ordering::SeqCst);
                    let stream = async_stream::stream! {
                        yield Ok::<_, Infallible>(Bytes::from_static(b"chunk-1"));
                        futures_util::future::pending::<()>().await;
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from_stream(stream))
                        .expect("response should build")
                }
            }),
        );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let runtime = build_execution_runtime_router_with_request_concurrency_limit(Some(1));
        let (runtime_url, runtime_handle) = start_server(runtime).await;

        let client = reqwest::Client::new();
        let first_response = client
            .post(format!("{runtime_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("first request should succeed");

        for _ in 0..50 {
            if upstream_hits.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        let second_response = client
            .post(format!("{runtime_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("second request should complete");

        assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            second_response
                .json::<serde_json::Value>()
                .await
                .expect("json body should decode")["error"]["type"],
            "overloaded"
        );
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        drop(first_response);
        runtime_handle.abort();
        upstream_handle.abort();
    }

    #[tokio::test]
    async fn execution_runtime_rejects_second_in_flight_stream_request_with_distributed_overload() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_clone = Arc::clone(&upstream_hits);
        let upstream = Router::new().route(
            "/slow",
            any(move |_request: Request| {
                let upstream_hits = Arc::clone(&upstream_hits_clone);
                async move {
                    upstream_hits.fetch_add(1, Ordering::SeqCst);
                    let stream = async_stream::stream! {
                        yield Ok::<_, Infallible>(Bytes::from_static(b"chunk-1"));
                        futures_util::future::pending::<()>().await;
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from_stream(stream))
                        .expect("response should build")
                }
            }),
        );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let distributed_gate = distributed_gate(DISTRIBUTED_REQUEST_GATE_NAME, 1);
        let runtime_a =
            build_execution_runtime_router_with_request_gates(None, Some(distributed_gate.clone()));
        let runtime_b =
            build_execution_runtime_router_with_request_gates(None, Some(distributed_gate));
        let (runtime_a_url, runtime_a_handle) = start_server(runtime_a).await;
        let (runtime_b_url, runtime_b_handle) = start_server(runtime_b).await;

        let client = reqwest::Client::new();
        let first_response = client
            .post(format!("{runtime_a_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("first request should succeed");

        for _ in 0..50 {
            if upstream_hits.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        let second_response = client
            .post(format!("{runtime_b_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("second request should complete");

        assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            second_response
                .json::<serde_json::Value>()
                .await
                .expect("json body should decode")["error"]["type"],
            "overloaded"
        );
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        drop(first_response);
        runtime_a_handle.abort();
        runtime_b_handle.abort();
        upstream_handle.abort();
    }

    #[tokio::test]
    async fn execution_runtime_exposes_request_concurrency_metrics() {
        let runtime = build_execution_runtime_router_with_request_gates(
            Some(4),
            Some(distributed_gate(DISTRIBUTED_REQUEST_GATE_NAME, 6)),
        );
        let (runtime_url, runtime_handle) = start_server(runtime).await;

        let response = reqwest::Client::new()
            .get(format!("{runtime_url}/metrics"))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let body = response.text().await.expect("body should read");
        assert!(body.contains("service_up{service=\"aether-gateway-execution-runtime\"} 1"));
        assert!(
            body.contains("concurrency_available_permits{gate=\"execution_runtime_requests\"} 4")
        );
        assert!(body.contains(
            "concurrency_available_permits{gate=\"execution_runtime_requests_distributed\"} 6"
        ));

        runtime_handle.abort();
    }

    #[tokio::test]
    async fn execution_runtime_transport_errors_do_not_expose_internal_details() {
        let secret = "Bearer upstream-secret https://user:password@example.test?q=token";
        let response = ExecutionRuntimeAppError(ExecutionRuntimeServerError::Transport(
            ExecutionRuntimeTransportError::UpstreamRequest(secret.to_string()),
        ))
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should read");
        let body = String::from_utf8(body.to_vec()).expect("error body should be utf8");
        assert_eq!(body, r#"{"error":"Upstream request failed"}"#);
        assert!(!body.contains(secret));
        assert!(!body.contains("upstream-secret"));
    }

    #[tokio::test]
    async fn execution_runtime_upstream_status_keeps_only_status_diagnostics() {
        let response = ExecutionRuntimeAppError(ExecutionRuntimeServerError::Transport(
            ExecutionRuntimeTransportError::UpstreamHttpStatus {
                status_code: 429,
                message: "authorization=Bearer upstream-secret".to_string(),
            },
        ))
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should read");
        let body = String::from_utf8(body.to_vec()).expect("error body should be utf8");
        assert_eq!(body, r#"{"error":"Upstream request returned HTTP 429"}"#);
        assert!(!body.contains("upstream-secret"));
    }
}
