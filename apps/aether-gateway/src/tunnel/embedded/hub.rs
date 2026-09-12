use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_runtime::{
    bounded_queue, BoundedQueueSender, MetricKind, MetricLabel, MetricSample, QueueSendError,
    QueueSnapshot,
};
use axum::extract::ws::Message;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tokio::sync::{watch, Notify};
use tracing::{debug, info, warn};

pub use super::body::LocalBodyEvent;
use super::body::{BodyReceiver, ResponseBuffer};
use super::control_plane::ControlPlaneClient;
use super::protocol;

const MAX_REQUEST_BODY_FRAME_SIZE: usize = 32 * 1024;
const MAX_TUNNEL_CONTROL_PAYLOAD_SIZE: usize = 256 * 1024;
const SOFT_AVOID_QUEUE_PRESSURE_PERCENT: u64 = 50;
const SOFT_AVOID_STREAM_PRESSURE_PERCENT: u64 = 85;
const OUTBOUND_BACKPRESSURE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_STREAM_INITIAL_WINDOW_BYTES: u32 = 4 * 1024 * 1024;
const DEFAULT_DRAIN_DEADLINE_MS: u64 = 30_000;
const DEFAULT_NODE_STATUS_QUEUE_CAPACITY: usize = 1_024;
const CONNECTION_WARMUP: Duration = Duration::from_secs(1);

#[cfg(test)]
#[path = "flow_control_tests.rs"]
mod flow_control_tests;

static STREAM_INITIAL_WINDOW_BYTES: LazyLock<u32> = LazyLock::new(|| {
    std::env::var("AETHER_TUNNEL_STREAM_INITIAL_WINDOW_BYTES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_STREAM_INITIAL_WINDOW_BYTES)
});

static DRAIN_DEADLINE_MS: LazyLock<u64> = LazyLock::new(|| {
    std::env::var("AETHER_TUNNEL_DRAIN_DEADLINE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_DRAIN_DEADLINE_MS)
});

static NODE_STATUS_QUEUE_CAPACITY: LazyLock<usize> = LazyLock::new(|| {
    std::env::var("AETHER_TUNNEL_NODE_STATUS_QUEUE_CAPACITY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_NODE_STATUS_QUEUE_CAPACITY)
});

pub(super) fn local_settings() -> protocol::SettingsPayload {
    protocol::SettingsPayload {
        initial_stream_window_bytes: (*STREAM_INITIAL_WINDOW_BYTES)
            .min(aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES as u32),
        min_window_update_bytes: STREAM_INITIAL_WINDOW_BYTES
            .saturating_div(4)
            .clamp(1, 1024 * 1024),
        drain_deadline_ms: *DRAIN_DEADLINE_MS,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStatus {
    Queued,
    Closed,
    Congested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnHealthState {
    Healthy,
    Warmup,
    Draining,
    Degraded,
    Closing,
}

impl ConnHealthState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Warmup => "warmup",
            Self::Draining => "draining",
            Self::Degraded => "degraded",
            Self::Closing => "closing",
        }
    }
}

#[derive(Debug)]
struct StreamFlowWindow {
    available: Mutex<u64>,
    notify: Notify,
    closed: AtomicBool,
}

impl StreamFlowWindow {
    fn new(initial: u32) -> Self {
        Self {
            available: Mutex::new(u64::from(initial)),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    async fn acquire(&self, bytes: usize, timeout: Duration) -> Result<Duration, ()> {
        if bytes == 0 {
            return Ok(Duration::ZERO);
        }

        let requested = bytes as u64;
        let started_at = Instant::now();
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closed.load(Ordering::Acquire) {
                return Err(());
            }
            {
                let mut available = self.available.lock();
                if *available >= requested {
                    *available -= requested;
                    return Ok(started_at.elapsed());
                }
            }

            let Some(remaining) = timeout.checked_sub(started_at.elapsed()) else {
                return Err(());
            };
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return Err(());
            }
        }
    }

    fn add(&self, delta: u32) {
        if delta == 0 {
            return;
        }
        let mut available = self.available.lock();
        *available = available.saturating_add(u64::from(delta));
        drop(available);
        self.notify.notify_waiters();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConnConfig {
    pub ping_interval: Duration,
    pub idle_timeout: Duration,
    pub outbound_queue_capacity: usize,
}

pub struct BoundedOutbound {
    tx: BoundedQueueSender<Message>,
    close_tx: watch::Sender<bool>,
    closing: AtomicBool,
}

impl BoundedOutbound {
    pub fn new(tx: BoundedQueueSender<Message>, close_tx: watch::Sender<bool>) -> Self {
        Self {
            tx,
            close_tx,
            closing: AtomicBool::new(false),
        }
    }

    pub fn send(&self, msg: Message) -> SendStatus {
        if self.is_closing() {
            return SendStatus::Closed;
        }

        match self.tx.try_send(msg) {
            Ok(()) => SendStatus::Queued,
            Err(QueueSendError::Closed(_)) => {
                self.mark_closing();
                SendStatus::Closed
            }
            Err(QueueSendError::Full(_)) => SendStatus::Congested,
        }
    }

    pub async fn send_wait(&self, msg: Message, timeout: Duration) -> (SendStatus, Duration) {
        if self.is_closing() {
            return (SendStatus::Closed, Duration::ZERO);
        }

        let started_at = std::time::Instant::now();
        match tokio::time::timeout(timeout, self.tx.send(msg)).await {
            Ok(Ok(())) => (SendStatus::Queued, started_at.elapsed()),
            Ok(Err(QueueSendError::Closed(_))) => {
                self.mark_closing();
                (SendStatus::Closed, started_at.elapsed())
            }
            Ok(Err(QueueSendError::Full(_))) => (SendStatus::Congested, started_at.elapsed()),
            Err(_) => (SendStatus::Congested, started_at.elapsed()),
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    pub fn mark_closing(&self) -> bool {
        if self.closing.swap(true, Ordering::AcqRel) {
            return false;
        }
        let _ = self.close_tx.send(true);
        true
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        self.tx.snapshot()
    }

    pub(super) fn subscribe_close(&self) -> watch::Receiver<bool> {
        self.close_tx.subscribe()
    }
}

pub struct ProxyConn {
    pub id: u64,
    pub node_id: String,
    pub node_name: String,
    pub node_generation: String,
    pub authenticated_key: Option<String>,
    pub(crate) management_token_credential: Option<ProxyManagementTokenCredential>,
    pub outbound: BoundedOutbound,
    next_stream_id: AtomicU32,
    pub stream_count: AtomicUsize,
    pub max_streams: usize,
    pub protocol_version: AtomicU8,
    draining: AtomicBool,
    connected_at: Instant,
    remote_health_score: AtomicU8,
    congested_total: AtomicU64,
    backpressure_total: AtomicU64,
    flow_window_blocked_ms: AtomicU64,
    write_latency_last_us: AtomicU64,
    write_latency_ewma_us: AtomicU64,
    settings: Mutex<protocol::SettingsPayload>,
}

impl ProxyConn {
    pub fn new(
        id: u64,
        node_id: String,
        node_name: String,
        tx: BoundedQueueSender<Message>,
        close_tx: watch::Sender<bool>,
        max_streams: usize,
        protocol_version: u8,
    ) -> Self {
        Self {
            settings: Mutex::new(local_settings()),
            id,
            node_id,
            node_name,
            node_generation: String::new(),
            authenticated_key: None,
            management_token_credential: None,
            outbound: BoundedOutbound::new(tx, close_tx),
            next_stream_id: AtomicU32::new(2),
            stream_count: AtomicUsize::new(0),
            max_streams,
            protocol_version: AtomicU8::new(protocol_version.max(1)),
            draining: AtomicBool::new(false),
            connected_at: Instant::now(),
            remote_health_score: AtomicU8::new(100),
            congested_total: AtomicU64::new(0),
            backpressure_total: AtomicU64::new(0),
            flow_window_blocked_ms: AtomicU64::new(0),
            write_latency_last_us: AtomicU64::new(0),
            write_latency_ewma_us: AtomicU64::new(0),
        }
    }

    pub fn with_authenticated_key(mut self, authenticated_key: String) -> Self {
        self.authenticated_key = Some(authenticated_key);
        self
    }

    pub(super) fn with_settings(mut self, settings: protocol::SettingsPayload) -> Self {
        *self.settings.get_mut() = settings;
        self
    }

    pub fn with_tunnel_generation(mut self, tunnel_generation: String) -> Self {
        self.node_generation = tunnel_generation;
        self
    }

    pub(crate) fn with_management_token_credential(
        mut self,
        credential: ProxyManagementTokenCredential,
    ) -> Self {
        self.management_token_credential = Some(credential);
        self
    }

    pub(crate) fn credential_binding(&self) -> Option<ProxyCredentialBinding> {
        match (
            self.authenticated_key.as_deref(),
            self.management_token_credential.as_ref(),
        ) {
            (Some(key), None) => Some(ProxyCredentialBinding::Psk(key.to_string())),
            (None, Some(credential)) => {
                Some(ProxyCredentialBinding::ManagementToken(credential.clone()))
            }
            _ => None,
        }
    }

    pub fn record_write_latency(&self, elapsed: std::time::Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.write_latency_last_us.store(micros, Ordering::Relaxed);
        let current = self.write_latency_ewma_us.load(Ordering::Relaxed);
        let next = if current == 0 {
            micros
        } else {
            let delta = micros as i128 - current as i128;
            (current as i128 + (delta / 8)).max(1) as u64
        };
        self.write_latency_ewma_us
            .store(next.max(1), Ordering::Relaxed);
    }

    pub fn alloc_stream_id(&self) -> Option<u32> {
        let mut current = self.stream_count.load(Ordering::Relaxed);
        loop {
            if current >= self.max_streams || !self.is_available() {
                return None;
            }
            match self.stream_count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }

        let sid = loop {
            let current_sid = self.next_stream_id.load(Ordering::Relaxed);
            let next_sid = if current_sid >= 0xFFFF_FFFE {
                2
            } else {
                current_sid + 2
            };
            if self
                .next_stream_id
                .compare_exchange_weak(current_sid, next_sid, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                break current_sid;
            }
        };

        Some(sid)
    }

    pub fn release_stream(&self) {
        let mut current = self.stream_count.load(Ordering::Relaxed);
        while current > 0 {
            match self.stream_count.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    pub fn is_available(&self) -> bool {
        !self.outbound.is_closing() && !self.is_draining()
    }

    pub fn request_close(&self) {
        self.outbound.mark_closing();
    }

    pub fn mark_draining(&self) -> bool {
        !self.draining.swap(true, Ordering::AcqRel)
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    pub fn send(&self, msg: Message) -> SendStatus {
        let was_closing = self.outbound.is_closing();
        let status = self.outbound.send(msg);
        if status == SendStatus::Congested && !was_closing {
            self.congested_total.fetch_add(1, Ordering::Relaxed);
            warn!(
                conn_id = self.id,
                node_id = %self.node_id,
                node_name = %self.node_name,
                queued_streams = self.stream_count.load(Ordering::Relaxed),
                "proxy outbound queue full, dropping frame without closing connection"
            );
        }
        status
    }

    pub async fn send_wait(&self, msg: Message, timeout: Duration) -> SendStatus {
        let before = self.outbound.snapshot();
        let (status, waited) = self.outbound.send_wait(msg, timeout).await;
        if status == SendStatus::Congested {
            self.congested_total.fetch_add(1, Ordering::Relaxed);
        }
        if before.depth >= before.capacity || waited > Duration::from_millis(1) {
            self.backpressure_total.fetch_add(1, Ordering::Relaxed);
            self.flow_window_blocked_ms.fetch_add(
                u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            debug!(
                conn_id = self.id,
                node_id = %self.node_id,
                queue_depth_before = before.depth,
                queue_capacity = before.capacity,
                waited_ms = waited.as_millis() as u64,
                send_status = ?status,
                "proxy outbound body frame waited on tunnel backpressure"
            );
        }
        status
    }

    pub fn protocol_version(&self) -> u8 {
        self.protocol_version.load(Ordering::Relaxed)
    }

    pub fn update_protocol_version(&self, protocol_version: u8) {
        let negotiated =
            protocol_version.clamp(1, aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION);
        self.protocol_version.store(negotiated, Ordering::Relaxed);
    }

    pub fn update_remote_health_score(&self, health_score: u8) {
        self.remote_health_score
            .store(health_score.min(100), Ordering::Relaxed);
    }

    fn snapshot(&self) -> ProxyConnSnapshot {
        let outbound = self.outbound.snapshot();
        let stream_count = self.stream_count.load(Ordering::Relaxed);
        let queue_pressure_percent = percent_u64(outbound.depth, outbound.capacity);
        let stream_pressure_percent = percent_u64(stream_count, self.max_streams);
        let soft_avoid = queue_pressure_percent >= SOFT_AVOID_QUEUE_PRESSURE_PERCENT
            || stream_pressure_percent >= SOFT_AVOID_STREAM_PRESSURE_PERCENT;
        let local_health_score =
            connection_health_score(queue_pressure_percent, stream_pressure_percent);
        let remote_health_score = u64::from(self.remote_health_score.load(Ordering::Relaxed));
        let health_score = local_health_score.min(remote_health_score);
        let state = if self.outbound.is_closing() {
            ConnHealthState::Closing
        } else if self.is_draining() {
            ConnHealthState::Draining
        } else if self.connected_at.elapsed() < CONNECTION_WARMUP {
            ConnHealthState::Warmup
        } else if soft_avoid || health_score < 60 {
            ConnHealthState::Degraded
        } else {
            ConnHealthState::Healthy
        };
        ProxyConnSnapshot {
            conn_id: self.id,
            available: self.is_available(),
            closing: self.outbound.is_closing(),
            draining: self.is_draining(),
            state,
            stream_count,
            max_streams: self.max_streams,
            protocol_version: self.protocol_version.load(Ordering::Relaxed),
            stream_pressure_percent,
            outbound,
            queue_pressure_percent,
            soft_avoid,
            health_score,
            congested_total: self.congested_total.load(Ordering::Relaxed),
            backpressure_total: self.backpressure_total.load(Ordering::Relaxed),
            flow_window_blocked_ms: self.flow_window_blocked_ms.load(Ordering::Relaxed),
            write_latency_last_us: self.write_latency_last_us.load(Ordering::Relaxed),
            write_latency_ewma_us: self.write_latency_ewma_us.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProxyManagementTokenCredential {
    pub(crate) verified_token_hash: crate::management_token_auth::VerifiedManagementTokenHash,
    pub(crate) token_id: String,
    pub(crate) user_id: String,
    pub(crate) remote_ip: IpAddr,
}

#[derive(Clone)]
pub(crate) enum ProxyCredentialBinding {
    Psk(String),
    ManagementToken(ProxyManagementTokenCredential),
}

impl std::fmt::Debug for ProxyCredentialBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Psk(_) => formatter.write_str("ProxyCredentialBinding::Psk([REDACTED])"),
            Self::ManagementToken(credential) => formatter
                .debug_tuple("ProxyCredentialBinding::ManagementToken")
                .field(credential)
                .finish(),
        }
    }
}

impl std::fmt::Debug for ProxyManagementTokenCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProxyManagementTokenCredential")
            .field("verified_token_hash", &"[REDACTED]")
            .field("token_id", &self.token_id)
            .field("user_id", &self.user_id)
            .field("remote_ip", &self.remote_ip)
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
struct ProxyConnSnapshot {
    conn_id: u64,
    available: bool,
    closing: bool,
    draining: bool,
    state: ConnHealthState,
    stream_count: usize,
    max_streams: usize,
    protocol_version: u8,
    stream_pressure_percent: u64,
    outbound: QueueSnapshot,
    queue_pressure_percent: u64,
    soft_avoid: bool,
    health_score: u64,
    congested_total: u64,
    backpressure_total: u64,
    flow_window_blocked_ms: u64,
    write_latency_last_us: u64,
    write_latency_ewma_us: u64,
}

#[derive(Clone)]
struct ProxyConnCandidate {
    conn: Arc<ProxyConn>,
    snapshot: ProxyConnSnapshot,
}

impl ProxyConnCandidate {
    fn rank_key(&self) -> (u8, u64, u64, u64, usize, usize, u64) {
        (
            u8::from(self.snapshot.soft_avoid),
            self.snapshot.queue_pressure_percent,
            self.snapshot.stream_pressure_percent,
            self.snapshot.write_latency_ewma_us,
            self.snapshot.outbound.depth,
            self.snapshot.stream_count,
            self.snapshot.conn_id,
        )
    }
}

#[derive(Debug, Clone)]
pub struct LocalResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct LocalWaitState {
    response: Option<LocalResponseHead>,
    error: Option<String>,
}

pub struct LocalStream {
    pub id: u64,
    tunnel_generation: String,
    proxy_conn_id: u64,
    proxy_stream_id: u32,
    request_window: StreamFlowWindow,
    response_consumed_since_update: Mutex<u64>,
    min_window_update_bytes: u32,
    response_connection: Mutex<Option<std::sync::Weak<ProxyConn>>>,
    wait_state: Mutex<LocalWaitState>,
    headers_notify: Notify,
    body: Arc<ResponseBuffer>,
    terminal: AtomicBool,
}

impl LocalStream {
    fn new(
        id: u64,
        tunnel_generation: String,
        proxy_conn_id: u64,
        proxy_stream_id: u32,
        initial_window_bytes: u32,
    ) -> Self {
        Self {
            id,
            tunnel_generation,
            proxy_conn_id,
            proxy_stream_id,
            request_window: StreamFlowWindow::new(initial_window_bytes),
            response_consumed_since_update: Mutex::new(0),
            min_window_update_bytes: (initial_window_bytes / 4).clamp(1, 1024 * 1024),
            response_connection: Mutex::new(None),
            wait_state: Mutex::new(LocalWaitState::default()),
            headers_notify: Notify::new(),
            body: ResponseBuffer::new(initial_window_bytes as usize),
            terminal: AtomicBool::new(false),
        }
    }

    pub(crate) fn tunnel_generation(&self) -> &str {
        &self.tunnel_generation
    }

    async fn acquire_request_window(
        &self,
        bytes: usize,
        timeout: Duration,
    ) -> Result<Duration, ()> {
        self.request_window.acquire(bytes, timeout).await
    }

    fn add_request_window(&self, delta: u32) {
        self.request_window.add(delta);
    }

    async fn flush_response_credit(&self) -> Result<(), String> {
        if self.terminal.load(Ordering::Acquire) {
            return Ok(());
        }
        let connection = self
            .response_connection
            .lock()
            .as_ref()
            .and_then(std::sync::Weak::upgrade);
        let Some(connection) = connection else {
            return Ok(());
        };
        if connection.protocol_version() < 3 {
            return Ok(());
        }
        let delta = {
            let consumed = self.response_consumed_since_update.lock();
            if *consumed < u64::from(self.min_window_update_bytes) {
                return Ok(());
            }
            (*consumed).min(u64::from(u32::MAX)) as u32
        };
        let frame = protocol::encode_window_update(self.proxy_stream_id, delta);
        if connection
            .send_wait(Message::Binary(frame.into()), OUTBOUND_BACKPRESSURE_TIMEOUT)
            .await
            == SendStatus::Queued
        {
            let mut consumed = self.response_consumed_since_update.lock();
            *consumed = consumed.saturating_sub(u64::from(delta));
            return Ok(());
        }
        if self.terminal.load(Ordering::Acquire) {
            return Ok(());
        }
        connection.request_close();
        Err("proxy flow-control update failed".to_string())
    }

    pub async fn wait_headers(&self, timeout: Duration) -> Result<LocalResponseHead, String> {
        tokio::time::timeout(timeout, async {
            loop {
                let notified = self.headers_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let outcome = {
                    let state = self.wait_state.lock();
                    if let Some(response) = &state.response {
                        return Ok(response.clone());
                    }
                    state.error.clone()
                };
                if let Some(error) = outcome {
                    return Err(error);
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| "timed out waiting for response headers".to_string())?
    }

    pub fn take_body_receiver(self: &Arc<Self>) -> Option<LocalBodyReceiver> {
        self.body.take_receiver().map(|receiver| LocalBodyReceiver {
            receiver,
            stream: Arc::clone(self),
            failed: false,
            pending: None,
        })
    }

    fn set_response_headers(&self, meta: protocol::ResponseMeta) {
        let mut notify = false;
        {
            let mut state = self.wait_state.lock();
            if state.response.is_none() && state.error.is_none() {
                state.response = Some(LocalResponseHead {
                    status: meta.status,
                    headers: meta.headers,
                });
                notify = true;
            }
        }
        if notify {
            self.headers_notify.notify_waiters();
        }
    }

    fn push_body_chunk(&self, payload: Bytes) -> bool {
        if self.terminal.load(Ordering::Acquire) {
            return false;
        }
        self.body.push(payload)
    }

    fn finish(&self) {
        if self.terminal.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut notify = false;
        {
            let mut state = self.wait_state.lock();
            if state.response.is_none() && state.error.is_none() {
                state.error = Some("stream ended before response headers".to_string());
                notify = true;
            }
        }
        if notify {
            self.headers_notify.notify_waiters();
        }
        self.request_window.close();
        self.body.finish(Ok(()));
    }

    fn fail(&self, error: impl Into<String>) {
        if self.terminal.swap(true, Ordering::AcqRel) {
            return;
        }

        let error = error.into();
        let mut notify = false;
        {
            let mut state = self.wait_state.lock();
            if state.response.is_none() && state.error.is_none() {
                state.error = Some(error.clone());
                notify = true;
            }
        }
        if notify {
            self.headers_notify.notify_waiters();
        }
        self.request_window.close();
        self.body.finish(Err(error));
    }
}

pub struct LocalBodyReceiver {
    receiver: BodyReceiver,
    stream: Arc<LocalStream>,
    failed: bool,
    pending: Option<LocalBodyEvent>,
}

impl LocalBodyReceiver {
    pub async fn recv(&mut self) -> Option<LocalBodyEvent> {
        if self.failed {
            return None;
        }
        if self.pending.is_none() {
            let event = self.receiver.recv().await?;
            if let LocalBodyEvent::Chunk(chunk) = &event {
                let mut consumed = self.stream.response_consumed_since_update.lock();
                *consumed = consumed.saturating_add(chunk.len() as u64);
            }
            self.pending = Some(event);
        }
        if matches!(self.pending, Some(LocalBodyEvent::Chunk(_))) {
            if let Err(error) = self.stream.flush_response_credit().await {
                self.failed = true;
                return Some(LocalBodyEvent::Error(error));
            }
        }
        self.pending.take()
    }
}

pub struct HubRouter {
    proxy_conns: RwLock<HashMap<String, Vec<Arc<ProxyConn>>>>,
    proxy_conns_by_id: DashMap<u64, Arc<ProxyConn>>,
    local_streams: DashMap<u64, Arc<LocalStream>>,
    proxy_to_local: DashMap<(u64, u32), u64>,
    next_conn_id: AtomicU64,
    next_local_stream_id: AtomicU64,
    control_plane: ControlPlaneClient,
    node_status_tx: BoundedQueueSender<NodeStatusEvent>,
    soft_avoid_selection_total: AtomicU64,
    selection_retry_total: AtomicU64,
    selection_unavailable_total: AtomicU64,
    scheduler_selected_conn_total: AtomicU64,
    stream_reset_total: AtomicU64,
    stream_reset_reasons: Mutex<HashMap<String, u64>>,
    drain_total: AtomicU64,
    drain_reasons: Mutex<HashMap<String, u64>>,
}

struct PendingStreamGuard<'router> {
    hub: &'router HubRouter,
    connection: &'router ProxyConn,
    stream_id: u64,
    committed: bool,
}

impl Drop for PendingStreamGuard<'_> {
    fn drop(&mut self) {
        if !self.committed && self.hub.cleanup_local_stream(self.stream_id) {
            self.connection.release_stream();
        }
    }
}

struct NodeStatusEvent {
    node_id: String,
    authenticated_key: Option<String>,
    tunnel_generation: String,
    connection: Option<Arc<ProxyConn>>,
    connected: bool,
    conn_count: usize,
    observed_at_unix_secs: u64,
}

impl HubRouter {
    pub fn new(control_plane: ControlPlaneClient) -> Arc<Self> {
        let (node_status_tx, mut node_status_rx) =
            bounded_queue::<NodeStatusEvent>(*NODE_STATUS_QUEUE_CAPACITY);
        let worker_control_plane = control_plane.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                while let Some(event) = node_status_rx.recv().await {
                    let connection = event.connection.clone();
                    let result = match connection {
                        Some(connection) => {
                            worker_control_plane
                                .push_node_status_for_connection(
                                    connection,
                                    event.connected,
                                    event.conn_count,
                                    event.observed_at_unix_secs,
                                )
                                .await
                        }
                        None => {
                            worker_control_plane
                                .push_node_status(
                                    &event.node_id,
                                    event.authenticated_key.as_deref(),
                                    &event.tunnel_generation,
                                    event.connected,
                                    event.conn_count,
                                    event.observed_at_unix_secs,
                                )
                                .await
                        }
                    };
                    if let Err(error) = result {
                        if super::control_plane::is_credential_revoked_error(&error) {
                            if let Some(connection) = event.connection.as_ref() {
                                connection.request_close();
                            }
                        }
                        warn!(
                            node_id = %event.node_id,
                            connected = event.connected,
                            conn_count = event.conn_count,
                            observed_at_unix_secs = event.observed_at_unix_secs,
                            error = %error,
                            "failed to push node status to app control plane"
                        );
                    }
                }
            });
        }

        Arc::new(Self {
            proxy_conns: RwLock::new(HashMap::new()),
            proxy_conns_by_id: DashMap::new(),
            local_streams: DashMap::new(),
            proxy_to_local: DashMap::new(),
            next_conn_id: AtomicU64::new(1),
            next_local_stream_id: AtomicU64::new(1),
            control_plane,
            node_status_tx,
            soft_avoid_selection_total: AtomicU64::new(0),
            selection_retry_total: AtomicU64::new(0),
            selection_unavailable_total: AtomicU64::new(0),
            scheduler_selected_conn_total: AtomicU64::new(0),
            stream_reset_total: AtomicU64::new(0),
            stream_reset_reasons: Mutex::new(HashMap::new()),
            drain_total: AtomicU64::new(0),
            drain_reasons: Mutex::new(HashMap::new()),
        })
    }

    pub fn alloc_conn_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn register_proxy(&self, conn: Arc<ProxyConn>) {
        let node_id = conn.node_id.clone();
        let node_name = conn.node_name.clone();
        let conn_id = conn.id;
        self.proxy_conns_by_id.insert(conn_id, conn.clone());

        let healthy_count = {
            let mut map = self.proxy_conns.write();
            map.entry(node_id.clone()).or_default().push(conn.clone());
            available_conn_count(map.get(&node_id).map(Vec::as_slice).unwrap_or(&[]))
        };

        info!(
            node_id = %node_id,
            node_name = %node_name,
            conn_id = conn_id,
            healthy_connections = healthy_count,
            "proxy connected"
        );

        self.notify_node_status(
            node_id,
            conn.authenticated_key.clone(),
            Some(conn),
            healthy_count > 0,
            healthy_count,
        );
    }

    pub fn unregister_proxy(&self, conn_id: u64, node_id: &str) {
        let disconnected_connection = self
            .proxy_conns_by_id
            .remove(&conn_id)
            .map(|(_, connection)| connection);
        let disconnected_authenticated_key = disconnected_connection
            .as_ref()
            .and_then(|connection| connection.authenticated_key.clone());

        let healthy_count = {
            let mut map = self.proxy_conns.write();
            if let Some(conns) = map.get_mut(node_id) {
                conns.retain(|c| c.id != conn_id);
                if conns.is_empty() {
                    map.remove(node_id);
                }
            }
            map.get(node_id)
                .map(|v| available_conn_count(v.as_slice()))
                .unwrap_or(0)
        };

        info!(
            node_id = %node_id,
            conn_id = conn_id,
            healthy_connections = healthy_count,
            "proxy disconnected"
        );

        self.cancel_streams_for_proxy(conn_id);
        let authenticated_key = self
            .proxy_conns
            .read()
            .get(node_id)
            .and_then(|connections| connections.first())
            .and_then(|connection| connection.authenticated_key.clone())
            .or(disconnected_authenticated_key);
        self.notify_node_status(
            node_id.to_string(),
            authenticated_key,
            disconnected_connection,
            healthy_count > 0,
            healthy_count,
        );
    }

    pub fn request_close_all_proxies(&self) -> usize {
        let conns = self
            .proxy_conns_by_id
            .iter()
            .map(|entry| Arc::clone(entry.value()))
            .collect::<Vec<_>>();
        let total = conns.len();
        for conn in conns {
            conn.request_close();
        }
        total
    }

    pub(crate) fn request_close_proxy(&self, conn_id: u64) -> bool {
        let Some(conn) = self
            .proxy_conns_by_id
            .get(&conn_id)
            .map(|entry| Arc::clone(entry.value()))
        else {
            return false;
        };
        conn.request_close();
        true
    }

    pub(crate) fn request_close_proxies_for_node(&self, node_id: &str) -> usize {
        let conns = self.proxy_connections_for_node(node_id);
        let total = conns.len();
        for conn in conns {
            conn.request_close();
        }
        total
    }

    pub(crate) fn proxy_connections_for_node(&self, node_id: &str) -> Vec<Arc<ProxyConn>> {
        self.proxy_conns
            .read()
            .get(node_id)
            .map(|connections| connections.to_vec())
            .unwrap_or_default()
    }

    fn notify_node_status(
        &self,
        node_id: String,
        authenticated_key: Option<String>,
        connection: Option<Arc<ProxyConn>>,
        connected: bool,
        conn_count: usize,
    ) {
        let tunnel_generation = connection
            .as_ref()
            .map(|connection| connection.node_generation.clone())
            .unwrap_or_default();
        let event = NodeStatusEvent {
            node_id,
            authenticated_key,
            tunnel_generation,
            connection,
            connected,
            conn_count,
            observed_at_unix_secs: current_unix_secs(),
        };
        match self.node_status_tx.try_send(event) {
            Ok(()) => {}
            Err(QueueSendError::Full(event)) => {
                let rejected_total = self.node_status_tx.snapshot().rejected_full_total;
                if rejected_total.is_power_of_two() {
                    warn!(
                        node_id = %event.node_id,
                        connected = event.connected,
                        conn_count = event.conn_count,
                        observed_at_unix_secs = event.observed_at_unix_secs,
                        queue_capacity = self.node_status_tx.capacity(),
                        rejected_total,
                        "node status queue full; dropping stale status event"
                    );
                }
            }
            Err(QueueSendError::Closed(event)) => {
                warn!(
                    node_id = %event.node_id,
                    connected = event.connected,
                    conn_count = event.conn_count,
                    observed_at_unix_secs = event.observed_at_unix_secs,
                    "node status worker unavailable"
                );
            }
        }
    }

    fn notify_current_node_status(&self, node_id: &str) {
        let (healthy_count, connection, authenticated_key) = {
            let map = self.proxy_conns.read();
            let connections = map.get(node_id).map(Vec::as_slice).unwrap_or(&[]);
            (
                available_conn_count(connections),
                connections.first().cloned(),
                connections
                    .first()
                    .and_then(|connection| connection.authenticated_key.clone()),
            )
        };
        self.notify_node_status(
            node_id.to_string(),
            authenticated_key,
            connection,
            healthy_count > 0,
            healthy_count,
        );
    }

    fn record_stream_reset(&self, reason: &str) {
        self.stream_reset_total.fetch_add(1, Ordering::Relaxed);
        increment_reason(&self.stream_reset_reasons, reason);
    }

    fn record_drain(&self, reason: &str) {
        self.drain_total.fetch_add(1, Ordering::Relaxed);
        increment_reason(&self.drain_reasons, reason);
    }

    fn ranked_proxy_conn_candidates(
        &self,
        node_id: &str,
        authorized_conn_ids: Option<&HashSet<u64>>,
    ) -> Vec<ProxyConnCandidate> {
        let conns = {
            let map = self.proxy_conns.read();
            map.get(node_id)
                .map(|entries| entries.to_vec())
                .unwrap_or_default()
        };
        let mut candidates = conns
            .into_iter()
            .filter_map(|conn| {
                if authorized_conn_ids.is_some_and(|allowed| !allowed.contains(&conn.id)) {
                    return None;
                }
                let snapshot = conn.snapshot();
                snapshot
                    .available
                    .then_some(ProxyConnCandidate { conn, snapshot })
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|candidate| candidate.rank_key());
        candidates
    }

    pub fn has_local_proxy(&self, node_id: &str) -> bool {
        !self.ranked_proxy_conn_candidates(node_id, None).is_empty()
    }

    pub async fn open_local_stream(
        &self,
        node_id: &str,
        meta: &protocol::RequestMeta,
    ) -> Result<Arc<LocalStream>, String> {
        self.open_local_stream_with_authorized_connections(node_id, meta, None)
            .await
    }

    pub(crate) async fn open_local_stream_with_authorized_connections(
        &self,
        node_id: &str,
        meta: &protocol::RequestMeta,
        authorized_conn_ids: Option<&HashSet<u64>>,
    ) -> Result<Arc<LocalStream>, String> {
        let candidates = self.ranked_proxy_conn_candidates(node_id, authorized_conn_ids);
        if candidates.is_empty() {
            self.selection_unavailable_total
                .fetch_add(1, Ordering::Relaxed);
            self.warn_no_available_proxy_connection(node_id);
            return Err(format!("no proxy connection for node {node_id}"));
        }

        let mut skipped_candidates = 0usize;
        let mut selected_candidate = None;
        let mut proxy_stream_id = None;
        for candidate in candidates {
            match candidate.conn.alloc_stream_id() {
                Some(stream_id) => {
                    proxy_stream_id = Some(stream_id);
                    selected_candidate = Some(candidate);
                    break;
                }
                None => skipped_candidates = skipped_candidates.saturating_add(1),
            }
        }

        let Some(candidate) = selected_candidate else {
            self.selection_unavailable_total
                .fetch_add(1, Ordering::Relaxed);
            return Err(format!("stream limit reached for node {node_id}"));
        };
        if skipped_candidates > 0 {
            self.selection_retry_total.fetch_add(1, Ordering::Relaxed);
        }
        if candidate.snapshot.soft_avoid {
            self.soft_avoid_selection_total
                .fetch_add(1, Ordering::Relaxed);
            debug!(
                node_id = %node_id,
                conn_id = candidate.snapshot.conn_id,
                queue_pressure_percent = candidate.snapshot.queue_pressure_percent,
                stream_pressure_percent = candidate.snapshot.stream_pressure_percent,
                "selected high-pressure proxy connection because no lower-pressure alternative was available"
            );
        }
        let proxy_conn = candidate.conn;
        let proxy_stream_id = proxy_stream_id.expect("selected candidate should carry a stream id");
        self.scheduler_selected_conn_total
            .fetch_add(1, Ordering::Relaxed);

        // Encode frames before registering the stream so that encoding failures
        // (practically impossible but theoretically possible) don't leak a stream
        // slot or orphan map entries.
        let meta_json = match serde_json::to_vec(meta) {
            Ok(json) => json,
            Err(e) => {
                proxy_conn.release_stream();
                return Err(format!("failed to encode request metadata: {e}"));
            }
        };
        let (meta_payload, meta_flags) = match protocol::compress_payload(&meta_json) {
            Ok(result) => result,
            Err(e) => {
                proxy_conn.release_stream();
                return Err(format!("failed to compress request metadata: {e}"));
            }
        };
        let header_frame = protocol::encode_frame(
            proxy_stream_id,
            protocol::REQUEST_HEADERS,
            meta_flags,
            &meta_payload,
        );

        // Frames encoded successfully -- now register the stream.
        let local_stream_id = self.next_local_stream_id.fetch_add(1, Ordering::Relaxed);
        let settings = proxy_conn.settings.lock().clone();
        let mut local_stream = LocalStream::new(
            local_stream_id,
            proxy_conn.node_generation.clone(),
            proxy_conn.id,
            proxy_stream_id,
            settings.initial_stream_window_bytes,
        );
        local_stream.min_window_update_bytes = settings.min_window_update_bytes;
        *local_stream.response_connection.get_mut() = Some(Arc::downgrade(&proxy_conn));
        let local_stream = Arc::new(local_stream);
        self.local_streams
            .insert(local_stream_id, local_stream.clone());
        self.proxy_to_local
            .insert((proxy_conn.id, proxy_stream_id), local_stream_id);
        let mut pending_stream = PendingStreamGuard {
            hub: self,
            connection: &proxy_conn,
            stream_id: local_stream_id,
            committed: false,
        };

        let send_status = proxy_conn
            .send_wait(
                Message::Binary(header_frame.into()),
                OUTBOUND_BACKPRESSURE_TIMEOUT,
            )
            .await;
        debug!(
            node_id = %node_id,
            conn_id = proxy_conn.id,
            proxy_stream_id = proxy_stream_id,
            local_stream_id = local_stream_id,
            stream_count = proxy_conn.stream_count.load(Ordering::Relaxed),
            queue_depth = proxy_conn.outbound.snapshot().depth,
            send_status = ?send_status,
            "open_local_stream dispatched"
        );
        match send_status {
            SendStatus::Queued => {
                pending_stream.committed = true;
                Ok(local_stream)
            }
            SendStatus::Closed | SendStatus::Congested => {
                Err("proxy connection congested".to_string())
            }
        }
    }

    fn warn_no_available_proxy_connection(&self, node_id: &str) {
        let conns = {
            let map = self.proxy_conns.read();
            map.get(node_id)
                .map(|entries| entries.to_vec())
                .unwrap_or_default()
        };
        if conns.is_empty() {
            return;
        }
        let snapshots = conns.iter().map(|conn| conn.snapshot()).collect::<Vec<_>>();
        warn!(
            node_id = %node_id,
            total_conns = snapshots.len(),
            available = snapshots.iter().filter(|snapshot| snapshot.available).count(),
            closing = snapshots.iter().filter(|snapshot| snapshot.closing).count(),
            draining = snapshots.iter().filter(|snapshot| snapshot.draining).count(),
            soft_avoid = snapshots.iter().filter(|snapshot| snapshot.soft_avoid).count(),
            "no available proxy connection despite registered connections"
        );
    }

    pub async fn push_local_request_body(
        &self,
        local_stream_id: u64,
        payload: Bytes,
        end_stream: bool,
    ) -> Result<(), String> {
        let stream = self
            .local_streams
            .get(&local_stream_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| "local stream not found".to_string())?;
        let proxy_conn = self
            .proxy_conns_by_id
            .get(&stream.proxy_conn_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| "proxy connection unavailable".to_string())?;

        let chunk_size = MAX_REQUEST_BODY_FRAME_SIZE
            .min(proxy_conn.settings.lock().initial_stream_window_bytes as usize);
        let total_chunks = payload.len().div_ceil(chunk_size);
        let result = if total_chunks == 0 {
            if end_stream {
                self.send_request_body_frame(&proxy_conn, &stream, &[], true)
                    .await
            } else {
                Ok(())
            }
        } else {
            for (index, chunk) in payload.chunks(chunk_size).enumerate() {
                let is_last_chunk = index + 1 == total_chunks;
                if let Err(error) = self
                    .send_request_body_frame(
                        &proxy_conn,
                        &stream,
                        chunk,
                        end_stream && is_last_chunk,
                    )
                    .await
                {
                    self.cancel_local_stream(local_stream_id, &error);
                    return Err(error);
                }
            }
            Ok(())
        };

        if let Err(error) = result {
            self.cancel_local_stream(local_stream_id, &error);
            return Err(error);
        }

        Ok(())
    }

    async fn send_request_body_frame(
        &self,
        proxy_conn: &Arc<ProxyConn>,
        stream: &Arc<LocalStream>,
        payload: &[u8],
        end_stream: bool,
    ) -> Result<(), String> {
        if proxy_conn.protocol_version() >= 3 && !payload.is_empty() {
            match stream
                .acquire_request_window(payload.len(), OUTBOUND_BACKPRESSURE_TIMEOUT)
                .await
            {
                Ok(waited) => {
                    if waited > Duration::from_millis(1) {
                        proxy_conn
                            .backpressure_total
                            .fetch_add(1, Ordering::Relaxed);
                        proxy_conn.flow_window_blocked_ms.fetch_add(
                            u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
                            Ordering::Relaxed,
                        );
                    }
                }
                Err(()) => {
                    self.record_stream_reset("request_window_timeout");
                    return Err(
                        "proxy stream reset: request flow-control window timeout".to_string()
                    );
                }
            }
        }

        let (body_payload, body_flags) = protocol::raw_payload(payload);
        let body_frame = protocol::encode_frame(
            stream.proxy_stream_id,
            protocol::REQUEST_BODY,
            body_flags
                | if end_stream {
                    protocol::FLAG_END_STREAM
                } else {
                    0
                },
            &body_payload,
        );
        match proxy_conn
            .send_wait(
                Message::Binary(body_frame.into()),
                OUTBOUND_BACKPRESSURE_TIMEOUT,
            )
            .await
        {
            SendStatus::Queued => Ok(()),
            SendStatus::Closed | SendStatus::Congested => {
                self.record_stream_reset("outbound_backpressure_timeout");
                Err("proxy stream reset: outbound tunnel backpressure timeout".to_string())
            }
        }
    }

    pub fn cancel_local_stream(&self, local_stream_id: u64, reason: &str) {
        let Some((_, stream)) = self.local_streams.remove(&local_stream_id) else {
            return;
        };

        self.proxy_to_local
            .remove(&(stream.proxy_conn_id, stream.proxy_stream_id));
        if let Some(pc) = self.proxy_conns_by_id.get(&stream.proxy_conn_id) {
            pc.release_stream();
            self.record_stream_reset(reason);
            let frame = if pc.protocol_version() >= 3 {
                protocol::encode_reset_stream(stream.proxy_stream_id, reason)
            } else {
                protocol::encode_stream_error(stream.proxy_stream_id, reason)
            };
            if pc.send(Message::Binary(frame.into())) != SendStatus::Queued {
                pc.request_close();
            }
        }
        stream.fail(reason.to_string());
    }

    fn cleanup_local_stream(&self, local_stream_id: u64) -> bool {
        let Some((_, stream)) = self.local_streams.remove(&local_stream_id) else {
            return false;
        };
        self.proxy_to_local
            .remove(&(stream.proxy_conn_id, stream.proxy_stream_id));
        true
    }

    pub async fn handle_proxy_frame(self: &Arc<Self>, proxy_conn_id: u64, data: &mut [u8]) {
        let header = match protocol::FrameHeader::parse(data) {
            Some(h) => h,
            None => return,
        };
        let Some(expected_len) = protocol::HEADER_SIZE.checked_add(header.payload_len as usize)
        else {
            return;
        };
        if data.len() != expected_len {
            if header.stream_id != 0 {
                self.fail_proxy_stream(proxy_conn_id, header.stream_id, "invalid frame length");
            }
            return;
        }

        match header.msg_type {
            protocol::RESPONSE_HEADERS => {
                self.route_response_headers(proxy_conn_id, header, data);
            }
            protocol::RESPONSE_BODY => {
                self.route_response_body(proxy_conn_id, header, data).await;
            }
            protocol::STREAM_END => {
                self.finish_proxy_stream(proxy_conn_id, header.stream_id);
            }
            protocol::STREAM_ERROR => {
                let raw_message = protocol::decode_payload_with_limit(
                    data,
                    &header,
                    MAX_TUNNEL_CONTROL_PAYLOAD_SIZE,
                )
                .ok()
                .and_then(|payload| String::from_utf8(payload).ok())
                .unwrap_or_else(|| "stream error".to_string());
                let message = safe_peer_stream_error(&raw_message);
                self.record_stream_reset(&message);
                self.fail_proxy_stream(proxy_conn_id, header.stream_id, message);
            }
            protocol::RESET_STREAM => {
                let raw_message = protocol::decode_payload_with_limit(
                    data,
                    &header,
                    MAX_TUNNEL_CONTROL_PAYLOAD_SIZE,
                )
                .ok()
                .and_then(|payload| {
                    serde_json::from_slice::<protocol::ResetStreamPayload>(&payload)
                        .ok()
                        .map(|payload| payload.reason)
                })
                .unwrap_or_else(|| "stream reset".to_string());
                let message = safe_peer_stream_error(&raw_message);
                self.record_stream_reset(&message);
                self.fail_proxy_stream(proxy_conn_id, header.stream_id, message);
            }
            protocol::HEARTBEAT_DATA => {
                self.handle_heartbeat(proxy_conn_id, header.stream_id, data, &header)
                    .await;
            }
            protocol::PING => {
                let payload = protocol::frame_payload_by_header(data, &header).unwrap_or(&[]);
                let pong = protocol::encode_pong(payload);
                let pc = self
                    .proxy_conns_by_id
                    .get(&proxy_conn_id)
                    .map(|entry| entry.value().clone());
                if let Some(pc) = pc {
                    let _ = pc.send(Message::Binary(pong.into()));
                }
            }
            protocol::PONG => {}
            protocol::GOAWAY => {
                if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
                    let first = pc.mark_draining();
                    if first {
                        let drain = protocol::decode_payload_with_limit(
                            data,
                            &header,
                            MAX_TUNNEL_CONTROL_PAYLOAD_SIZE,
                        )
                        .ok()
                        .and_then(|payload| {
                            if payload.is_empty() {
                                None
                            } else {
                                serde_json::from_slice::<protocol::GoAwayPayload>(&payload).ok()
                            }
                        });
                        let reason = drain
                            .as_ref()
                            .map(|payload| payload.reason.as_str())
                            .unwrap_or("goaway");
                        let deadline_ms = drain
                            .as_ref()
                            .map(|payload| payload.drain_deadline_ms)
                            .unwrap_or(*DRAIN_DEADLINE_MS);
                        let last_accepted_stream_id = drain
                            .as_ref()
                            .map(|payload| payload.last_accepted_stream_id)
                            .unwrap_or(u32::MAX);
                        self.record_drain(reason);
                        warn!(
                            proxy_conn_id = proxy_conn_id,
                            node_id = %pc.node_id,
                            drain_deadline_ms = deadline_ms,
                            last_accepted_stream_id = last_accepted_stream_id,
                            reason = reason,
                            "received GOAWAY from proxy connection; marking connection draining"
                        );
                        let node_id = pc.node_id.clone();
                        drop(pc);
                        self.notify_current_node_status(&node_id);
                        self.reset_streams_after_last_accepted(
                            proxy_conn_id,
                            last_accepted_stream_id,
                            "goaway_last_accepted_exceeded",
                        );
                        self.schedule_drain_deadline(proxy_conn_id, deadline_ms, reason);
                    }
                }
            }
            protocol::HELLO => {
                if let Some(payload) = protocol::decode_payload_with_limit(
                    data,
                    &header,
                    MAX_TUNNEL_CONTROL_PAYLOAD_SIZE,
                )
                .ok()
                .and_then(|payload| serde_json::from_slice::<protocol::HelloPayload>(&payload).ok())
                {
                    if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
                        pc.update_protocol_version(payload.protocol_version);
                    }
                }
                debug!(
                    msg_type = header.msg_type,
                    proxy_conn_id = proxy_conn_id,
                    "received tunnel protocol v3 HELLO from proxy"
                );
            }
            protocol::SETTINGS => {
                let settings = protocol::decode_payload_with_limit(
                    data,
                    &header,
                    MAX_TUNNEL_CONTROL_PAYLOAD_SIZE,
                )
                .ok()
                .and_then(|payload| {
                    serde_json::from_slice::<protocol::SettingsPayload>(&payload).ok()
                })
                .filter(|settings| settings.is_valid());
                if let Some(connection) = self.proxy_conns_by_id.get(&proxy_conn_id) {
                    if header.stream_id != 0 || header.flags != 0 {
                        connection.request_close();
                        return;
                    }
                    let Some(settings) = settings else {
                        connection.request_close();
                        return;
                    };
                    let local = local_settings();
                    let settings = settings
                        .negotiate(local.initial_stream_window_bytes, local.drain_deadline_ms);
                    let mut current = connection.settings.lock();
                    if connection.stream_count.load(Ordering::Acquire) > 0 && *current != settings {
                        drop(current);
                        connection.request_close();
                        return;
                    }
                    *current = settings;
                }
            }
            protocol::WINDOW_UPDATE => {
                self.handle_window_update(proxy_conn_id, header.stream_id, data, &header);
            }
            protocol::LOAD_REPORT => {
                if let Some(payload) = protocol::decode_payload_with_limit(
                    data,
                    &header,
                    MAX_TUNNEL_CONTROL_PAYLOAD_SIZE,
                )
                .ok()
                .and_then(|payload| {
                    serde_json::from_slice::<protocol::LoadReportPayload>(&payload).ok()
                }) {
                    if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
                        pc.update_remote_health_score(payload.health_score);
                    }
                }
                debug!(
                    msg_type = header.msg_type,
                    proxy_conn_id = proxy_conn_id,
                    "received tunnel protocol v3 control frame from proxy"
                );
            }
            protocol::CONNECTION_CLOSE => {
                if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
                    let node_id = pc.node_id.clone();
                    pc.request_close();
                    drop(pc);
                    self.notify_current_node_status(&node_id);
                }
            }
            _ => {
                debug!(
                    msg_type = header.msg_type,
                    proxy_conn_id = proxy_conn_id,
                    "unexpected frame type from proxy"
                );
            }
        }
    }

    fn handle_window_update(
        &self,
        proxy_conn_id: u64,
        proxy_stream_id: u32,
        data: &[u8],
        header: &protocol::FrameHeader,
    ) {
        let Some(delta) =
            protocol::decode_payload_with_limit(data, header, MAX_TUNNEL_CONTROL_PAYLOAD_SIZE)
                .ok()
                .and_then(|payload| {
                    serde_json::from_slice::<protocol::WindowUpdatePayload>(&payload).ok()
                })
                .map(|payload| payload.delta_bytes)
        else {
            return;
        };
        let Some(local_id) = self.lookup_local_stream(proxy_conn_id, proxy_stream_id) else {
            return;
        };
        if let Some(stream) = self.local_streams.get(&local_id) {
            stream.add_request_window(delta);
            debug!(
                proxy_conn_id,
                proxy_stream_id,
                local_stream_id = local_id,
                delta_bytes = delta,
                "applied tunnel request WINDOW_UPDATE"
            );
        }
    }

    fn schedule_drain_deadline(
        self: &Arc<Self>,
        proxy_conn_id: u64,
        deadline_ms: u64,
        reason: &str,
    ) {
        let hub = Arc::clone(self);
        let reason = metric_reason(reason);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(deadline_ms)).await;
            let reset = hub.reset_streams_for_proxy(
                proxy_conn_id,
                &format!("drain_deadline_exceeded_{reason}"),
            );
            if reset > 0 {
                warn!(
                    proxy_conn_id,
                    reset_streams = reset,
                    deadline_ms,
                    "reset in-flight streams after tunnel drain deadline"
                );
            }
        });
    }

    fn reset_streams_after_last_accepted(
        &self,
        proxy_conn_id: u64,
        last_accepted_stream_id: u32,
        reason: &str,
    ) -> usize {
        if last_accepted_stream_id == u32::MAX {
            return 0;
        }

        let proxy_stream_ids = self
            .proxy_to_local
            .iter()
            .filter_map(|entry| {
                let (conn_id, stream_id) = *entry.key();
                (conn_id == proxy_conn_id && stream_id > last_accepted_stream_id)
                    .then_some(stream_id)
            })
            .collect::<Vec<_>>();

        let mut reset = 0usize;
        for proxy_stream_id in proxy_stream_ids {
            if self.reset_proxy_stream(proxy_conn_id, proxy_stream_id, reason) {
                reset += 1;
            }
        }
        reset
    }

    fn reset_streams_for_proxy(&self, proxy_conn_id: u64, reason: &str) -> usize {
        let proxy_stream_ids = self
            .proxy_to_local
            .iter()
            .filter_map(|entry| {
                let (conn_id, stream_id) = *entry.key();
                (conn_id == proxy_conn_id).then_some(stream_id)
            })
            .collect::<Vec<_>>();

        let mut reset = 0usize;
        for proxy_stream_id in proxy_stream_ids {
            if self.reset_proxy_stream(proxy_conn_id, proxy_stream_id, reason) {
                reset += 1;
            }
        }
        reset
    }

    fn reset_proxy_stream(&self, proxy_conn_id: u64, proxy_stream_id: u32, reason: &str) -> bool {
        let Some(stream) = self.handle_stream_cleanup(proxy_conn_id, proxy_stream_id) else {
            return false;
        };

        if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
            let frame = if pc.protocol_version() >= 3 {
                protocol::encode_reset_stream(proxy_stream_id, reason)
            } else {
                protocol::encode_stream_error(proxy_stream_id, reason)
            };
            let _ = pc.send(Message::Binary(frame.into()));
        }

        self.record_stream_reset(reason);
        stream.fail(reason.to_string());
        true
    }

    fn route_response_headers(
        &self,
        proxy_conn_id: u64,
        header: protocol::FrameHeader,
        data: &[u8],
    ) {
        let Some(local_id) = self.lookup_local_stream(proxy_conn_id, header.stream_id) else {
            return;
        };
        let Ok(payload) =
            protocol::decode_payload_with_limit(data, &header, MAX_TUNNEL_CONTROL_PAYLOAD_SIZE)
        else {
            self.fail_proxy_stream(
                proxy_conn_id,
                header.stream_id,
                "failed to decode response headers",
            );
            return;
        };
        let Ok(meta) = serde_json::from_slice::<protocol::ResponseMeta>(&payload) else {
            self.fail_proxy_stream(
                proxy_conn_id,
                header.stream_id,
                "invalid response headers payload",
            );
            return;
        };
        if let Some(entry) = self.local_streams.get(&local_id) {
            entry.value().set_response_headers(meta);
        }
    }

    async fn route_response_body(
        &self,
        proxy_conn_id: u64,
        header: protocol::FrameHeader,
        data: &[u8],
    ) {
        let Some(local_id) = self.lookup_local_stream(proxy_conn_id, header.stream_id) else {
            return;
        };
        let Ok(payload) = protocol::decode_payload_with_limit(
            data,
            &header,
            aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES,
        ) else {
            self.fail_proxy_stream(
                proxy_conn_id,
                header.stream_id,
                "failed to decode response body",
            );
            return;
        };

        let stream = match self.local_streams.get(&local_id) {
            Some(entry) => entry.value().clone(),
            None => return,
        };

        if !stream.push_body_chunk(Bytes::from(payload)) {
            self.cancel_local_stream(local_id, "local relay response congested");
        }
    }

    fn handle_stream_cleanup(
        &self,
        proxy_conn_id: u64,
        proxy_stream_id: u32,
    ) -> Option<Arc<LocalStream>> {
        let local_id = self
            .proxy_to_local
            .remove(&(proxy_conn_id, proxy_stream_id))
            .map(|(_, local_id)| local_id)?;

        let stream = self
            .local_streams
            .remove(&local_id)
            .map(|(_, stream)| stream)?;
        if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
            pc.release_stream();
        }
        Some(stream)
    }

    fn finish_proxy_stream(&self, proxy_conn_id: u64, proxy_stream_id: u32) {
        if let Some(stream) = self.handle_stream_cleanup(proxy_conn_id, proxy_stream_id) {
            stream.finish();
        }
    }

    fn fail_proxy_stream(
        &self,
        proxy_conn_id: u64,
        proxy_stream_id: u32,
        error: impl Into<String>,
    ) {
        if let Some(stream) = self.handle_stream_cleanup(proxy_conn_id, proxy_stream_id) {
            stream.fail(error.into());
        }
    }

    fn lookup_local_stream(&self, proxy_conn_id: u64, proxy_stream_id: u32) -> Option<u64> {
        self.proxy_to_local
            .get(&(proxy_conn_id, proxy_stream_id))
            .map(|entry| *entry.value())
    }

    async fn handle_heartbeat(
        &self,
        proxy_conn_id: u64,
        stream_id: u32,
        data: &[u8],
        header: &protocol::FrameHeader,
    ) {
        let payload = match protocol::decode_payload_with_limit(
            data,
            header,
            MAX_TUNNEL_CONTROL_PAYLOAD_SIZE,
        ) {
            Ok(payload) => payload,
            Err(error) => {
                warn!(proxy_conn_id = proxy_conn_id, error = %crate::error::redact_error_detail(&error), "failed to decode heartbeat payload");
                return;
            }
        };
        let Some(authenticated_node_id) = self
            .proxy_conns_by_id
            .get(&proxy_conn_id)
            .map(|entry| entry.node_id.clone())
        else {
            warn!(
                proxy_conn_id,
                "heartbeat rejected for unregistered proxy connection"
            );
            return;
        };
        let payload_node_id = match heartbeat_payload_node_id(&payload) {
            Ok(node_id) => node_id,
            Err(error) => {
                warn!(
                    proxy_conn_id,
                    authenticated_node_id = %authenticated_node_id,
                    error = %error,
                    "heartbeat rejected before control-plane dispatch"
                );
                return;
            }
        };
        if payload_node_id != authenticated_node_id {
            warn!(
                proxy_conn_id,
                authenticated_node_id = %authenticated_node_id,
                payload_node_id = %payload_node_id,
                "heartbeat rejected because node identity does not match tunnel authentication"
            );
            return;
        }
        let connection = self
            .proxy_conns_by_id
            .get(&proxy_conn_id)
            .map(|entry| Arc::clone(entry.value()));
        let Some(connection) = connection else {
            warn!(
                proxy_conn_id,
                "heartbeat rejected for unregistered proxy connection"
            );
            return;
        };
        let ack_payload = match self
            .control_plane
            .heartbeat_ack_for_connection(connection.clone(), &payload)
            .await
        {
            Ok(payload) => payload,
            Err(error) => {
                if super::control_plane::is_credential_revoked_error(&error) {
                    connection.request_close();
                }
                warn!(
                    proxy_conn_id = proxy_conn_id,
                    error = %error,
                    "control-plane heartbeat callback failed; keeping heartbeat pending"
                );
                return;
            }
        };
        let pc = self
            .proxy_conns_by_id
            .get(&proxy_conn_id)
            .map(|entry| entry.value().clone());
        if let Some(pc) = pc {
            let frame = protocol::encode_frame(stream_id, protocol::HEARTBEAT_ACK, 0, &ack_payload);
            let _ = pc
                .send_wait(Message::Binary(frame.into()), Duration::from_millis(250))
                .await;
        }
    }

    fn cancel_streams_for_proxy(&self, proxy_conn_id: u64) {
        let mut cancelled = 0usize;
        self.proxy_to_local.retain(|key, local_id| {
            if key.0 != proxy_conn_id {
                return true;
            }
            if let Some((_, stream)) = self.local_streams.remove(local_id) {
                stream.fail("proxy disconnected".to_string());
            }
            cancelled += 1;
            false
        });

        if cancelled > 0 {
            warn!(
                proxy_conn_id = proxy_conn_id,
                streams_cancelled = cancelled,
                "cancelled in-flight streams due to proxy disconnect"
            );
        }
    }

    pub fn stats(&self) -> HubStats {
        let node_status_queue = self.node_status_tx.snapshot();
        let proxy_conns = self
            .proxy_conns_by_id
            .iter()
            .map(|entry| entry.value().snapshot())
            .collect::<Vec<_>>();
        let total_proxy = proxy_conns.len();
        let nodes = self.proxy_conns.read().len();
        let available_proxy_connections = proxy_conns
            .iter()
            .filter(|snapshot| snapshot.available)
            .count();
        let closing_proxy_connections = proxy_conns
            .iter()
            .filter(|snapshot| snapshot.closing)
            .count();
        let draining_proxy_connections = proxy_conns
            .iter()
            .filter(|snapshot| snapshot.draining)
            .count();
        let soft_avoid_proxy_connections = proxy_conns
            .iter()
            .filter(|snapshot| snapshot.available && snapshot.soft_avoid)
            .count();
        let mut proxy_connections_by_state = BTreeMap::<String, usize>::new();
        for snapshot in &proxy_conns {
            *proxy_connections_by_state
                .entry(snapshot.state.as_str().to_string())
                .or_default() += 1;
        }
        let outbound_queue_depth_total = proxy_conns
            .iter()
            .map(|snapshot| snapshot.outbound.depth)
            .sum();
        let outbound_queue_depth_max = proxy_conns
            .iter()
            .map(|snapshot| snapshot.outbound.depth)
            .max()
            .unwrap_or(0);
        let outbound_queue_capacity_total = proxy_conns
            .iter()
            .map(|snapshot| snapshot.outbound.capacity)
            .sum();
        let outbound_queue_rejected_full_total = proxy_conns
            .iter()
            .map(|snapshot| snapshot.outbound.rejected_full_total)
            .sum();
        let outbound_queue_rejected_closed_total = proxy_conns
            .iter()
            .map(|snapshot| snapshot.outbound.rejected_closed_total)
            .sum();
        let proxy_connection_congested_total = proxy_conns
            .iter()
            .map(|snapshot| snapshot.congested_total)
            .sum();
        let body_backpressure_total = proxy_conns
            .iter()
            .map(|snapshot| snapshot.backpressure_total)
            .sum();
        let flow_window_blocked_ms = proxy_conns
            .iter()
            .map(|snapshot| snapshot.flow_window_blocked_ms)
            .sum();
        let connection_health_score_min = proxy_conns
            .iter()
            .map(|snapshot| snapshot.health_score)
            .min()
            .unwrap_or(100);
        let protocol_v1_proxy_connections = proxy_conns
            .iter()
            .filter(|snapshot| snapshot.protocol_version == 1)
            .count();
        let protocol_v2_proxy_connections = proxy_conns
            .iter()
            .filter(|snapshot| snapshot.protocol_version >= 2)
            .count();
        let protocol_v3_proxy_connections = proxy_conns
            .iter()
            .filter(|snapshot| snapshot.protocol_version >= 3)
            .count();
        let write_latency_last_us_max = proxy_conns
            .iter()
            .map(|snapshot| snapshot.write_latency_last_us)
            .max()
            .unwrap_or(0);
        let write_latency_ewma_us_max = proxy_conns
            .iter()
            .map(|snapshot| snapshot.write_latency_ewma_us)
            .max()
            .unwrap_or(0);

        HubStats {
            proxy_connections: total_proxy,
            available_proxy_connections,
            closing_proxy_connections,
            draining_proxy_connections,
            soft_avoid_proxy_connections,
            proxy_connections_by_state,
            protocol_v1_proxy_connections,
            protocol_v2_proxy_connections,
            protocol_v3_proxy_connections,
            nodes,
            active_streams: self.local_streams.len(),
            outbound_queue_depth_total,
            outbound_queue_depth_max,
            outbound_queue_capacity_total,
            outbound_queue_rejected_full_total,
            outbound_queue_rejected_closed_total,
            proxy_connection_congested_total,
            body_backpressure_total,
            flow_window_blocked_ms,
            connection_health_score_min,
            stream_initial_window_bytes: u64::from(*STREAM_INITIAL_WINDOW_BYTES),
            stream_reset_total: self.stream_reset_total.load(Ordering::Relaxed),
            stream_reset_reasons: reason_snapshot(&self.stream_reset_reasons),
            drain_total: self.drain_total.load(Ordering::Relaxed),
            drain_reasons: reason_snapshot(&self.drain_reasons),
            proxy_connection_write_latency_last_us_max: write_latency_last_us_max,
            proxy_connection_write_latency_ewma_us_max: write_latency_ewma_us_max,
            soft_avoid_selection_total: self.soft_avoid_selection_total.load(Ordering::Relaxed),
            selection_retry_total: self.selection_retry_total.load(Ordering::Relaxed),
            selection_unavailable_total: self.selection_unavailable_total.load(Ordering::Relaxed),
            scheduler_selected_conn_total: self
                .scheduler_selected_conn_total
                .load(Ordering::Relaxed),
            node_status_queue_capacity: node_status_queue.capacity,
            node_status_queue_depth: node_status_queue.depth,
            node_status_queue_high_watermark: node_status_queue.high_watermark,
            node_status_queue_enqueued_total: node_status_queue.enqueued_total,
            node_status_queue_rejected_full_total: node_status_queue.rejected_full_total,
            node_status_queue_rejected_closed_total: node_status_queue.rejected_closed_total,
        }
    }
}

fn heartbeat_payload_node_id(payload: &[u8]) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|_| "heartbeat payload is not valid JSON".to_string())?;
    let node_id = value
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "heartbeat payload is missing node_id".to_string())?;
    Ok(node_id.to_string())
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn percent_u64(value: usize, total: usize) -> u64 {
    if total == 0 {
        return 0;
    }
    ((value as u128) * 100 / (total as u128)) as u64
}

fn connection_health_score(queue_pressure_percent: u64, stream_pressure_percent: u64) -> u64 {
    let pressure_penalty = queue_pressure_percent
        .saturating_mul(3)
        .saturating_add(stream_pressure_percent.saturating_mul(2))
        / 5;
    100u64.saturating_sub(pressure_penalty.min(100))
}

fn available_conn_count(conns: &[Arc<ProxyConn>]) -> usize {
    conns.iter().filter(|conn| conn.is_available()).count()
}

fn increment_reason(reasons: &Mutex<HashMap<String, u64>>, raw_reason: &str) {
    let reason = metric_reason(raw_reason);
    let mut reasons = reasons.lock();
    *reasons.entry(reason).or_default() += 1;
}

fn reason_snapshot(reasons: &Mutex<HashMap<String, u64>>) -> BTreeMap<String, u64> {
    reasons
        .lock()
        .iter()
        .map(|(reason, value)| (reason.clone(), *value))
        .collect()
}

fn metric_reason(raw: &str) -> String {
    let mut normalized = String::new();
    let mut previous_underscore = false;
    for ch in raw.trim().chars().flat_map(char::to_lowercase) {
        let out = if ch.is_ascii_alphanumeric() {
            Some(ch)
        } else if matches!(ch, '_' | '-' | ' ' | ':' | '/' | '.') {
            Some('_')
        } else {
            None
        };
        let Some(out) = out else {
            continue;
        };
        if out == '_' {
            if previous_underscore {
                continue;
            }
            previous_underscore = true;
        } else {
            previous_underscore = false;
        }
        normalized.push(out);
        if normalized.len() >= 64 {
            break;
        }
    }
    let normalized = normalized.trim_matches('_').to_string();
    if normalized.is_empty() {
        "unspecified".to_string()
    } else {
        normalized
    }
}

fn safe_peer_stream_error(raw: &str) -> String {
    const CLASSIFICATION_PREFIX_BYTES: usize = 4 * 1024;

    let prefix = if raw.len() <= CLASSIFICATION_PREFIX_BYTES {
        raw
    } else {
        let mut end = CLASSIFICATION_PREFIX_BYTES;
        while !raw.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        &raw[..end]
    };
    let category = if contains_ascii_case_insensitive(prefix, "timed out")
        || contains_ascii_case_insensitive(prefix, "timeout")
    {
        "timeout"
    } else if contains_ascii_case_insensitive(prefix, "overloaded")
        || contains_ascii_case_insensitive(prefix, "backpressure")
        || contains_ascii_case_insensitive(prefix, "congested")
        || contains_ascii_case_insensitive(prefix, "window")
    {
        "overloaded"
    } else if contains_ascii_case_insensitive(prefix, "forbidden")
        || contains_ascii_case_insensitive(prefix, "unauthorized")
        || contains_ascii_case_insensitive(prefix, "authentication")
    {
        "forbidden"
    } else if contains_ascii_case_insensitive(prefix, "dns") {
        "dns"
    } else if contains_ascii_case_insensitive(prefix, "connect")
        || contains_ascii_case_insensitive(prefix, "socket")
    {
        "connect"
    } else if contains_ascii_case_insensitive(prefix, "cancel")
        || contains_ascii_case_insensitive(prefix, "reset")
    {
        "reset"
    } else {
        "relay"
    };
    format!("tunnel stream {category} error")
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|candidate| candidate.eq_ignore_ascii_case(needle.as_bytes()))
}

#[derive(serde::Serialize)]
pub struct HubStats {
    pub proxy_connections: usize,
    pub available_proxy_connections: usize,
    pub closing_proxy_connections: usize,
    pub draining_proxy_connections: usize,
    pub soft_avoid_proxy_connections: usize,
    pub proxy_connections_by_state: BTreeMap<String, usize>,
    pub protocol_v1_proxy_connections: usize,
    pub protocol_v2_proxy_connections: usize,
    pub protocol_v3_proxy_connections: usize,
    pub nodes: usize,
    pub active_streams: usize,
    pub outbound_queue_depth_total: usize,
    pub outbound_queue_depth_max: usize,
    pub outbound_queue_capacity_total: usize,
    pub outbound_queue_rejected_full_total: u64,
    pub outbound_queue_rejected_closed_total: u64,
    pub proxy_connection_congested_total: u64,
    pub body_backpressure_total: u64,
    pub flow_window_blocked_ms: u64,
    pub connection_health_score_min: u64,
    pub stream_initial_window_bytes: u64,
    pub stream_reset_total: u64,
    pub stream_reset_reasons: BTreeMap<String, u64>,
    pub drain_total: u64,
    pub drain_reasons: BTreeMap<String, u64>,
    pub proxy_connection_write_latency_last_us_max: u64,
    pub proxy_connection_write_latency_ewma_us_max: u64,
    pub soft_avoid_selection_total: u64,
    pub selection_retry_total: u64,
    pub selection_unavailable_total: u64,
    pub scheduler_selected_conn_total: u64,
    pub node_status_queue_capacity: usize,
    pub node_status_queue_depth: usize,
    pub node_status_queue_high_watermark: usize,
    pub node_status_queue_enqueued_total: u64,
    pub node_status_queue_rejected_full_total: u64,
    pub node_status_queue_rejected_closed_total: u64,
}

impl HubStats {
    pub fn to_metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![
            MetricSample::new(
                "tunnel_proxy_connections",
                "Current number of connected proxy sockets.",
                MetricKind::Gauge,
                self.proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_connections_available",
                "Current number of proxy connections available for new work.",
                MetricKind::Gauge,
                self.available_proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_connections_closing",
                "Current number of proxy connections marked closing.",
                MetricKind::Gauge,
                self.closing_proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_connections_draining",
                "Current number of proxy connections marked draining.",
                MetricKind::Gauge,
                self.draining_proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_connections_soft_avoid",
                "Current number of available proxy connections currently soft-avoided by the scheduler.",
                MetricKind::Gauge,
                self.soft_avoid_proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_connections_protocol_v1",
                "Current number of connected proxy sockets still using tunnel protocol v1.",
                MetricKind::Gauge,
                self.protocol_v1_proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_connections_protocol_v2",
                "Current number of connected proxy sockets using tunnel protocol v2 or newer.",
                MetricKind::Gauge,
                self.protocol_v2_proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_connections_protocol_v3",
                "Current number of connected proxy sockets using tunnel protocol v3 or newer.",
                MetricKind::Gauge,
                self.protocol_v3_proxy_connections as u64,
            ),
            MetricSample::new(
                "tunnel_nodes",
                "Current number of connected logical nodes.",
                MetricKind::Gauge,
                self.nodes as u64,
            ),
            MetricSample::new(
                "tunnel_active_streams",
                "Current number of active local relay streams.",
                MetricKind::Gauge,
                self.active_streams as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_outbound_queue_depth_total",
                "Current aggregate depth across proxy outbound queues.",
                MetricKind::Gauge,
                self.outbound_queue_depth_total as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_outbound_queue_depth_max",
                "Current maximum depth observed on a single proxy outbound queue.",
                MetricKind::Gauge,
                self.outbound_queue_depth_max as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_outbound_queue_capacity_total",
                "Current aggregate capacity across proxy outbound queues.",
                MetricKind::Gauge,
                self.outbound_queue_capacity_total as u64,
            ),
            MetricSample::new(
                "tunnel_proxy_outbound_queue_rejected_full_total",
                "Total proxy outbound queue sends rejected because a queue was full.",
                MetricKind::Counter,
                self.outbound_queue_rejected_full_total,
            ),
            MetricSample::new(
                "tunnel_proxy_outbound_queue_rejected_closed_total",
                "Total proxy outbound queue sends rejected because a queue was closed.",
                MetricKind::Counter,
                self.outbound_queue_rejected_closed_total,
            ),
            MetricSample::new(
                "tunnel_proxy_connection_congested_total",
                "Total number of times a proxy outbound queue became congested.",
                MetricKind::Counter,
                self.proxy_connection_congested_total,
            ),
            MetricSample::new(
                "tunnel_body_backpressure_total",
                "Total body-frame sends that waited on tunnel outbound backpressure.",
                MetricKind::Counter,
                self.body_backpressure_total,
            ),
            MetricSample::new(
                "tunnel_flow_window_blocked_ms",
                "Total milliseconds spent waiting on tunnel flow-control or outbound queue backpressure.",
                MetricKind::Counter,
                self.flow_window_blocked_ms,
            ),
            MetricSample::new(
                "tunnel_connection_health_score",
                "Minimum current tunnel connection health score across connected proxy sockets.",
                MetricKind::Gauge,
                self.connection_health_score_min,
            ),
            MetricSample::new(
                "tunnel_stream_initial_window_bytes",
                "Configured initial per-stream tunnel flow-control window in bytes.",
                MetricKind::Gauge,
                self.stream_initial_window_bytes,
            ),
            MetricSample::new(
                "tunnel_stream_reset_total",
                "Total tunnel streams reset by either side.",
                MetricKind::Counter,
                self.stream_reset_total,
            )
            .with_labels(vec![MetricLabel::new("reason", "all")]),
            MetricSample::new(
                "tunnel_drain_total",
                "Total tunnel drain events.",
                MetricKind::Counter,
                self.drain_total,
            )
            .with_labels(vec![MetricLabel::new("reason", "all")]),
            MetricSample::new(
                "tunnel_scheduler_selected_conn_total",
                "Total number of times the tunnel scheduler selected a proxy connection.",
                MetricKind::Counter,
                self.scheduler_selected_conn_total,
            ),
            MetricSample::new(
                "tunnel_proxy_connection_write_latency_last_us_max",
                "Maximum observed last write latency across proxy connections in microseconds.",
                MetricKind::Gauge,
                self.proxy_connection_write_latency_last_us_max,
            ),
            MetricSample::new(
                "tunnel_proxy_connection_write_latency_ewma_us_max",
                "Maximum observed write latency EWMA across proxy connections in microseconds.",
                MetricKind::Gauge,
                self.proxy_connection_write_latency_ewma_us_max,
            ),
            MetricSample::new(
                "tunnel_proxy_soft_avoid_selection_total",
                "Total number of times the scheduler had to pick a high-pressure proxy connection.",
                MetricKind::Counter,
                self.soft_avoid_selection_total,
            ),
            MetricSample::new(
                "tunnel_proxy_selection_retry_total",
                "Total number of times the scheduler retried a lower-ranked proxy connection after a race on stream allocation.",
                MetricKind::Counter,
                self.selection_retry_total,
            ),
            MetricSample::new(
                "tunnel_proxy_selection_unavailable_total",
                "Total number of relay selections that failed because no proxy connection was available.",
                MetricKind::Counter,
                self.selection_unavailable_total,
            ),
        ];

        for (state, count) in &self.proxy_connections_by_state {
            samples.push(
                MetricSample::new(
                    "tunnel_proxy_connections_state",
                    "Current number of proxy connections by health state.",
                    MetricKind::Gauge,
                    *count as u64,
                )
                .with_labels(vec![MetricLabel::new("state", state.clone())]),
            );
        }
        for (reason, value) in &self.stream_reset_reasons {
            samples.push(
                MetricSample::new(
                    "tunnel_stream_reset_total",
                    "Total tunnel streams reset by either side.",
                    MetricKind::Counter,
                    *value,
                )
                .with_labels(vec![MetricLabel::new("reason", reason.clone())]),
            );
        }
        for (reason, value) in &self.drain_reasons {
            samples.push(
                MetricSample::new(
                    "tunnel_drain_total",
                    "Total tunnel drain events.",
                    MetricKind::Counter,
                    *value,
                )
                .with_labels(vec![MetricLabel::new("reason", reason.clone())]),
            );
        }

        let node_status_queue = QueueSnapshot {
            capacity: self.node_status_queue_capacity,
            depth: self.node_status_queue_depth,
            high_watermark: self.node_status_queue_high_watermark,
            enqueued_total: self.node_status_queue_enqueued_total,
            rejected_full_total: self.node_status_queue_rejected_full_total,
            rejected_closed_total: self.node_status_queue_rejected_closed_total,
        };
        samples.extend(node_status_queue.to_metric_samples("tunnel_node_status"));

        samples
    }
}

#[cfg(test)]
mod tests {
    use aether_runtime::bounded_queue;

    use super::{
        protocol, safe_peer_stream_error, ControlPlaneClient, HubRouter, ProxyConn,
        MAX_REQUEST_BODY_FRAME_SIZE,
    };
    use axum::extract::ws::Message;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::watch;

    #[test]
    fn peer_stream_errors_are_projected_to_finite_categories() {
        let sensitive = safe_peer_stream_error(
            "request failed for https://alice:secret@10.0.0.8/private?token=query-secret\r\nx: y",
        );
        assert_eq!(sensitive, "tunnel stream relay error");
        for secret in ["alice", "secret", "10.0.0.8", "private", "query", "x: y"] {
            assert!(!sensitive.contains(secret), "leaked {secret}: {sensitive}");
        }
        assert_eq!(
            safe_peer_stream_error("upstream connect timeout: Bearer secret"),
            "tunnel stream timeout error"
        );
        assert_eq!(
            safe_peer_stream_error("outbound backpressure timeout"),
            "tunnel stream timeout error"
        );
    }

    fn build_meta() -> protocol::RequestMeta {
        protocol::RequestMeta {
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            method: "GET".to_string(),
            url: "https://example.com".to_string(),
            headers: HashMap::new(),
            stream: false,
            request_timeout_ms: None,
            stream_first_byte_timeout_ms: None,
            timeout: 30,
            follow_redirects: None,
            http1_only: false,
            transport_profile: None,
        }
    }

    #[tokio::test]
    async fn cancel_local_stream_notifies_proxy() {
        let hub = HubRouter::new(ControlPlaneClient::disabled());

        let (proxy_tx, mut proxy_rx) = bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        let proxy = Arc::new(ProxyConn::new(
            100,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        ));
        hub.register_proxy(proxy);

        let stream = hub
            .open_local_stream("node-1", &build_meta())
            .await
            .expect("open local stream");
        let _ = proxy_rx.try_recv().expect("headers frame");
        hub.push_local_request_body(stream.id, Bytes::new(), true)
            .await
            .expect("finish empty body");
        let _ = proxy_rx.try_recv().expect("body frame");

        hub.cancel_local_stream(stream.id, "client dropped");

        let cancelled = proxy_rx.try_recv().expect("cancel frame");
        let cancelled_data = match cancelled {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let header = protocol::FrameHeader::parse(&cancelled_data).expect("cancel frame header");
        assert_eq!(header.msg_type, protocol::STREAM_ERROR);
    }

    #[tokio::test]
    async fn cancel_local_stream_uses_reset_stream_for_v3_proxy() {
        let hub = HubRouter::new(ControlPlaneClient::disabled());

        let (proxy_tx, mut proxy_rx) = bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        let proxy = Arc::new(ProxyConn::new(
            101,
            "node-v3-reset".to_string(),
            "Node V3 Reset".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            3,
        ));
        hub.register_proxy(proxy);

        let stream = hub
            .open_local_stream("node-v3-reset", &build_meta())
            .await
            .expect("open local stream");
        let _ = proxy_rx.try_recv().expect("headers frame");

        hub.cancel_local_stream(stream.id, "client dropped");

        let cancelled = proxy_rx.try_recv().expect("cancel frame");
        let cancelled_data = match cancelled {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let header = protocol::FrameHeader::parse(&cancelled_data).expect("cancel frame header");
        assert_eq!(header.msg_type, protocol::RESET_STREAM);
        let payload = protocol::decode_payload(&cancelled_data, &header).expect("payload");
        let payload: protocol::ResetStreamPayload =
            serde_json::from_slice(&payload).expect("reset payload");
        assert_eq!(payload.reason, "client dropped");
    }

    #[tokio::test]
    async fn window_update_unblocks_v3_request_body_flow_control() {
        let hub = HubRouter::new(ControlPlaneClient::disabled());

        let (proxy_tx, mut proxy_rx) = bounded_queue(512);
        let (proxy_close_tx, _) = watch::channel(false);
        let proxy = Arc::new(ProxyConn::new(
            102,
            "node-v3-window".to_string(),
            "Node V3 Window".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            3,
        ));
        hub.register_proxy(proxy);

        let stream = hub
            .open_local_stream("node-v3-window", &build_meta())
            .await
            .expect("open local stream");
        let _ = proxy_rx.try_recv().expect("headers frame");
        let stream_id = stream.id;
        let proxy_stream_id = stream.proxy_stream_id;
        let payload = Bytes::from(vec![
            b'x';
            (*super::STREAM_INITIAL_WINDOW_BYTES as usize) + 17
        ]);

        let push_task = tokio::spawn({
            let hub = Arc::clone(&hub);
            async move { hub.push_local_request_body(stream_id, payload, true).await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        assert!(
            !push_task.is_finished(),
            "request body should wait after exhausting the initial stream window"
        );

        let mut window_update = protocol::encode_window_update(proxy_stream_id, 17);
        hub.handle_proxy_frame(102, &mut window_update).await;

        push_task
            .await
            .expect("push task should join")
            .expect("window update should let body send complete");
    }

    #[tokio::test]
    async fn push_local_request_body_splits_large_payload_and_marks_end() {
        let hub = HubRouter::new(ControlPlaneClient::disabled());

        let (proxy_tx, mut proxy_rx) = bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        let proxy = Arc::new(ProxyConn::new(
            200,
            "node-2".to_string(),
            "Node 2".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        ));
        hub.register_proxy(proxy);

        let stream = hub
            .open_local_stream("node-2", &build_meta())
            .await
            .expect("open local stream");
        let _ = proxy_rx.try_recv().expect("headers frame");

        let payload = Bytes::from(vec![b'x'; MAX_REQUEST_BODY_FRAME_SIZE + 17]);
        hub.push_local_request_body(stream.id, payload, true)
            .await
            .expect("push request body");

        let first = match proxy_rx.try_recv().expect("first body frame") {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let first_header = protocol::FrameHeader::parse(&first).expect("first body header");
        assert_eq!(first_header.msg_type, protocol::REQUEST_BODY);
        assert_eq!(first_header.flags & protocol::FLAG_GZIP_COMPRESSED, 0);
        assert_eq!(first_header.flags & protocol::FLAG_END_STREAM, 0);

        let second = match proxy_rx.try_recv().expect("second body frame") {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let second_header = protocol::FrameHeader::parse(&second).expect("second body header");
        assert_eq!(second_header.msg_type, protocol::REQUEST_BODY);
        assert_eq!(second_header.flags & protocol::FLAG_GZIP_COMPRESSED, 0);
        assert_ne!(second_header.flags & protocol::FLAG_END_STREAM, 0);
    }

    #[tokio::test]
    async fn goaway_marks_connection_draining_and_reroutes_new_streams() {
        let hub = HubRouter::new(ControlPlaneClient::disabled());

        let (proxy_one_tx, mut proxy_one_rx) = bounded_queue(8);
        let (proxy_one_close_tx, _) = watch::channel(false);
        let proxy_one = Arc::new(ProxyConn::new(
            201,
            "node-drain".to_string(),
            "Node Drain".to_string(),
            proxy_one_tx,
            proxy_one_close_tx,
            16,
            2,
        ));
        hub.register_proxy(Arc::clone(&proxy_one));

        let (proxy_two_tx, mut proxy_two_rx) = bounded_queue(8);
        let (proxy_two_close_tx, _) = watch::channel(false);
        let proxy_two = Arc::new(ProxyConn::new(
            202,
            "node-drain".to_string(),
            "Node Drain".to_string(),
            proxy_two_tx,
            proxy_two_close_tx,
            16,
            2,
        ));
        hub.register_proxy(Arc::clone(&proxy_two));

        let mut goaway = protocol::encode_goaway();
        hub.handle_proxy_frame(201, &mut goaway).await;
        assert!(
            proxy_one.is_draining(),
            "first connection should be draining"
        );
        assert!(
            !proxy_two.is_draining(),
            "second connection should remain schedulable"
        );

        let _stream = hub
            .open_local_stream("node-drain", &build_meta())
            .await
            .expect("open local stream");
        assert!(
            proxy_one_rx.try_recv().is_err(),
            "draining connection should not receive new streams"
        );
        let routed = proxy_two_rx
            .try_recv()
            .expect("headers should route to second connection");
        let routed_data = match routed {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let header = protocol::FrameHeader::parse(&routed_data).expect("frame header");
        assert_eq!(header.msg_type, protocol::REQUEST_HEADERS);
    }

    #[tokio::test]
    async fn heartbeat_callback_failure_does_not_send_fake_ack() {
        let hub = HubRouter::new(ControlPlaneClient::local(
            |_connection, _payload| Box::pin(async { Err("db unavailable".to_string()) }),
            |_connection, _connected, _conn_count, _observed_at_unix_secs| {
                Box::pin(async { Ok(()) })
            },
        ));

        let (proxy_tx, mut proxy_rx) = bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        let proxy = Arc::new(ProxyConn::new(
            300,
            "node-3".to_string(),
            "Node 3".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        ));
        hub.register_proxy(proxy);

        let payload = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-3",
            "heartbeat_id": 99u64,
        }))
        .expect("payload should serialize");
        let mut frame = protocol::encode_frame(1, protocol::HEARTBEAT_DATA, 0, &payload);
        hub.handle_proxy_frame(300, &mut frame).await;

        assert!(proxy_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn heartbeat_for_another_node_is_rejected_before_callback_and_ack() {
        let callback_calls = Arc::new(AtomicUsize::new(0));
        let callback_calls_for_heartbeat = Arc::clone(&callback_calls);
        let hub = HubRouter::new(ControlPlaneClient::local(
            move |_connection, _payload| {
                callback_calls_for_heartbeat.fetch_add(1, Ordering::Relaxed);
                Box::pin(async { Ok(br#"{"heartbeat_id":99}"#.to_vec()) })
            },
            |_connection, _connected, _conn_count, _observed_at_unix_secs| {
                Box::pin(async { Ok(()) })
            },
        ));

        let (proxy_tx, mut proxy_rx) = bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        let proxy = Arc::new(ProxyConn::new(
            301,
            "authenticated-node".to_string(),
            "Authenticated Node".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        ));
        hub.register_proxy(proxy);

        let payload = serde_json::to_vec(&serde_json::json!({
            "node_id": "victim-node",
            "heartbeat_id": 99u64,
        }))
        .expect("payload should serialize");
        let mut frame = protocol::encode_frame(1, protocol::HEARTBEAT_DATA, 0, &payload);
        hub.handle_proxy_frame(301, &mut frame).await;

        assert_eq!(callback_calls.load(Ordering::Relaxed), 0);
        assert!(proxy_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn second_stream_works_after_first_completes_via_stream_end() {
        let hub = HubRouter::new(ControlPlaneClient::disabled());

        let (proxy_tx, mut proxy_rx) = bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        let proxy = Arc::new(ProxyConn::new(
            400,
            "node-reuse".to_string(),
            "Node Reuse".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        ));
        hub.register_proxy(Arc::clone(&proxy));

        // First request: open stream, send body, simulate proxy response + STREAM_END
        let stream1 = hub
            .open_local_stream("node-reuse", &build_meta())
            .await
            .expect("open first stream");
        let _ = proxy_rx.try_recv().expect("first headers frame");
        hub.push_local_request_body(stream1.id, Bytes::new(), true)
            .await
            .expect("first body");
        let _ = proxy_rx.try_recv().expect("first body frame");

        // Simulate proxy sending RESPONSE_HEADERS
        let resp_meta = serde_json::to_vec(&serde_json::json!({
            "status": 200,
            "headers": []
        }))
        .unwrap();
        let mut resp_headers_frame = protocol::encode_frame(
            // Extract the proxy_stream_id from the request headers frame
            2, // first stream_id allocated
            protocol::RESPONSE_HEADERS,
            0,
            &resp_meta,
        );
        hub.handle_proxy_frame(400, &mut resp_headers_frame).await;

        // Simulate proxy sending STREAM_END
        let mut end_frame = protocol::encode_frame(2, protocol::STREAM_END, 0, &[]);
        hub.handle_proxy_frame(400, &mut end_frame).await;

        // Verify stream_count went back to 0
        assert_eq!(
            proxy
                .stream_count
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "stream_count should be 0 after STREAM_END"
        );

        // Second request: should work
        let stream2 = hub
            .open_local_stream("node-reuse", &build_meta())
            .await
            .expect("open second stream should succeed");
        let second_headers = proxy_rx.try_recv().expect("second headers frame");
        let second_data = match second_headers {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let header =
            protocol::FrameHeader::parse(&second_data).expect("second request header frame");
        assert_eq!(header.msg_type, protocol::REQUEST_HEADERS);
        assert_ne!(
            header.stream_id, 2,
            "second stream should have different stream_id"
        );

        // Simulate proxy response for second stream
        let mut resp2_headers =
            protocol::encode_frame(header.stream_id, protocol::RESPONSE_HEADERS, 0, &resp_meta);
        hub.handle_proxy_frame(400, &mut resp2_headers).await;

        let response = stream2
            .wait_headers(std::time::Duration::from_secs(1))
            .await
            .expect("second stream should receive headers");
        assert_eq!(response.status, 200);
    }
}
