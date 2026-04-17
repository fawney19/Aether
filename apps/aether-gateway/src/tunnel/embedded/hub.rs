use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_runtime::{BoundedQueueSender, MetricKind, MetricSample, QueueSendError, QueueSnapshot};
use axum::extract::ws::Message;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;
use tokio::sync::{watch, Notify};
use tracing::{debug, info, warn};

use super::control_plane::ControlPlaneClient;
use super::protocol;

const MAX_REQUEST_BODY_FRAME_SIZE: usize = 32 * 1024;
const SOFT_AVOID_QUEUE_PRESSURE_PERCENT: u64 = 50;
const SOFT_AVOID_STREAM_PRESSURE_PERCENT: u64 = 85;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStatus {
    Queued,
    Closed,
    Congested,
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
            Err(QueueSendError::Full(_)) => {
                self.mark_closing();
                SendStatus::Congested
            }
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
}

pub struct ProxyConn {
    pub id: u64,
    pub node_id: String,
    pub node_name: String,
    pub outbound: BoundedOutbound,
    next_stream_id: AtomicU32,
    pub stream_count: AtomicUsize,
    pub max_streams: usize,
    draining: AtomicBool,
    congested_total: AtomicU64,
}

impl ProxyConn {
    pub fn new(
        id: u64,
        node_id: String,
        node_name: String,
        tx: BoundedQueueSender<Message>,
        close_tx: watch::Sender<bool>,
        max_streams: usize,
    ) -> Self {
        Self {
            id,
            node_id,
            node_name,
            outbound: BoundedOutbound::new(tx, close_tx),
            next_stream_id: AtomicU32::new(2),
            stream_count: AtomicUsize::new(0),
            max_streams,
            draining: AtomicBool::new(false),
            congested_total: AtomicU64::new(0),
        }
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
                "proxy outbound queue full, closing congested connection"
            );
        }
        status
    }

    fn snapshot(&self) -> ProxyConnSnapshot {
        let outbound = self.outbound.snapshot();
        let stream_count = self.stream_count.load(Ordering::Relaxed);
        let queue_pressure_percent = percent_u64(outbound.depth, outbound.capacity);
        let stream_pressure_percent = percent_u64(stream_count, self.max_streams);
        let soft_avoid = queue_pressure_percent >= SOFT_AVOID_QUEUE_PRESSURE_PERCENT
            || stream_pressure_percent >= SOFT_AVOID_STREAM_PRESSURE_PERCENT;
        ProxyConnSnapshot {
            conn_id: self.id,
            available: self.is_available(),
            closing: self.outbound.is_closing(),
            draining: self.is_draining(),
            stream_count,
            max_streams: self.max_streams,
            stream_pressure_percent,
            outbound,
            queue_pressure_percent,
            soft_avoid,
            congested_total: self.congested_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProxyConnSnapshot {
    conn_id: u64,
    available: bool,
    closing: bool,
    draining: bool,
    stream_count: usize,
    max_streams: usize,
    stream_pressure_percent: u64,
    outbound: QueueSnapshot,
    queue_pressure_percent: u64,
    soft_avoid: bool,
    congested_total: u64,
}

#[derive(Clone)]
struct ProxyConnCandidate {
    conn: Arc<ProxyConn>,
    snapshot: ProxyConnSnapshot,
}

impl ProxyConnCandidate {
    fn rank_key(&self) -> (u8, u64, u64, usize, usize, u64) {
        (
            u8::from(self.snapshot.soft_avoid),
            self.snapshot.queue_pressure_percent,
            self.snapshot.stream_pressure_percent,
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

#[derive(Debug)]
pub enum LocalBodyEvent {
    Chunk(Bytes),
    End,
    Error(String),
}

#[derive(Debug, Default)]
struct LocalWaitState {
    response: Option<LocalResponseHead>,
    error: Option<String>,
}

pub struct LocalStream {
    pub id: u64,
    proxy_conn_id: u64,
    proxy_stream_id: u32,
    wait_state: Mutex<LocalWaitState>,
    headers_notify: Notify,
    body_tx: mpsc::Sender<LocalBodyEvent>,
    body_rx: Mutex<Option<mpsc::Receiver<LocalBodyEvent>>>,
    terminal: AtomicBool,
}

impl LocalStream {
    fn new(id: u64, proxy_conn_id: u64, proxy_stream_id: u32) -> Self {
        let (body_tx, body_rx) = mpsc::channel(128);
        Self {
            id,
            proxy_conn_id,
            proxy_stream_id,
            wait_state: Mutex::new(LocalWaitState::default()),
            headers_notify: Notify::new(),
            body_tx,
            body_rx: Mutex::new(Some(body_rx)),
            terminal: AtomicBool::new(false),
        }
    }

    pub async fn wait_headers(&self, timeout: Duration) -> Result<LocalResponseHead, String> {
        tokio::time::timeout(timeout, async {
            loop {
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
                self.headers_notify.notified().await;
            }
        })
        .await
        .map_err(|_| "timed out waiting for response headers".to_string())?
    }

    pub fn take_body_receiver(&self) -> Option<mpsc::Receiver<LocalBodyEvent>> {
        self.body_rx.lock().take()
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

    async fn push_body_chunk(&self, payload: Bytes) -> bool {
        if self.terminal.load(Ordering::Acquire) {
            return false;
        }
        // Use a timeout to prevent a slow consumer from blocking the shared
        // proxy-connection reader (head-of-line blocking across streams).
        match tokio::time::timeout(
            Duration::from_secs(5),
            self.body_tx.send(LocalBodyEvent::Chunk(payload)),
        )
        .await
        {
            Ok(Ok(())) => true,
            _ => false,
        }
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
        let _ = self.body_tx.try_send(LocalBodyEvent::End);
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
        let _ = self.body_tx.try_send(LocalBodyEvent::Error(error));
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
    node_status_tx: mpsc::UnboundedSender<NodeStatusEvent>,
    soft_avoid_selection_total: AtomicU64,
    selection_retry_total: AtomicU64,
    selection_unavailable_total: AtomicU64,
}

#[derive(Debug)]
struct NodeStatusEvent {
    node_id: String,
    connected: bool,
    conn_count: usize,
    observed_at_unix_secs: u64,
}

impl HubRouter {
    pub fn new(control_plane: ControlPlaneClient) -> Arc<Self> {
        let (node_status_tx, mut node_status_rx) = mpsc::unbounded_channel::<NodeStatusEvent>();
        let worker_control_plane = control_plane.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                while let Some(event) = node_status_rx.recv().await {
                    if let Err(error) = worker_control_plane
                        .push_node_status(
                            &event.node_id,
                            event.connected,
                            event.conn_count,
                            event.observed_at_unix_secs,
                        )
                        .await
                    {
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

        let pool_size = {
            let mut map = self.proxy_conns.write();
            map.entry(node_id.clone()).or_default().push(conn);
            map.get(&node_id).map(|v| v.len()).unwrap_or(0)
        };

        info!(
            node_id = %node_id,
            node_name = %node_name,
            conn_id = conn_id,
            pool_size = pool_size,
            "proxy connected"
        );

        self.notify_node_status(node_id, true, pool_size);
    }

    pub fn unregister_proxy(&self, conn_id: u64, node_id: &str) {
        self.proxy_conns_by_id.remove(&conn_id);

        let pool_size = {
            let mut map = self.proxy_conns.write();
            if let Some(conns) = map.get_mut(node_id) {
                conns.retain(|c| c.id != conn_id);
                if conns.is_empty() {
                    map.remove(node_id);
                }
            }
            map.get(node_id).map(|v| v.len()).unwrap_or(0)
        };

        info!(
            node_id = %node_id,
            conn_id = conn_id,
            remaining = pool_size,
            "proxy disconnected"
        );

        self.cancel_streams_for_proxy(conn_id);
        self.notify_node_status(node_id.to_string(), pool_size > 0, pool_size);
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

    fn notify_node_status(&self, node_id: String, connected: bool, conn_count: usize) {
        let event = NodeStatusEvent {
            node_id,
            connected,
            conn_count,
            observed_at_unix_secs: current_unix_secs(),
        };
        if let Err(error) = self.node_status_tx.send(event) {
            warn!(
                node_id = %error.0.node_id,
                connected = error.0.connected,
                conn_count = error.0.conn_count,
                observed_at_unix_secs = error.0.observed_at_unix_secs,
                "node status worker unavailable"
            );
        }
    }

    fn ranked_proxy_conn_candidates(&self, node_id: &str) -> Vec<ProxyConnCandidate> {
        let conns = {
            let map = self.proxy_conns.read();
            map.get(node_id)
                .map(|entries| entries.to_vec())
                .unwrap_or_default()
        };
        let mut candidates = conns
            .into_iter()
            .filter_map(|conn| {
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
        !self.ranked_proxy_conn_candidates(node_id).is_empty()
    }

    pub fn open_local_stream(
        &self,
        node_id: &str,
        meta: &protocol::RequestMeta,
    ) -> Result<Arc<LocalStream>, String> {
        let candidates = self.ranked_proxy_conn_candidates(node_id);
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
        let local_stream = Arc::new(LocalStream::new(
            local_stream_id,
            proxy_conn.id,
            proxy_stream_id,
        ));
        self.local_streams
            .insert(local_stream_id, local_stream.clone());
        self.proxy_to_local
            .insert((proxy_conn.id, proxy_stream_id), local_stream_id);

        let send_status = proxy_conn.send(Message::Binary(header_frame.into()));
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
            SendStatus::Queued => Ok(local_stream),
            SendStatus::Closed | SendStatus::Congested => {
                self.cleanup_local_stream(local_stream_id);
                proxy_conn.release_stream();
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

    pub fn push_local_request_body(
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

        let total_chunks = payload.len().div_ceil(MAX_REQUEST_BODY_FRAME_SIZE);
        if total_chunks == 0 {
            if end_stream {
                self.send_request_body_frame(&proxy_conn, stream.proxy_stream_id, &[], true)?;
            }
        } else {
            for (index, chunk) in payload.chunks(MAX_REQUEST_BODY_FRAME_SIZE).enumerate() {
                let is_last_chunk = index + 1 == total_chunks;
                self.send_request_body_frame(
                    &proxy_conn,
                    stream.proxy_stream_id,
                    chunk,
                    end_stream && is_last_chunk,
                )?;
            }
        }

        Ok(())
    }

    fn send_request_body_frame(
        &self,
        proxy_conn: &Arc<ProxyConn>,
        proxy_stream_id: u32,
        payload: &[u8],
        end_stream: bool,
    ) -> Result<(), String> {
        let (body_payload, body_flags) = protocol::compress_payload(payload)
            .map_err(|e| format!("failed to compress request body: {e}"))?;
        let body_frame = protocol::encode_frame(
            proxy_stream_id,
            protocol::REQUEST_BODY,
            body_flags
                | if end_stream {
                    protocol::FLAG_END_STREAM
                } else {
                    0
                },
            &body_payload,
        );
        match proxy_conn.send(Message::Binary(body_frame.into())) {
            SendStatus::Queued => Ok(()),
            SendStatus::Closed | SendStatus::Congested => {
                Err("proxy connection congested".to_string())
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
            let frame = protocol::encode_stream_error(stream.proxy_stream_id, reason);
            let _ = pc.send(Message::Binary(frame.into()));
        }
        stream.fail(reason.to_string());
    }

    fn cleanup_local_stream(&self, local_stream_id: u64) {
        let Some((_, stream)) = self.local_streams.remove(&local_stream_id) else {
            return;
        };
        self.proxy_to_local
            .remove(&(stream.proxy_conn_id, stream.proxy_stream_id));
    }

    pub async fn handle_proxy_frame(&self, proxy_conn_id: u64, data: &mut [u8]) {
        let header = match protocol::FrameHeader::parse(data) {
            Some(h) => h,
            None => return,
        };
        let expected_len = protocol::HEADER_SIZE + header.payload_len as usize;
        if data.len() < expected_len {
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
                let message = protocol::decode_payload(data, &header)
                    .ok()
                    .and_then(|payload| String::from_utf8(payload).ok())
                    .unwrap_or_else(|| "stream error".to_string());
                self.fail_proxy_stream(proxy_conn_id, header.stream_id, message);
            }
            protocol::HEARTBEAT_DATA => {
                self.handle_heartbeat(proxy_conn_id, header.stream_id, data, &header)
                    .await;
            }
            protocol::PING => {
                let payload = protocol::frame_payload_by_header(data, &header).unwrap_or(&[]);
                let pong = protocol::encode_pong(payload);
                if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
                    let _ = pc.send(Message::Binary(pong.into()));
                }
            }
            protocol::PONG => {}
            protocol::GOAWAY => {
                if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
                    let first = pc.mark_draining();
                    if first {
                        warn!(
                            proxy_conn_id = proxy_conn_id,
                            node_id = %pc.node_id,
                            "received GOAWAY from proxy connection; marking connection draining"
                        );
                    }
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

    fn route_response_headers(
        &self,
        proxy_conn_id: u64,
        header: protocol::FrameHeader,
        data: &[u8],
    ) {
        let Some(local_id) = self.lookup_local_stream(proxy_conn_id, header.stream_id) else {
            return;
        };
        let Ok(payload) = protocol::decode_payload(data, &header) else {
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
        let Ok(payload) = protocol::decode_payload(data, &header) else {
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

        if !stream.push_body_chunk(Bytes::from(payload)).await {
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
        let payload = match protocol::decode_payload(data, header) {
            Ok(payload) => payload,
            Err(error) => {
                warn!(proxy_conn_id = proxy_conn_id, error = %error, "failed to decode heartbeat payload");
                return;
            }
        };
        let ack_payload = match self.control_plane.heartbeat_ack(&payload).await {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    proxy_conn_id = proxy_conn_id,
                    error = %error,
                    "control-plane heartbeat callback failed; keeping heartbeat pending"
                );
                return;
            }
        };
        if let Some(pc) = self.proxy_conns_by_id.get(&proxy_conn_id) {
            let frame = protocol::encode_frame(stream_id, protocol::HEARTBEAT_ACK, 0, &ack_payload);
            let _ = pc.send(Message::Binary(frame.into()));
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

        HubStats {
            proxy_connections: total_proxy,
            available_proxy_connections,
            closing_proxy_connections,
            draining_proxy_connections,
            soft_avoid_proxy_connections,
            nodes,
            active_streams: self.local_streams.len(),
            outbound_queue_depth_total,
            outbound_queue_depth_max,
            outbound_queue_capacity_total,
            outbound_queue_rejected_full_total,
            outbound_queue_rejected_closed_total,
            proxy_connection_congested_total,
            soft_avoid_selection_total: self.soft_avoid_selection_total.load(Ordering::Relaxed),
            selection_retry_total: self.selection_retry_total.load(Ordering::Relaxed),
            selection_unavailable_total: self.selection_unavailable_total.load(Ordering::Relaxed),
        }
    }
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

#[derive(serde::Serialize)]
pub struct HubStats {
    pub proxy_connections: usize,
    pub available_proxy_connections: usize,
    pub closing_proxy_connections: usize,
    pub draining_proxy_connections: usize,
    pub soft_avoid_proxy_connections: usize,
    pub nodes: usize,
    pub active_streams: usize,
    pub outbound_queue_depth_total: usize,
    pub outbound_queue_depth_max: usize,
    pub outbound_queue_capacity_total: usize,
    pub outbound_queue_rejected_full_total: u64,
    pub outbound_queue_rejected_closed_total: u64,
    pub proxy_connection_congested_total: u64,
    pub soft_avoid_selection_total: u64,
    pub selection_retry_total: u64,
    pub selection_unavailable_total: u64,
}

impl HubStats {
    pub fn to_metric_samples(&self) -> Vec<MetricSample> {
        vec![
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
        ]
    }
}

#[cfg(test)]
mod tests {
    use aether_runtime::bounded_queue;

    use super::{protocol, ControlPlaneClient, HubRouter, ProxyConn, MAX_REQUEST_BODY_FRAME_SIZE};
    use axum::extract::ws::Message;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::watch;

    fn build_meta() -> protocol::RequestMeta {
        protocol::RequestMeta {
            method: "GET".to_string(),
            url: "https://example.com".to_string(),
            headers: HashMap::new(),
            timeout: 30,
            follow_redirects: None,
            http1_only: false,
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
        ));
        hub.register_proxy(proxy);

        let stream = hub
            .open_local_stream("node-1", &build_meta())
            .expect("open local stream");
        let _ = proxy_rx.try_recv().expect("headers frame");
        hub.push_local_request_body(stream.id, Bytes::new(), true)
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
        ));
        hub.register_proxy(proxy);

        let stream = hub
            .open_local_stream("node-2", &build_meta())
            .expect("open local stream");
        let _ = proxy_rx.try_recv().expect("headers frame");

        let payload = Bytes::from(vec![b'x'; MAX_REQUEST_BODY_FRAME_SIZE + 17]);
        hub.push_local_request_body(stream.id, payload, true)
            .expect("push request body");

        let first = match proxy_rx.try_recv().expect("first body frame") {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let first_header = protocol::FrameHeader::parse(&first).expect("first body header");
        assert_eq!(first_header.msg_type, protocol::REQUEST_BODY);
        assert_eq!(first_header.flags & protocol::FLAG_END_STREAM, 0);

        let second = match proxy_rx.try_recv().expect("second body frame") {
            Message::Binary(data) => data.to_vec(),
            other => panic!("unexpected message: {other:?}"),
        };
        let second_header = protocol::FrameHeader::parse(&second).expect("second body header");
        assert_eq!(second_header.msg_type, protocol::REQUEST_BODY);
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
            |_payload| Box::pin(async { Err("db unavailable".to_string()) }),
            |_node_id, _connected, _conn_count, _observed_at_unix_secs| Box::pin(async { Ok(()) }),
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
        ));
        hub.register_proxy(Arc::clone(&proxy));

        // First request: open stream, send body, simulate proxy response + STREAM_END
        let stream1 = hub
            .open_local_stream("node-reuse", &build_meta())
            .expect("open first stream");
        let _ = proxy_rx.try_recv().expect("first headers frame");
        hub.push_local_request_body(stream1.id, Bytes::new(), true)
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
