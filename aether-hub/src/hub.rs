use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::Message;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{watch, Notify};
use tracing::{debug, info, warn};

use crate::control_plane::ControlPlaneClient;
use crate::protocol;

const MAX_REQUEST_BODY_FRAME_SIZE: usize = 32 * 1024;

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
    tx: mpsc::Sender<Message>,
    close_tx: watch::Sender<bool>,
    closing: AtomicBool,
}

impl BoundedOutbound {
    pub fn new(tx: mpsc::Sender<Message>, close_tx: watch::Sender<bool>) -> Self {
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
            Err(TrySendError::Closed(_)) => {
                self.mark_closing();
                SendStatus::Closed
            }
            Err(TrySendError::Full(_)) => {
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
}

pub struct ProxyConn {
    pub id: u64,
    pub node_id: String,
    pub node_name: String,
    pub outbound: BoundedOutbound,
    next_stream_id: AtomicU32,
    pub stream_count: AtomicUsize,
    pub max_streams: usize,
}

impl ProxyConn {
    pub fn new(
        id: u64,
        node_id: String,
        node_name: String,
        tx: mpsc::Sender<Message>,
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
        !self.outbound.is_closing()
    }

    pub fn request_close(&self) {
        self.outbound.mark_closing();
    }

    pub fn send(&self, msg: Message) -> SendStatus {
        let was_closing = self.outbound.is_closing();
        let status = self.outbound.send(msg);
        if status == SendStatus::Congested && !was_closing {
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

    fn push_body_chunk(&self, payload: Bytes) -> bool {
        if self.terminal.load(Ordering::Acquire) {
            return false;
        }
        self.body_tx
            .try_send(LocalBodyEvent::Chunk(payload))
            .is_ok()
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
}

impl HubRouter {
    pub fn new(control_plane: ControlPlaneClient) -> Arc<Self> {
        Arc::new(Self {
            proxy_conns: RwLock::new(HashMap::new()),
            proxy_conns_by_id: DashMap::new(),
            local_streams: DashMap::new(),
            proxy_to_local: DashMap::new(),
            next_conn_id: AtomicU64::new(1),
            next_local_stream_id: AtomicU64::new(1),
            control_plane,
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

    fn notify_node_status(&self, node_id: String, connected: bool, conn_count: usize) {
        let control_plane = self.control_plane.clone();
        tokio::spawn(async move {
            if let Err(error) = control_plane
                .push_node_status(&node_id, connected, conn_count)
                .await
            {
                warn!(
                    node_id = %node_id,
                    connected = connected,
                    conn_count = conn_count,
                    error = %error,
                    "failed to push node status to app control plane"
                );
            }
        });
    }

    fn get_proxy_conn(&self, node_id: &str) -> Option<Arc<ProxyConn>> {
        let map = self.proxy_conns.read();
        let conns = map.get(node_id)?;
        conns
            .iter()
            .filter(|c| c.is_available())
            .min_by_key(|c| c.stream_count.load(Ordering::Relaxed))
            .cloned()
    }

    pub fn open_local_stream(
        &self,
        node_id: &str,
        meta: &protocol::RequestMeta,
    ) -> Result<Arc<LocalStream>, String> {
        let proxy_conn = self
            .get_proxy_conn(node_id)
            .ok_or_else(|| format!("no proxy connection for node {node_id}"))?;
        let proxy_stream_id = proxy_conn
            .alloc_stream_id()
            .ok_or_else(|| format!("stream limit reached for node {node_id}"))?;

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

        match proxy_conn.send(Message::Binary(header_frame.into())) {
            SendStatus::Queued => Ok(local_stream),
            SendStatus::Closed | SendStatus::Congested => {
                self.cleanup_local_stream(local_stream_id);
                proxy_conn.release_stream();
                Err("proxy connection congested".to_string())
            }
        }
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
                self.route_response_body(proxy_conn_id, header, data);
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
                warn!(
                    proxy_conn_id = proxy_conn_id,
                    "received GOAWAY from proxy connection"
                );
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

    fn route_response_body(&self, proxy_conn_id: u64, header: protocol::FrameHeader, data: &[u8]) {
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
                warn!(proxy_conn_id = proxy_conn_id, error = %error, "control-plane heartbeat callback failed");
                b"{}".to_vec()
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
        let proxy_conns = self.proxy_conns.read();
        let total_proxy = proxy_conns.values().map(|v| v.len()).sum();
        let nodes = proxy_conns.len();
        drop(proxy_conns);

        HubStats {
            proxy_connections: total_proxy,
            nodes,
            active_streams: self.local_streams.len(),
        }
    }
}

#[derive(serde::Serialize)]
pub struct HubStats {
    pub proxy_connections: usize,
    pub nodes: usize,
    pub active_streams: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_meta() -> protocol::RequestMeta {
        protocol::RequestMeta {
            method: "GET".to_string(),
            url: "https://example.com".to_string(),
            headers: HashMap::new(),
            timeout: 30,
        }
    }

    #[tokio::test]
    async fn cancel_local_stream_notifies_proxy() {
        let hub = HubRouter::new(ControlPlaneClient::disabled());

        let (proxy_tx, mut proxy_rx) = mpsc::channel(8);
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

        let (proxy_tx, mut proxy_rx) = mpsc::channel(8);
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
}
