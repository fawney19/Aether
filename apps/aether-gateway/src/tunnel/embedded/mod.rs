mod body;
mod control_plane;
mod hub;
mod local_relay;
pub mod protocol;
mod proxy_conn;

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use aether_gateway_tunnel::{
    resolve_proxy_max_streams, resolve_proxy_node_name, resolve_proxy_protocol_version,
};
use aether_runtime::{
    hold_admission_permit_until, prometheus_response, service_up_sample, AdmissionPermit,
    ConcurrencyError, ConcurrencyGate, ConcurrencySnapshot, MetricKind, MetricLabel, MetricSample,
};
use aether_runtime_state::{
    MemoryRuntimeStateConfig, RuntimeSemaphore, RuntimeSemaphoreError, RuntimeSemaphoreSnapshot,
    RuntimeState,
};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use dashmap::DashMap;
use sha2::{Digest as _, Sha256};
use tracing::warn;

use crate::error::{redact_error_debug, redact_error_detail};
use crate::{data::GatewayDataState, middleware};

pub use control_plane::ControlPlaneClient;
pub use hub::{ConnConfig, HubRouter, LocalBodyEvent, ProxyConn};
pub use local_relay::relay_request;
pub(crate) use local_relay::{open_direct_relay_stream, DirectRelayResponse};

const RELAY_AUTH_CLOCK_SKEW_SECS: u64 = 60;
const MAX_RELAY_AUTH_ID_LEN: usize = 200;
const MAX_RELAY_AUTH_NONCE_LEN: usize = 128;
const TUNNEL_SECURITY_PROOF_CLOCK_SKEW_SECS: u64 = 60;
const MAX_TUNNEL_SECURITY_NODE_ID_LEN: usize = 200;
const MAX_TUNNEL_SECURITY_SESSION_LEN: usize = 128;
const MAX_TUNNEL_SECURITY_PROOF_NONCE_LEN: usize = 128;
const MAX_TUNNEL_SECURITY_PROOF_SIGNATURE_LEN: usize = 128;
const CONTROL_PLANE_AUTH_CLOCK_SKEW_SECS: u64 = 60;
const MAX_CONTROL_PLANE_AUTH_NODE_ID_LEN: usize = 200;
const MAX_CONTROL_PLANE_AUTH_NONCE_LEN: usize = 128;
const MAX_CONTROL_PLANE_AUTH_SIGNATURE_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelayAuthError {
    Unavailable,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControlPlaneAuthError {
    Unavailable,
    Invalid,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RelayRequestAuthenticated;

pub(crate) struct PendingRelayAuth {
    pub(crate) payload_digest: aether_contracts::tunnel::TunnelRelayPayloadDigest,
    nonce: String,
    sender: String,
    expires_at_unix_secs: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<HubRouter>,
    pub proxy_conn_cfg: ConnConfig,
    pub max_streams: usize,
    data: Arc<GatewayDataState>,
    request_gate: Option<Arc<ConcurrencyGate>>,
    distributed_request_gate: Option<Arc<RuntimeSemaphore>>,
    secure_tunnel_keys: Arc<DashMap<String, String>>,
    relay_instance_id: Arc<str>,
    relay_auth_secret: Option<Arc<[u8]>>,
    relay_auth_runtime_state: Arc<RuntimeState>,
}

#[derive(Debug)]
enum RequestAdmissionError {
    Local(ConcurrencyError),
    Distributed(RuntimeSemaphoreError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProxyTunnelSecurityError {
    MissingKey,
    InvalidKey,
    MissingMode,
    UnsupportedMode,
    MissingSession,
    MissingProof,
    MalformedHeader,
    InvalidProof,
    Replay,
    MissingAuthorization,
    InvalidAuthorization,
    ManagementTokenUnavailable,
    Unavailable,
}

impl ProxyTunnelSecurityError {
    fn status_code(self) -> axum::http::StatusCode {
        match self {
            Self::MissingKey
            | Self::MissingMode
            | Self::MissingSession
            | Self::MissingProof
            | Self::InvalidProof
            | Self::Replay
            | Self::MissingAuthorization
            | Self::InvalidAuthorization => axum::http::StatusCode::UNAUTHORIZED,
            Self::UnsupportedMode | Self::MalformedHeader => axum::http::StatusCode::BAD_REQUEST,
            Self::InvalidKey | Self::ManagementTokenUnavailable | Self::Unavailable => {
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            }
        }
    }
}

impl AppState {
    pub fn new(
        control_plane: ControlPlaneClient,
        proxy_conn_cfg: ConnConfig,
        max_streams: usize,
    ) -> Self {
        Self {
            hub: HubRouter::new(control_plane),
            proxy_conn_cfg,
            max_streams,
            data: Arc::new(GatewayDataState::disabled()),
            request_gate: None,
            distributed_request_gate: None,
            secure_tunnel_keys: Arc::new(DashMap::new()),
            relay_instance_id: Arc::from("standalone"),
            relay_auth_secret: None,
            relay_auth_runtime_state: Arc::new(RuntimeState::memory(
                MemoryRuntimeStateConfig::default(),
            )),
        }
    }

    pub fn with_relay_auth(
        mut self,
        instance_id: impl Into<String>,
        secret: Option<impl Into<Vec<u8>>>,
        runtime_state: Arc<RuntimeState>,
    ) -> Self {
        self.relay_instance_id = Arc::from(instance_id.into());
        self.relay_auth_secret = secret
            .map(Into::into)
            .filter(|value: &Vec<u8>| value.len() >= 32)
            .map(Arc::from);
        self.relay_auth_runtime_state = runtime_state;
        self
    }

    async fn claim_relay_auth_nonce(
        &self,
        nonce: &str,
        sender: &str,
        now_unix_secs: u64,
        expires_at_unix_secs: u64,
    ) -> Result<bool, RelayAuthError> {
        let ttl = std::time::Duration::from_secs(
            expires_at_unix_secs
                .saturating_sub(now_unix_secs)
                .saturating_add(1)
                .max(1),
        );
        self.relay_auth_runtime_state
            .kv_set_if_absent(
                &format!("tunnel:relay:auth:nonce:{nonce}"),
                sender.to_string(),
                ttl,
            )
            .await
            .map_err(|_| RelayAuthError::Unavailable)
    }

    pub(crate) async fn authenticate_relay_request_headers(
        &self,
        headers: &HeaderMap,
        node_id: &str,
        require_local_owner: bool,
    ) -> Result<PendingRelayAuth, RelayAuthError> {
        let secret = self
            .relay_auth_secret
            .as_deref()
            .ok_or(RelayAuthError::Unavailable)?;
        let sender = relay_auth_header(
            headers,
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_SENDER_HEADER,
            MAX_RELAY_AUTH_ID_LEN,
        )?;
        let owner = relay_auth_header(
            headers,
            aether_contracts::tunnel::TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
            MAX_RELAY_AUTH_ID_LEN,
        )?;
        if require_local_owner && owner != self.relay_instance_id.as_ref() {
            return Err(RelayAuthError::Invalid);
        }
        let nonce = relay_auth_header(
            headers,
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_NONCE_HEADER,
            MAX_RELAY_AUTH_NONCE_LEN,
        )?;
        if !nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(RelayAuthError::Invalid);
        }
        let signature = relay_auth_header(
            headers,
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_SIGNATURE_HEADER,
            128,
        )?;
        let payload_digest = relay_auth_header(
            headers,
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
            96,
        )?;
        let payload_digest =
            aether_contracts::tunnel::TunnelRelayPayloadDigest::decode_header_value(payload_digest)
                .ok_or(RelayAuthError::Invalid)?;
        let timestamp = relay_auth_header(
            headers,
            aether_contracts::tunnel::TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
            20,
        )?
        .parse::<u64>()
        .map_err(|_| RelayAuthError::Invalid)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_err(|_| RelayAuthError::Unavailable)?
            .as_secs();
        if now.abs_diff(timestamp) > RELAY_AUTH_CLOCK_SKEW_SECS {
            return Err(RelayAuthError::Invalid);
        }

        let forwarded_by = relay_auth_optional_header(
            headers,
            aether_contracts::tunnel::TUNNEL_RELAY_FORWARDED_BY_HEADER,
            MAX_RELAY_AUTH_ID_LEN,
        )?
        .unwrap_or_default();
        if !forwarded_by.is_empty() && forwarded_by != sender {
            return Err(RelayAuthError::Invalid);
        }
        let rollout_probe = match relay_auth_optional_header(
            headers,
            crate::tunnel::TUNNEL_RELAY_ROLLOUT_PROBE_HEADER,
            1,
        )? {
            Some(crate::tunnel::TUNNEL_RELAY_ROLLOUT_PROBE_VALUE) => true,
            Some(_) => return Err(RelayAuthError::Invalid),
            None => false,
        };
        if rollout_probe && forwarded_by.is_empty() {
            return Err(RelayAuthError::Invalid);
        }
        if !aether_contracts::tunnel::verify_tunnel_relay_request_signature(
            secret,
            sender,
            owner,
            node_id,
            forwarded_by,
            rollout_probe,
            timestamp,
            nonce,
            &payload_digest,
            signature,
        ) {
            return Err(RelayAuthError::Invalid);
        }
        Ok(PendingRelayAuth {
            payload_digest,
            nonce: nonce.to_string(),
            sender: sender.to_string(),
            expires_at_unix_secs: timestamp.saturating_add(RELAY_AUTH_CLOCK_SKEW_SECS),
        })
    }

    pub(crate) async fn commit_relay_auth(
        &self,
        pending: &PendingRelayAuth,
    ) -> Result<(), RelayAuthError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_err(|_| RelayAuthError::Unavailable)?
            .as_secs();
        if now > pending.expires_at_unix_secs {
            return Err(RelayAuthError::Invalid);
        }
        if !self
            .claim_relay_auth_nonce(
                &pending.nonce,
                &pending.sender,
                now,
                pending.expires_at_unix_secs,
            )
            .await?
        {
            return Err(RelayAuthError::Invalid);
        }
        Ok(())
    }

    pub(crate) async fn authenticate_relay_request(
        &self,
        headers: &HeaderMap,
        node_id: &str,
        payload_digest: &aether_contracts::tunnel::TunnelRelayPayloadDigest,
        require_local_owner: bool,
    ) -> Result<(), RelayAuthError> {
        let pending = self
            .authenticate_relay_request_headers(headers, node_id, require_local_owner)
            .await?;
        if pending.payload_digest != *payload_digest {
            return Err(RelayAuthError::Invalid);
        }
        self.commit_relay_auth(&pending).await
    }

    pub(crate) fn register_secure_tunnel_key(
        &self,
        node_id: impl Into<String>,
        key: impl Into<String>,
    ) {
        self.secure_tunnel_keys.insert(node_id.into(), key.into());
    }

    pub(crate) fn secure_tunnel_key(&self, node_id: &str) -> Option<String> {
        self.secure_tunnel_keys
            .get(node_id)
            .map(|entry| entry.value().clone())
    }

    async fn secure_tunnel_binding_for_node(
        &self,
        node_id: &str,
    ) -> Result<Option<(String, String)>, aether_data::DataLayerError> {
        if !self.data.has_proxy_node_reader() {
            return Ok(None);
        }
        let binding =
            crate::state::decrypt_or_migrate_proxy_tunnel_psk_binding(self.data.as_ref(), node_id)
                .await?;
        if let Some(binding) = binding.as_ref() {
            self.register_secure_tunnel_key(node_id.to_string(), binding.key.clone());
        } else {
            self.secure_tunnel_keys.remove(node_id);
        }
        Ok(binding.map(|binding| (binding.key, binding.tunnel_generation)))
    }

    async fn secure_tunnel_binding_for_handshake(
        &self,
        node_id: &str,
        _requested_generation: &str,
    ) -> Result<Option<(String, String)>, aether_data::DataLayerError> {
        if !self.data.has_proxy_node_reader() {
            return Err(aether_data::DataLayerError::InvalidConfiguration(
                "proxy node reader is unavailable for tunnel authentication".to_string(),
            ));
        }
        self.secure_tunnel_binding_for_node(node_id).await
    }

    async fn authenticate_proxy_tunnel_management_token(
        &self,
        headers: &HeaderMap,
        node_id: &str,
        remote_ip: IpAddr,
    ) -> Result<(hub::ProxyManagementTokenCredential, String), ProxyTunnelSecurityError> {
        let authenticated = crate::management_token_auth::authenticate_management_token(
            self.data.as_ref(),
            headers,
            remote_ip,
        )
        .await
        .map_err(|error| match error {
            crate::management_token_auth::ManagementTokenAuthError::Missing => {
                ProxyTunnelSecurityError::MissingAuthorization
            }
            crate::management_token_auth::ManagementTokenAuthError::Invalid => {
                ProxyTunnelSecurityError::InvalidAuthorization
            }
            crate::management_token_auth::ManagementTokenAuthError::Unavailable => {
                ProxyTunnelSecurityError::ManagementTokenUnavailable
            }
        })?;
        if !crate::roles::can_write_admin_console(&authenticated.user.role) {
            return Err(ProxyTunnelSecurityError::InvalidAuthorization);
        }

        let Some(node) = self
            .data
            .find_proxy_node(node_id)
            .await
            .map_err(|_| ProxyTunnelSecurityError::ManagementTokenUnavailable)?
        else {
            return Err(ProxyTunnelSecurityError::InvalidAuthorization);
        };
        if !node.tunnel_mode {
            return Err(ProxyTunnelSecurityError::InvalidAuthorization);
        }

        if !management_token_may_connect_proxy_tunnel(&authenticated.permissions) {
            return Err(ProxyTunnelSecurityError::InvalidAuthorization);
        }

        if let Err(error) = self
            .data
            .record_management_token_usage(&authenticated.token.id, Some(&remote_ip.to_string()))
            .await
        {
            warn!(
                token_id = %authenticated.token.id,
                error = %error,
                "failed to record proxy tunnel management token usage"
            );
        }
        Ok((
            hub::ProxyManagementTokenCredential {
                verified_token_hash: authenticated.verified_token_hash,
                token_id: authenticated.token.id,
                user_id: authenticated.user.id,
                remote_ip,
            },
            node.tunnel_generation,
        ))
    }

    async fn authorized_proxy_connections_for_new_stream(
        &self,
        node_id: &str,
    ) -> Result<Option<HashSet<u64>>, String> {
        if !self.data.has_proxy_node_reader() {
            return Err(control_plane::CONTROL_PLANE_CREDENTIAL_UNAVAILABLE.to_string());
        }

        let connections = self
            .hub
            .proxy_connections_for_node(node_id)
            .into_iter()
            .filter(|connection| connection.is_available())
            .collect::<Vec<_>>();
        if connections.is_empty() {
            return Ok(Some(HashSet::new()));
        }

        let mut authorized = HashSet::new();
        let mut validation_unavailable = false;
        for connection in connections {
            match validate_proxy_connection_credential(self.data.as_ref(), &connection).await {
                Ok(()) => {
                    authorized.insert(connection.id);
                }
                Err(error) if error == control_plane::CONTROL_PLANE_CREDENTIAL_UNAVAILABLE => {
                    validation_unavailable = true;
                }
                Err(_) => {
                    warn!(
                        node_id = %node_id,
                        conn_id = connection.id,
                        "closing proxy connection after credential revocation"
                    );
                    self.hub.request_close_proxy(connection.id);
                }
            }
        }

        // Keep the node existence/mode read after credential validation. If a node is
        // deleted while a token lookup is in flight, that lookup must not authorize a
        // stream against the deleted node.
        let node = self
            .data
            .find_proxy_node(node_id)
            .await
            .map_err(|_| "proxy tunnel credential validation unavailable".to_string())?;
        if !node.is_some_and(|node| node.id == node_id && node.tunnel_mode) {
            self.hub.request_close_proxies_for_node(node_id);
            return Err("proxy tunnel credential was revoked".to_string());
        }

        if !authorized.is_empty() {
            return Ok(Some(authorized));
        }
        if validation_unavailable {
            return Err("proxy tunnel credential validation unavailable".to_string());
        }
        Err("proxy tunnel credential was revoked".to_string())
    }

    pub(crate) async fn open_authorized_local_stream(
        &self,
        node_id: &str,
        meta: &protocol::RequestMeta,
    ) -> Result<Arc<hub::LocalStream>, String> {
        let authorized = self
            .authorized_proxy_connections_for_new_stream(node_id)
            .await?;
        self.hub
            .open_local_stream_with_authorized_connections(node_id, meta, authorized.as_ref())
            .await
    }

    async fn authenticate_proxy_tunnel_security(
        &self,
        headers: &HeaderMap,
        node_id: &str,
        tunnel_generation: &str,
        protocol_version: u8,
        stored_security_key: Option<String>,
    ) -> Result<(String, String), ProxyTunnelSecurityError> {
        let tunnel_security = required_proxy_tunnel_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_SECURITY_HEADER,
            32,
            ProxyTunnelSecurityError::MissingMode,
        )?;
        let security_session = required_proxy_tunnel_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_SECURITY_SESSION_HEADER,
            MAX_TUNNEL_SECURITY_SESSION_LEN,
            ProxyTunnelSecurityError::MissingSession,
        )?;
        if !valid_proxy_tunnel_token(security_session, 16) {
            return Err(ProxyTunnelSecurityError::MalformedHeader);
        }
        let (security_key, security_session) = resolve_proxy_tunnel_security(
            stored_security_key,
            Some(tunnel_security),
            Some(security_session.to_string()),
        )?;

        let timestamp = required_proxy_tunnel_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER,
            20,
            ProxyTunnelSecurityError::MissingProof,
        )?;
        if !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ProxyTunnelSecurityError::MalformedHeader);
        }
        let timestamp = timestamp
            .parse::<u64>()
            .map_err(|_| ProxyTunnelSecurityError::MalformedHeader)?;
        let nonce = required_proxy_tunnel_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_SECURITY_PROOF_NONCE_HEADER,
            MAX_TUNNEL_SECURITY_PROOF_NONCE_LEN,
            ProxyTunnelSecurityError::MissingProof,
        )?;
        if !valid_proxy_tunnel_token(nonce, 16) {
            return Err(ProxyTunnelSecurityError::MalformedHeader);
        }
        let signature = required_proxy_tunnel_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER,
            MAX_TUNNEL_SECURITY_PROOF_SIGNATURE_LEN,
            ProxyTunnelSecurityError::MissingProof,
        )?;
        if signature.len() != 43
            || !signature
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ProxyTunnelSecurityError::MalformedHeader);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_err(|_| ProxyTunnelSecurityError::Unavailable)?
            .as_secs();
        if now.abs_diff(timestamp) > TUNNEL_SECURITY_PROOF_CLOCK_SKEW_SECS {
            return Err(ProxyTunnelSecurityError::InvalidProof);
        }
        if !aether_contracts::tunnel_security::verify_tunnel_security_handshake_for_generation(
            &security_key,
            node_id,
            tunnel_generation,
            tunnel_security,
            &security_session,
            protocol_version,
            timestamp,
            nonce,
            signature,
        ) {
            return Err(ProxyTunnelSecurityError::InvalidProof);
        }

        let ttl = std::time::Duration::from_secs(
            timestamp
                .saturating_add(TUNNEL_SECURITY_PROOF_CLOCK_SKEW_SECS)
                .saturating_sub(now)
                .saturating_add(1)
                .max(1),
        );
        let claimed = self
            .relay_auth_runtime_state
            .kv_set_if_absent(
                &format!("tunnel:security:proof:nonce:{node_id}:{tunnel_generation}:{nonce}"),
                security_session.clone(),
                ttl,
            )
            .await
            .map_err(|_| ProxyTunnelSecurityError::Unavailable)?;
        if !claimed {
            return Err(ProxyTunnelSecurityError::Replay);
        }
        Ok((security_key, security_session))
    }

    pub(crate) async fn authenticate_control_plane_request(
        &self,
        headers: &HeaderMap,
        method: &str,
        path: &str,
        payload_node_id: &str,
        body: &[u8],
    ) -> Result<String, ControlPlaneAuthError> {
        if !self.data.has_proxy_node_reader() {
            return Err(ControlPlaneAuthError::Unavailable);
        }
        let authenticated_node_id = control_plane_auth_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_CONTROL_PLANE_NODE_ID_HEADER,
            MAX_CONTROL_PLANE_AUTH_NODE_ID_LEN,
        )?;
        if authenticated_node_id != payload_node_id {
            return Err(ControlPlaneAuthError::Invalid);
        }
        let authenticated_generation = control_plane_auth_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_CONTROL_PLANE_GENERATION_HEADER,
            128,
        )?;
        if !valid_proxy_tunnel_token(authenticated_generation, 1) {
            return Err(ControlPlaneAuthError::Invalid);
        }
        let timestamp = control_plane_auth_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_CONTROL_PLANE_TIMESTAMP_HEADER,
            20,
        )?
        .parse::<u64>()
        .map_err(|_| ControlPlaneAuthError::Invalid)?;
        let nonce = control_plane_auth_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_CONTROL_PLANE_NONCE_HEADER,
            MAX_CONTROL_PLANE_AUTH_NONCE_LEN,
        )?;
        if nonce.len() < 16
            || !nonce
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ControlPlaneAuthError::Invalid);
        }
        let signature = control_plane_auth_header(
            headers,
            aether_contracts::tunnel_security::TUNNEL_CONTROL_PLANE_SIGNATURE_HEADER,
            MAX_CONTROL_PLANE_AUTH_SIGNATURE_LEN,
        )?;
        if signature.len() != 43
            || !signature
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ControlPlaneAuthError::Invalid);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map_err(|_| ControlPlaneAuthError::Unavailable)?
            .as_secs();
        if now.abs_diff(timestamp) > CONTROL_PLANE_AUTH_CLOCK_SKEW_SECS {
            return Err(ControlPlaneAuthError::Invalid);
        }
        let (security_key, current_generation) = self
            .secure_tunnel_binding_for_node(payload_node_id)
            .await
            .map_err(|_| ControlPlaneAuthError::Unavailable)?
            .ok_or(ControlPlaneAuthError::Invalid)?;
        if current_generation != authenticated_generation {
            return Err(ControlPlaneAuthError::Invalid);
        }
        if !aether_contracts::tunnel_security::verify_tunnel_control_plane_request_for_generation(
            &security_key,
            method,
            path,
            authenticated_node_id,
            authenticated_generation,
            timestamp,
            nonce,
            body,
            signature,
        ) {
            return Err(ControlPlaneAuthError::Invalid);
        }

        let ttl = std::time::Duration::from_secs(
            timestamp
                .saturating_add(CONTROL_PLANE_AUTH_CLOCK_SKEW_SECS)
                .saturating_sub(now)
                .saturating_add(1)
                .max(1),
        );
        let node_digest = Sha256::digest(
            format!("{authenticated_node_id}\0{authenticated_generation}").as_bytes(),
        );
        let claimed = self
            .relay_auth_runtime_state
            .kv_set_if_absent(
                &format!("tunnel:control-plane:auth:nonce:{node_digest:x}:{nonce}"),
                timestamp.to_string(),
                ttl,
            )
            .await
            .map_err(|_| ControlPlaneAuthError::Unavailable)?;
        if !claimed {
            return Err(ControlPlaneAuthError::Invalid);
        }
        Ok(current_generation)
    }

    pub(crate) fn with_data(mut self, data: Arc<GatewayDataState>) -> Self {
        self.data = data;
        self
    }

    pub fn with_request_concurrency_limit(mut self, limit: Option<usize>) -> Self {
        self.request_gate = limit
            .filter(|limit| *limit > 0)
            .map(|limit| Arc::new(ConcurrencyGate::new("tunnel_requests", limit)));
        self
    }

    pub fn with_distributed_request_gate(mut self, gate: RuntimeSemaphore) -> Self {
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
        let mut samples = vec![service_up_sample("aether-tunnel-standalone")];
        if let Some(snapshot) = self.request_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples("tunnel_requests"));
        }
        if let Some(gate) = self.distributed_request_gate.as_ref() {
            match gate.snapshot().await {
                Ok(snapshot) => {
                    samples.extend(snapshot.to_metric_samples("tunnel_requests_distributed"));
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
                        "tunnel_requests_distributed",
                    )]),
                ),
            }
        }
        samples.extend(self.hub.stats().to_metric_samples());
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

fn management_token_may_connect_proxy_tunnel(permissions: &[String]) -> bool {
    permissions
        .iter()
        .any(|permission| permission == "admin:proxy_nodes:admin")
}

fn relay_auth_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    max_len: usize,
) -> Result<&'a str, RelayAuthError> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or(RelayAuthError::Invalid)?;
    if values.next().is_some() {
        return Err(RelayAuthError::Invalid);
    }
    let value = value.to_str().map_err(|_| RelayAuthError::Invalid)?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max_len || trimmed != value {
        return Err(RelayAuthError::Invalid);
    }
    Ok(trimmed)
}

fn control_plane_auth_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    max_len: usize,
) -> Result<&'a str, ControlPlaneAuthError> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or(ControlPlaneAuthError::Invalid)?;
    if values.next().is_some() {
        return Err(ControlPlaneAuthError::Invalid);
    }
    let value = value.to_str().map_err(|_| ControlPlaneAuthError::Invalid)?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max_len || trimmed != value {
        return Err(ControlPlaneAuthError::Invalid);
    }
    Ok(trimmed)
}

fn relay_auth_optional_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    max_len: usize,
) -> Result<Option<&'a str>, RelayAuthError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(RelayAuthError::Invalid);
    }
    let value = value.to_str().map_err(|_| RelayAuthError::Invalid)?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max_len || trimmed != value {
        return Err(RelayAuthError::Invalid);
    }
    Ok(Some(trimmed))
}

fn required_proxy_tunnel_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    max_len: usize,
    missing_error: ProxyTunnelSecurityError,
) -> Result<&'a str, ProxyTunnelSecurityError> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or(missing_error)?;
    if values.next().is_some() {
        return Err(ProxyTunnelSecurityError::MalformedHeader);
    }
    let value = value
        .to_str()
        .map_err(|_| ProxyTunnelSecurityError::MalformedHeader)?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max_len || trimmed != value {
        return Err(ProxyTunnelSecurityError::MalformedHeader);
    }
    Ok(trimmed)
}

fn valid_proxy_tunnel_token(value: &str, min_len: usize) -> bool {
    value.len() >= min_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn constant_time_secret_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

pub(crate) async fn validate_proxy_connection_credential(
    data: &GatewayDataState,
    connection: &hub::ProxyConn,
) -> Result<(), &'static str> {
    if !data.has_proxy_node_reader() {
        return Err(control_plane::CONTROL_PLANE_CREDENTIAL_UNAVAILABLE);
    }
    let node = data
        .find_proxy_node(&connection.node_id)
        .await
        .map_err(|_| control_plane::CONTROL_PLANE_CREDENTIAL_UNAVAILABLE)?;
    let Some(node) = node.filter(|node| node.id == connection.node_id && node.tunnel_mode) else {
        return Err(control_plane::CONTROL_PLANE_CREDENTIAL_REVOKED);
    };
    if connection.node_generation.is_empty() || connection.node_generation != node.tunnel_generation
    {
        return Err(control_plane::CONTROL_PLANE_CREDENTIAL_REVOKED);
    }

    match connection.credential_binding() {
        Some(hub::ProxyCredentialBinding::Psk(authenticated_key)) => {
            let current_key = crate::state::decrypt_or_migrate_proxy_tunnel_psk(data, &node.id)
                .await
                .map_err(|_| control_plane::CONTROL_PLANE_CREDENTIAL_UNAVAILABLE)?;
            if current_key
                .as_deref()
                .is_some_and(|current| constant_time_secret_eq(current, &authenticated_key))
            {
                Ok(())
            } else {
                Err(control_plane::CONTROL_PLANE_CREDENTIAL_REVOKED)
            }
        }
        Some(hub::ProxyCredentialBinding::ManagementToken(credential)) => {
            let authenticated = crate::management_token_auth::authenticate_management_token_hash(
                data,
                &credential.verified_token_hash,
                credential.remote_ip,
            )
            .await
            .map_err(|error| match error {
                crate::management_token_auth::ManagementTokenAuthError::Unavailable => {
                    control_plane::CONTROL_PLANE_CREDENTIAL_UNAVAILABLE
                }
                crate::management_token_auth::ManagementTokenAuthError::Invalid
                | crate::management_token_auth::ManagementTokenAuthError::Missing => {
                    control_plane::CONTROL_PLANE_CREDENTIAL_REVOKED
                }
            })?;
            if authenticated.token.id == credential.token_id
                && authenticated.token.user_id == credential.user_id
                && authenticated.user.id == credential.user_id
                && crate::roles::can_write_admin_console(&authenticated.user.role)
                && management_token_may_connect_proxy_tunnel(&authenticated.permissions)
            {
                Ok(())
            } else {
                Err(control_plane::CONTROL_PLANE_CREDENTIAL_REVOKED)
            }
        }
        None => Err(control_plane::CONTROL_PLANE_CREDENTIAL_REVOKED),
    }
}

fn resolve_proxy_tunnel_security(
    stored_security_key: Option<String>,
    tunnel_security: Option<&str>,
    security_session: Option<String>,
) -> Result<(String, String), ProxyTunnelSecurityError> {
    let key = stored_security_key.ok_or(ProxyTunnelSecurityError::MissingKey)?;
    aether_contracts::tunnel_security::decode_psk(&key)
        .map_err(|_| ProxyTunnelSecurityError::InvalidKey)?;

    match tunnel_security {
        Some(aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED) => {}
        Some(_) => return Err(ProxyTunnelSecurityError::UnsupportedMode),
        None => return Err(ProxyTunnelSecurityError::MissingMode),
    }

    let session = security_session.ok_or(ProxyTunnelSecurityError::MissingSession)?;
    Ok((key, session))
}

#[cfg(test)]
mod relay_auth_tests {
    use super::{AppState, ConnConfig, ControlPlaneClient, RelayAuthError};
    use aether_contracts::tunnel::{
        sign_tunnel_relay_request, tunnel_relay_payload_digest, TUNNEL_RELAY_AUTH_NONCE_HEADER,
        TUNNEL_RELAY_AUTH_PAYLOAD_HEADER, TUNNEL_RELAY_AUTH_SENDER_HEADER,
        TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
        TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
    };
    use axum::http::{HeaderMap, HeaderValue};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const SECRET: &[u8] = b"relay-auth-test-secret-at-least-32-bytes";

    fn state() -> AppState {
        AppState::new(
            ControlPlaneClient::disabled(),
            ConnConfig {
                ping_interval: Duration::from_secs(15),
                idle_timeout: Duration::ZERO,
                outbound_queue_capacity: 8,
            },
            8,
        )
        .with_relay_auth(
            "gateway-b",
            Some(SECRET.to_vec()),
            std::sync::Arc::new(aether_runtime_state::RuntimeState::memory(
                aether_runtime_state::MemoryRuntimeStateConfig::default(),
            )),
        )
    }

    fn signed_headers(
        owner: &str,
        node_id: &str,
        metadata: &[u8],
        body: &[u8],
        timestamp: u64,
        nonce: &str,
    ) -> HeaderMap {
        let sender = "gateway-a";
        let payload_digest = tunnel_relay_payload_digest(metadata, body);
        let signature = sign_tunnel_relay_request(
            SECRET,
            sender,
            owner,
            node_id,
            "",
            false,
            timestamp,
            nonce,
            &payload_digest,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            TUNNEL_RELAY_AUTH_SENDER_HEADER,
            HeaderValue::from_static(sender),
        );
        headers.insert(
            TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
            HeaderValue::from_str(owner).expect("owner header"),
        );
        headers.insert(
            TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
            HeaderValue::from_str(&timestamp.to_string()).expect("timestamp header"),
        );
        headers.insert(
            TUNNEL_RELAY_AUTH_NONCE_HEADER,
            HeaderValue::from_str(nonce).expect("nonce header"),
        );
        headers.insert(
            TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
            HeaderValue::from_str(&payload_digest.encode_header_value()).expect("payload header"),
        );
        headers.insert(
            TUNNEL_RELAY_AUTH_SIGNATURE_HEADER,
            HeaderValue::from_str(&signature).expect("signature header"),
        );
        headers
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_secs()
    }

    #[tokio::test]
    async fn accepts_valid_relay_authentication_once() {
        let state = state();
        let metadata = b"metadata-envelope";
        let body = b"request-body";
        let headers = signed_headers("gateway-b", "node-1", metadata, body, now(), "nonce-valid");

        assert_eq!(
            state
                .authenticate_relay_request(
                    &headers,
                    "node-1",
                    &tunnel_relay_payload_digest(metadata, body),
                    true,
                )
                .await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn rejects_metadata_tampering_and_owner_mismatch() {
        let state = state();
        let headers = signed_headers(
            "gateway-b",
            "node-1",
            b"metadata",
            b"body",
            now(),
            "nonce-tamper",
        );
        assert_eq!(
            state
                .authenticate_relay_request(
                    &headers,
                    "node-1",
                    &tunnel_relay_payload_digest(b"tampered", b"body"),
                    true,
                )
                .await,
            Err(RelayAuthError::Invalid)
        );

        let headers = signed_headers(
            "gateway-other",
            "node-1",
            b"metadata",
            b"body",
            now(),
            "nonce-owner",
        );
        assert_eq!(
            state
                .authenticate_relay_request(
                    &headers,
                    "node-1",
                    &tunnel_relay_payload_digest(b"metadata", b"body"),
                    true,
                )
                .await,
            Err(RelayAuthError::Invalid)
        );
    }

    #[tokio::test]
    async fn payload_tampering_does_not_consume_a_valid_nonce() {
        let state = state();
        let metadata = b"metadata";
        let body = b"original-body";
        let headers = signed_headers(
            "gateway-b",
            "node-1",
            metadata,
            body,
            now(),
            "nonce-body-preserve",
        );

        assert_eq!(
            state
                .authenticate_relay_request(
                    &headers,
                    "node-1",
                    &tunnel_relay_payload_digest(metadata, b"tampered-body"),
                    true,
                )
                .await,
            Err(RelayAuthError::Invalid)
        );
        assert_eq!(
            state
                .authenticate_relay_request(
                    &headers,
                    "node-1",
                    &tunnel_relay_payload_digest(metadata, body),
                    true,
                )
                .await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn rejects_replay_and_expired_timestamp() {
        let state = state();
        let payload_digest = tunnel_relay_payload_digest(b"metadata", b"body");
        let headers = signed_headers(
            "gateway-b",
            "node-1",
            b"metadata",
            b"body",
            now(),
            "nonce-replay",
        );
        assert_eq!(
            state
                .authenticate_relay_request(&headers, "node-1", &payload_digest, true)
                .await,
            Ok(())
        );
        assert_eq!(
            state
                .authenticate_relay_request(&headers, "node-1", &payload_digest, true)
                .await,
            Err(RelayAuthError::Invalid)
        );

        let expired = now().saturating_sub(super::RELAY_AUTH_CLOCK_SKEW_SECS + 1);
        let headers = signed_headers(
            "gateway-b",
            "node-1",
            b"metadata",
            b"body",
            expired,
            "nonce-expired",
        );
        assert_eq!(
            state
                .authenticate_relay_request(&headers, "node-1", &payload_digest, true)
                .await,
            Err(RelayAuthError::Invalid)
        );
    }

    #[tokio::test]
    async fn rejects_pending_relay_auth_that_expires_before_commit() {
        let state = state();
        let metadata = b"metadata";
        let body = b"body";
        let nonce = "nonce-expired-before-commit";
        let headers = signed_headers("gateway-b", "node-1", metadata, body, now(), nonce);
        let mut pending = state
            .authenticate_relay_request_headers(&headers, "node-1", true)
            .await
            .expect("fresh signature headers should authenticate");
        pending.expires_at_unix_secs = now().saturating_sub(1);

        assert_eq!(
            state.commit_relay_auth(&pending).await,
            Err(RelayAuthError::Invalid)
        );

        let fresh_headers = signed_headers("gateway-b", "node-1", metadata, body, now(), nonce);
        assert_eq!(
            state
                .authenticate_relay_request(
                    &fresh_headers,
                    "node-1",
                    &tunnel_relay_payload_digest(metadata, body),
                    true,
                )
                .await,
            Ok(()),
            "an expired pending request must not consume the nonce"
        );
    }
}

#[cfg(test)]
mod proxy_tunnel_security_tests {
    use super::{
        hub::ProxyManagementTokenCredential, management_token_may_connect_proxy_tunnel,
        resolve_proxy_tunnel_security, AppState, ConnConfig, ControlPlaneAuthError,
        ControlPlaneClient, ProxyConn, ProxyTunnelSecurityError,
        TUNNEL_SECURITY_PROOF_CLOCK_SKEW_SECS,
    };
    use aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION;
    use aether_contracts::tunnel_security::{
        sign_tunnel_control_plane_request_for_generation,
        sign_tunnel_security_handshake_for_generation, TUNNEL_CONTROL_PLANE_GENERATION_HEADER,
        TUNNEL_CONTROL_PLANE_NODE_ID_HEADER, TUNNEL_CONTROL_PLANE_NONCE_HEADER,
        TUNNEL_CONTROL_PLANE_SIGNATURE_HEADER, TUNNEL_CONTROL_PLANE_TIMESTAMP_HEADER,
        TUNNEL_SECURITY_HEADER, TUNNEL_SECURITY_NON_TLS_REQUIRED,
        TUNNEL_SECURITY_PROOF_NONCE_HEADER, TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER,
        TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER, TUNNEL_SECURITY_SESSION_HEADER,
    };
    use aether_data::repository::management_tokens::{
        CreateManagementTokenRecord, InMemoryManagementTokenRepository,
        ManagementTokenWriteRepository, RegenerateManagementTokenSecret, StoredManagementToken,
        StoredManagementTokenUserSummary, StoredManagementTokenWithUser,
    };
    use aether_data::repository::proxy_nodes::{
        InMemoryProxyNodeRepository, ProxyNodeHeartbeatMutation, ProxyNodeReadRepository,
        ProxyNodeRegistrationMutation, ProxyNodeTunnelStatusMutation, ProxyNodeWriteRepository,
        StoredProxyNode,
    };
    use aether_data::repository::users::{InMemoryUserReadRepository, StoredUserAuthRecord};
    use aether_runtime::{bounded_queue, BoundedQueueReceiver};
    use axum::extract::ws::Message;
    use axum::http::{HeaderMap, HeaderValue};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tokio::sync::watch;

    const VALID_PSK: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    const ROTATED_PSK: &str = "CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=";
    const NODE_ID: &str = "node-proof-1";
    const SESSION: &str = "0123456789abcdef0123456789abcdef";
    const REVOCATION_NODE_ID: &str = "node-revocation-1";
    const REVOCATION_TOKEN_ID: &str = "token-revocation-1";
    const REVOCATION_USER_ID: &str = "user-revocation-1";
    const RAW_MANAGEMENT_TOKEN: &str = "ae-tunnel-revocation-original";
    const ROTATED_MANAGEMENT_TOKEN: &str = "ae-tunnel-revocation-rotated";
    const TEST_TUNNEL_GENERATION: &str = "test-generation-1";

    fn state() -> AppState {
        AppState::new(
            ControlPlaneClient::disabled(),
            ConnConfig {
                ping_interval: Duration::from_secs(15),
                idle_timeout: Duration::ZERO,
                outbound_queue_capacity: 8,
            },
            8,
        )
    }

    struct RevocationFixture {
        state: AppState,
        data: Arc<crate::data::GatewayDataState>,
        token_repository: Arc<InMemoryManagementTokenRepository>,
        node_repository: Arc<InMemoryProxyNodeRepository>,
    }

    struct RegisteredTestProxy {
        connection: Arc<ProxyConn>,
        _outbound_rx: BoundedQueueReceiver<Message>,
        close_rx: watch::Receiver<bool>,
    }

    fn management_token_hash(raw_token: &str) -> String {
        format!("{:x}", Sha256::digest(raw_token.as_bytes()))
    }

    fn management_token_user_summary() -> StoredManagementTokenUserSummary {
        StoredManagementTokenUserSummary::new(
            REVOCATION_USER_ID.to_string(),
            Some("tunnel-revocation@example.com".to_string()),
            "tunnel_revocation_admin".to_string(),
            "admin".to_string(),
        )
        .expect("management token user summary should build")
    }

    fn management_token_with_user() -> StoredManagementTokenWithUser {
        let token = StoredManagementToken::new(
            REVOCATION_TOKEN_ID.to_string(),
            REVOCATION_USER_ID.to_string(),
            "tunnel revocation token".to_string(),
        )
        .expect("management token should build")
        .with_permissions(Some(json!(["admin:proxy_nodes:admin"])))
        .with_runtime_fields(None, None, None, 0, true);
        StoredManagementTokenWithUser::new(token, management_token_user_summary())
    }

    fn management_token_create_record(raw_token: &str) -> CreateManagementTokenRecord {
        CreateManagementTokenRecord {
            id: REVOCATION_TOKEN_ID.to_string(),
            user_id: REVOCATION_USER_ID.to_string(),
            user: management_token_user_summary(),
            token_hash: management_token_hash(raw_token),
            token_prefix: Some("ae-tunnel".to_string()),
            name: "tunnel revocation token".to_string(),
            description: None,
            allowed_ips: None,
            permissions: Some(json!(["admin:proxy_nodes:admin"])),
            expires_at_unix_secs: None,
            is_active: true,
        }
    }

    fn current_management_token_user() -> StoredUserAuthRecord {
        StoredUserAuthRecord::new(
            REVOCATION_USER_ID.to_string(),
            Some("tunnel-revocation@example.com".to_string()),
            true,
            "tunnel_revocation_admin".to_string(),
            None,
            "admin".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("current management token user should build")
    }

    fn tunnel_node(psk: Option<&str>) -> StoredProxyNode {
        StoredProxyNode::new(
            REVOCATION_NODE_ID.to_string(),
            "revocation test node".to_string(),
            "127.0.0.1".to_string(),
            0,
            false,
            "online".to_string(),
            30,
            0,
            0,
            0,
            0,
            0,
            true,
            true,
            1,
        )
        .expect("tunnel node should build")
        .with_runtime_fields(
            None,
            None,
            None,
            None,
            psk.map(|psk| {
                json!({
                    "tunnel_security": {
                        "mode": TUNNEL_SECURITY_NON_TLS_REQUIRED,
                        "encryption_key": psk,
                    }
                })
            }),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .with_tunnel_generation(TEST_TUNNEL_GENERATION.to_string())
    }

    fn revocation_fixture(psk: Option<&str>) -> RevocationFixture {
        let token_repository = Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
            vec![management_token_with_user()],
            vec![(
                management_token_hash(RAW_MANAGEMENT_TOKEN),
                REVOCATION_TOKEN_ID.to_string(),
            )],
        ));
        let node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![tunnel_node(psk)]));
        let data = Arc::new(
            crate::data::GatewayDataState::with_management_token_repository_for_tests(Arc::clone(
                &token_repository,
            ))
            .attach_proxy_node_repository_for_tests(Arc::clone(&node_repository))
            .with_user_reader(Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
                current_management_token_user(),
            ])))
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY),
        );
        let state = state().with_data(Arc::clone(&data));
        RevocationFixture {
            state,
            data,
            token_repository,
            node_repository,
        }
    }

    fn register_psk_connection(state: &AppState, psk: &str) -> RegisteredTestProxy {
        let (outbound_tx, outbound_rx) = bounded_queue(8);
        let (close_tx, close_rx) = watch::channel(false);
        let connection = Arc::new(
            ProxyConn::new(
                state.hub.alloc_conn_id(),
                REVOCATION_NODE_ID.to_string(),
                "revocation test node".to_string(),
                outbound_tx,
                close_tx,
                8,
                CURRENT_TUNNEL_PROTOCOL_VERSION,
            )
            .with_tunnel_generation(TEST_TUNNEL_GENERATION.to_string())
            .with_authenticated_key(psk.to_string()),
        );
        state.hub.register_proxy(Arc::clone(&connection));
        RegisteredTestProxy {
            connection,
            _outbound_rx: outbound_rx,
            close_rx,
        }
    }

    fn register_management_token_connection(
        state: &AppState,
        raw_token: &str,
    ) -> RegisteredTestProxy {
        let (outbound_tx, outbound_rx) = bounded_queue(8);
        let (close_tx, close_rx) = watch::channel(false);
        let connection = Arc::new(
            ProxyConn::new(
                state.hub.alloc_conn_id(),
                REVOCATION_NODE_ID.to_string(),
                "revocation test node".to_string(),
                outbound_tx,
                close_tx,
                8,
                CURRENT_TUNNEL_PROTOCOL_VERSION,
            )
            .with_tunnel_generation(TEST_TUNNEL_GENERATION.to_string())
            .with_management_token_credential(ProxyManagementTokenCredential {
                verified_token_hash: crate::management_token_auth::VerifiedManagementTokenHash::new(
                    management_token_hash(raw_token),
                ),
                token_id: REVOCATION_TOKEN_ID.to_string(),
                user_id: REVOCATION_USER_ID.to_string(),
                remote_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            }),
        );
        state.hub.register_proxy(Arc::clone(&connection));
        RegisteredTestProxy {
            connection,
            _outbound_rx: outbound_rx,
            close_rx,
        }
    }

    async fn assert_connection_authorized(state: &AppState, proxy: &RegisteredTestProxy) {
        let authorized = state
            .authorized_proxy_connections_for_new_stream(REVOCATION_NODE_ID)
            .await
            .expect("current connection credential should validate")
            .expect("repository-backed validation should return an authorization set");
        assert_eq!(authorized.len(), 1);
        assert!(authorized.contains(&proxy.connection.id));
        assert!(proxy.connection.is_available());
        assert!(!*proxy.close_rx.borrow());
    }

    async fn assert_connection_revoked(state: &AppState, proxy: &RegisteredTestProxy) {
        let error = state
            .authorized_proxy_connections_for_new_stream(REVOCATION_NODE_ID)
            .await
            .expect_err("revoked connection must not authorize a new logical stream");
        assert_eq!(error, "proxy tunnel credential was revoked");
        assert!(!proxy.connection.is_available());
        assert!(*proxy.close_rx.borrow());
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_secs()
    }

    fn proof_headers(timestamp: u64, nonce: &str) -> HeaderMap {
        let signature = sign_tunnel_security_handshake_for_generation(
            VALID_PSK,
            NODE_ID,
            TEST_TUNNEL_GENERATION,
            TUNNEL_SECURITY_NON_TLS_REQUIRED,
            SESSION,
            CURRENT_TUNNEL_PROTOCOL_VERSION,
            timestamp,
            nonce,
        )
        .expect("proof should sign");
        let mut headers = HeaderMap::new();
        headers.insert(
            TUNNEL_SECURITY_HEADER,
            HeaderValue::from_static(TUNNEL_SECURITY_NON_TLS_REQUIRED),
        );
        headers.insert(
            TUNNEL_SECURITY_SESSION_HEADER,
            HeaderValue::from_static(SESSION),
        );
        headers.insert(
            TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER,
            HeaderValue::from_str(&timestamp.to_string()).expect("timestamp header"),
        );
        headers.insert(
            TUNNEL_SECURITY_PROOF_NONCE_HEADER,
            HeaderValue::from_str(nonce).expect("nonce header"),
        );
        headers.insert(
            TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER,
            HeaderValue::from_str(&signature).expect("signature header"),
        );
        headers
    }

    #[test]
    fn proxy_tunnel_security_rejects_missing_registered_psk() {
        assert_eq!(
            resolve_proxy_tunnel_security(
                None,
                Some(TUNNEL_SECURITY_NON_TLS_REQUIRED),
                Some("session-1".to_string()),
            ),
            Err(ProxyTunnelSecurityError::MissingKey)
        );
        assert_eq!(
            resolve_proxy_tunnel_security(None, None, None),
            Err(ProxyTunnelSecurityError::MissingKey)
        );
    }

    #[test]
    fn proxy_tunnel_security_rejects_invalid_registered_psk() {
        assert_eq!(
            resolve_proxy_tunnel_security(
                Some("not-a-valid-32-byte-base64-key".to_string()),
                Some(TUNNEL_SECURITY_NON_TLS_REQUIRED),
                Some("session-1".to_string()),
            ),
            Err(ProxyTunnelSecurityError::InvalidKey)
        );
    }

    #[test]
    fn proxy_tunnel_security_requires_declared_supported_mode() {
        assert_eq!(
            resolve_proxy_tunnel_security(
                Some(VALID_PSK.to_string()),
                None,
                Some("session-1".to_string()),
            ),
            Err(ProxyTunnelSecurityError::MissingMode)
        );
        assert_eq!(
            resolve_proxy_tunnel_security(
                Some(VALID_PSK.to_string()),
                Some("unsupported"),
                Some("session-1".to_string()),
            ),
            Err(ProxyTunnelSecurityError::UnsupportedMode)
        );
    }

    #[test]
    fn proxy_tunnel_security_requires_session() {
        assert_eq!(
            resolve_proxy_tunnel_security(
                Some(VALID_PSK.to_string()),
                Some(TUNNEL_SECURITY_NON_TLS_REQUIRED),
                None,
            ),
            Err(ProxyTunnelSecurityError::MissingSession)
        );
    }

    #[test]
    fn proxy_tunnel_security_accepts_valid_psk_mode_and_session() {
        assert_eq!(
            resolve_proxy_tunnel_security(
                Some(VALID_PSK.to_string()),
                Some(TUNNEL_SECURITY_NON_TLS_REQUIRED),
                Some("session-1".to_string()),
            ),
            Ok((VALID_PSK.to_string(), "session-1".to_string()))
        );
    }

    #[test]
    fn proxy_tunnel_management_fallback_requires_admin_permission() {
        assert!(!management_token_may_connect_proxy_tunnel(&[
            "admin:proxy_nodes:write".to_string(),
        ]));
        assert!(management_token_may_connect_proxy_tunnel(&[
            "admin:proxy_nodes:admin".to_string(),
        ]));
    }

    #[tokio::test]
    async fn proxy_tunnel_security_accepts_valid_proof_once_and_rejects_replay() {
        let state = state();
        let headers = proof_headers(now(), "nonce-valid-proof-0001");

        assert_eq!(
            state
                .authenticate_proxy_tunnel_security(
                    &headers,
                    NODE_ID,
                    TEST_TUNNEL_GENERATION,
                    CURRENT_TUNNEL_PROTOCOL_VERSION,
                    Some(VALID_PSK.to_string()),
                )
                .await,
            Ok((VALID_PSK.to_string(), SESSION.to_string()))
        );
        assert_eq!(
            state
                .authenticate_proxy_tunnel_security(
                    &headers,
                    NODE_ID,
                    TEST_TUNNEL_GENERATION,
                    CURRENT_TUNNEL_PROTOCOL_VERSION,
                    Some(VALID_PSK.to_string()),
                )
                .await,
            Err(ProxyTunnelSecurityError::Replay)
        );
    }

    #[tokio::test]
    async fn proxy_tunnel_handshake_fails_closed_without_proxy_node_reader() {
        let state = state();
        state.register_secure_tunnel_key(NODE_ID, VALID_PSK);
        assert_eq!(state.secure_tunnel_key(NODE_ID).as_deref(), Some(VALID_PSK));

        let error = state
            .secure_tunnel_binding_for_handshake(NODE_ID, "attacker-selected-generation")
            .await
            .expect_err("cached PSK must not replace the authoritative node generation");
        assert!(matches!(
            error,
            aether_data::DataLayerError::InvalidConfiguration(_)
        ));
    }

    #[tokio::test]
    async fn existing_proxy_connection_cannot_open_stream_without_proxy_node_reader() {
        let state = state();
        let proxy = register_psk_connection(&state, VALID_PSK);

        let error = state
            .authorized_proxy_connections_for_new_stream(REVOCATION_NODE_ID)
            .await
            .expect_err("new streams require authoritative credential revalidation");
        assert_eq!(
            error,
            super::control_plane::CONTROL_PLANE_CREDENTIAL_UNAVAILABLE
        );
        assert!(proxy.connection.is_available());
    }

    #[tokio::test]
    async fn proxy_tunnel_security_rejects_tampering_expiry_and_duplicate_headers() {
        let state = state();
        let timestamp = now();
        let headers = proof_headers(timestamp, "nonce-tamper-proof-0001");
        assert_eq!(
            state
                .authenticate_proxy_tunnel_security(
                    &headers,
                    "node-proof-forged",
                    TEST_TUNNEL_GENERATION,
                    CURRENT_TUNNEL_PROTOCOL_VERSION,
                    Some(VALID_PSK.to_string()),
                )
                .await,
            Err(ProxyTunnelSecurityError::InvalidProof)
        );

        let expired = proof_headers(
            timestamp.saturating_sub(TUNNEL_SECURITY_PROOF_CLOCK_SKEW_SECS + 1),
            "nonce-expired-proof-01",
        );
        assert_eq!(
            state
                .authenticate_proxy_tunnel_security(
                    &expired,
                    NODE_ID,
                    TEST_TUNNEL_GENERATION,
                    CURRENT_TUNNEL_PROTOCOL_VERSION,
                    Some(VALID_PSK.to_string()),
                )
                .await,
            Err(ProxyTunnelSecurityError::InvalidProof)
        );

        let mut duplicate = proof_headers(timestamp, "nonce-duplicate-proof-1");
        duplicate.append(
            TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER,
            HeaderValue::from_static("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        );
        assert_eq!(
            state
                .authenticate_proxy_tunnel_security(
                    &duplicate,
                    NODE_ID,
                    TEST_TUNNEL_GENERATION,
                    CURRENT_TUNNEL_PROTOCOL_VERSION,
                    Some(VALID_PSK.to_string()),
                )
                .await,
            Err(ProxyTunnelSecurityError::MalformedHeader)
        );
    }

    #[tokio::test]
    async fn proxy_tunnel_security_requires_complete_proof_before_nonce_claim() {
        let state = state();
        let mut headers = proof_headers(now(), "nonce-missing-proof-001");
        headers.remove(TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER);

        assert_eq!(
            state
                .authenticate_proxy_tunnel_security(
                    &headers,
                    NODE_ID,
                    TEST_TUNNEL_GENERATION,
                    CURRENT_TUNNEL_PROTOCOL_VERSION,
                    Some(VALID_PSK.to_string()),
                )
                .await,
            Err(ProxyTunnelSecurityError::MissingProof)
        );
    }

    #[tokio::test]
    async fn control_plane_authentication_fails_closed_without_proxy_node_reader() {
        let state = state();
        state.register_secure_tunnel_key(NODE_ID, VALID_PSK);
        let body = br#"{"node_id":"node-proof-1"}"#;
        let timestamp = now();
        let nonce = "control-plane-no-reader-0001";
        let signature = sign_tunnel_control_plane_request_for_generation(
            VALID_PSK,
            "POST",
            aether_gateway_tunnel::TUNNEL_HEARTBEAT_PATH,
            NODE_ID,
            TEST_TUNNEL_GENERATION,
            timestamp,
            nonce,
            body,
        )
        .expect("control-plane request should sign");
        let mut headers = HeaderMap::new();
        headers.insert(
            TUNNEL_CONTROL_PLANE_NODE_ID_HEADER,
            HeaderValue::from_static(NODE_ID),
        );
        headers.insert(
            TUNNEL_CONTROL_PLANE_GENERATION_HEADER,
            HeaderValue::from_static(TEST_TUNNEL_GENERATION),
        );
        headers.insert(
            TUNNEL_CONTROL_PLANE_TIMESTAMP_HEADER,
            HeaderValue::from_str(&timestamp.to_string()).expect("timestamp header"),
        );
        headers.insert(
            TUNNEL_CONTROL_PLANE_NONCE_HEADER,
            HeaderValue::from_static(nonce),
        );
        headers.insert(
            TUNNEL_CONTROL_PLANE_SIGNATURE_HEADER,
            HeaderValue::from_str(&signature).expect("signature header"),
        );

        let result = state
            .authenticate_control_plane_request(
                &headers,
                "POST",
                aether_gateway_tunnel::TUNNEL_HEARTBEAT_PATH,
                NODE_ID,
                body,
            )
            .await;

        assert_eq!(result, Err(ControlPlaneAuthError::Unavailable));
    }

    #[tokio::test]
    async fn new_stream_revalidation_rejects_rotated_psk_without_local_close() {
        let fixture = revocation_fixture(Some(VALID_PSK));
        let proxy = register_psk_connection(&fixture.state, VALID_PSK);
        assert_connection_authorized(&fixture.state, &proxy).await;

        let node = fixture
            .node_repository
            .find_proxy_node(REVOCATION_NODE_ID)
            .await
            .expect("node lookup should succeed")
            .expect("node should exist");
        let previous = node
            .proxy_metadata
            .expect("validated PSK should remain in protected metadata");
        let replacement = json!({
            "tunnel_security": {
                "mode": TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "encryption_key": ROTATED_PSK,
            }
        });
        assert!(fixture
            .node_repository
            .compare_and_set_proxy_metadata(REVOCATION_NODE_ID, &previous, &replacement)
            .await
            .expect("PSK rotation should succeed"));

        assert_connection_revoked(&fixture.state, &proxy).await;
    }

    #[tokio::test]
    async fn new_stream_revalidation_rejects_deleted_node_without_local_close() {
        let fixture = revocation_fixture(Some(VALID_PSK));
        let proxy = register_psk_connection(&fixture.state, VALID_PSK);
        assert_connection_authorized(&fixture.state, &proxy).await;

        fixture
            .node_repository
            .delete_node(REVOCATION_NODE_ID)
            .await
            .expect("node deletion should succeed")
            .expect("node should exist before deletion");

        assert_connection_revoked(&fixture.state, &proxy).await;
    }

    #[tokio::test]
    async fn same_node_id_recreated_with_same_psk_does_not_rebind_old_connection() {
        let fixture = revocation_fixture(Some(VALID_PSK));
        let proxy = register_psk_connection(&fixture.state, VALID_PSK);
        assert_connection_authorized(&fixture.state, &proxy).await;
        let deleted_generation = proxy.connection.node_generation.clone();

        fixture
            .node_repository
            .delete_node(REVOCATION_NODE_ID)
            .await
            .expect("node deletion should succeed")
            .expect("node should exist before deletion");
        let replacement = fixture
            .node_repository
            .register_node(&ProxyNodeRegistrationMutation {
                node_id: Some(REVOCATION_NODE_ID.to_string()),
                name: "replacement node".to_string(),
                ip: "127.0.0.1".to_string(),
                port: 0,
                region: None,
                heartbeat_interval: 30,
                active_connections: None,
                total_requests: None,
                avg_latency_ms: None,
                hardware_info: None,
                estimated_max_concurrency: None,
                proxy_metadata: Some(json!({
                    "tunnel_security": {
                        "mode": TUNNEL_SECURITY_NON_TLS_REQUIRED,
                        "encryption_key": VALID_PSK,
                    }
                })),
                proxy_version: None,
                registered_by: None,
                tunnel_mode: true,
            })
            .await
            .expect("same-id replacement node should register");

        assert_eq!(replacement.id, REVOCATION_NODE_ID);
        assert_ne!(replacement.tunnel_generation, deleted_generation);
        assert_connection_revoked(&fixture.state, &proxy).await;

        let stale_heartbeat = fixture
            .node_repository
            .apply_heartbeat(&ProxyNodeHeartbeatMutation {
                node_id: REVOCATION_NODE_ID.to_string(),
                expected_tunnel_generation: Some(deleted_generation.clone()),
                heartbeat_interval: Some(1),
                active_connections: Some(99),
                total_requests_delta: Some(500),
                avg_latency_ms: Some(1.0),
                failed_requests_delta: Some(5),
                dns_failures_delta: Some(4),
                stream_errors_delta: Some(3),
                proxy_metadata: Some(json!({"forged": true})),
                proxy_version: Some("stale".to_string()),
            })
            .await
            .expect("stale heartbeat should be handled without repository failure");
        assert!(stale_heartbeat.is_none());
        let stale_status = fixture
            .node_repository
            .update_tunnel_status(&ProxyNodeTunnelStatusMutation {
                node_id: REVOCATION_NODE_ID.to_string(),
                expected_tunnel_generation: Some(deleted_generation),
                connected: true,
                conn_count: 99,
                detail: Some("stale".to_string()),
                observed_at_unix_secs: Some(now()),
            })
            .await
            .expect("stale status should be handled without repository failure");
        assert!(stale_status.is_none());
        let unchanged = fixture
            .node_repository
            .find_proxy_node(REVOCATION_NODE_ID)
            .await
            .expect("replacement lookup should succeed")
            .expect("replacement should remain");
        assert_eq!(unchanged.tunnel_generation, replacement.tunnel_generation);
        assert_eq!(unchanged.active_connections, 0);
        assert_eq!(unchanged.total_requests, 0);
        assert!(unchanged
            .proxy_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("forged"))
            .is_none());
    }

    #[tokio::test]
    async fn management_token_disable_on_another_gateway_revokes_existing_connection() {
        let fixture = revocation_fixture(None);
        let shared_data = Arc::clone(&fixture.data);
        let gateway_a = fixture.state;
        let gateway_b = state().with_data(shared_data);
        let proxy = register_management_token_connection(&gateway_b, RAW_MANAGEMENT_TOKEN);
        assert_connection_authorized(&gateway_b, &proxy).await;

        let outcome = gateway_a
            .data
            .set_management_token_active(REVOCATION_TOKEN_ID, false)
            .await
            .expect("management token disable should succeed");
        assert!(outcome.is_some());

        // gateway_b never receives an in-process close request. Its next-stream
        // strong read of the shared repository must still observe the revocation.
        assert_connection_revoked(&gateway_b, &proxy).await;
    }

    #[tokio::test]
    async fn new_stream_revalidation_rejects_deleted_management_token() {
        let fixture = revocation_fixture(None);
        let proxy = register_management_token_connection(&fixture.state, RAW_MANAGEMENT_TOKEN);
        assert_connection_authorized(&fixture.state, &proxy).await;

        assert!(fixture
            .token_repository
            .delete_management_token(REVOCATION_TOKEN_ID)
            .await
            .expect("management token deletion should succeed"));

        assert_connection_revoked(&fixture.state, &proxy).await;
    }

    #[tokio::test]
    async fn new_stream_revalidation_rejects_regenerated_management_token_secret() {
        let fixture = revocation_fixture(None);
        let proxy = register_management_token_connection(&fixture.state, RAW_MANAGEMENT_TOKEN);
        assert_connection_authorized(&fixture.state, &proxy).await;

        fixture
            .token_repository
            .regenerate_management_token_secret(&RegenerateManagementTokenSecret {
                token_id: REVOCATION_TOKEN_ID.to_string(),
                token_hash: management_token_hash(ROTATED_MANAGEMENT_TOKEN),
                token_prefix: Some("ae-tunnel-rotated".to_string()),
            })
            .await
            .expect("management token regeneration should succeed")
            .expect("management token should exist");

        assert_connection_revoked(&fixture.state, &proxy).await;
        let rotated_proxy =
            register_management_token_connection(&fixture.state, ROTATED_MANAGEMENT_TOKEN);
        assert_connection_authorized(&fixture.state, &rotated_proxy).await;
    }

    #[tokio::test]
    async fn same_token_id_recreated_with_different_hash_does_not_rebind_old_connection() {
        let fixture = revocation_fixture(None);
        let proxy = register_management_token_connection(&fixture.state, RAW_MANAGEMENT_TOKEN);
        assert_connection_authorized(&fixture.state, &proxy).await;

        assert!(fixture
            .token_repository
            .delete_management_token(REVOCATION_TOKEN_ID)
            .await
            .expect("original management token deletion should succeed"));
        fixture
            .token_repository
            .create_management_token(&management_token_create_record(ROTATED_MANAGEMENT_TOKEN))
            .await
            .expect("same-id replacement management token should be created");

        assert_connection_revoked(&fixture.state, &proxy).await;
        let replacement_proxy =
            register_management_token_connection(&fixture.state, ROTATED_MANAGEMENT_TOKEN);
        assert_connection_authorized(&fixture.state, &replacement_proxy).await;
    }
}

pub fn build_router_with_state(state: AppState) -> Router {
    middleware::apply_cf_header_stripping(
        Router::new()
            .route("/health", get(health))
            .route("/metrics", get(metrics))
            .route("/stats", get(stats))
            .route("/api/internal/proxy-tunnel", get(ws_proxy))
            .route(
                "/api/internal/tunnel/relay/{node_id}",
                post(local_relay::relay_request),
            )
            .with_state(state),
    )
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let request_concurrency = state.request_concurrency_snapshot().map(|snapshot| {
        serde_json::json!({
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
            serde_json::json!({
                "limit": snapshot.limit,
                "in_flight": snapshot.in_flight,
                "available_permits": snapshot.available_permits,
                "high_watermark": snapshot.high_watermark,
                "rejected": snapshot.rejected,
            })
        });
    Json(serde_json::json!({
        "status": "ok",
        "request_concurrency": request_concurrency,
        "distributed_request_concurrency": distributed_request_concurrency,
    }))
}

async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.hub.stats())
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    prometheus_response(&state.metric_samples().await)
}

pub async fn ws_proxy(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let node_id = match required_proxy_tunnel_header(
        &headers,
        "x-node-id",
        MAX_TUNNEL_SECURITY_NODE_ID_LEN,
        ProxyTunnelSecurityError::MalformedHeader,
    ) {
        Ok(node_id) if valid_proxy_tunnel_token(node_id, 1) => node_id.to_string(),
        _ => {
            warn!("proxy connection rejected: invalid X-Node-ID header");
            return axum::http::StatusCode::BAD_REQUEST.into_response();
        }
    };

    let node_name = resolve_proxy_node_name(&headers, &node_id);

    let requested_generation = match required_proxy_tunnel_header(
        &headers,
        aether_contracts::tunnel_security::TUNNEL_GENERATION_HEADER,
        128,
        ProxyTunnelSecurityError::MalformedHeader,
    ) {
        Ok(generation) if valid_proxy_tunnel_token(generation, 1) => generation.to_string(),
        _ => {
            warn!(node_id = %node_id, "proxy connection rejected: invalid tunnel generation header");
            return axum::http::StatusCode::BAD_REQUEST.into_response();
        }
    };

    let max_streams = resolve_proxy_max_streams(&headers, state.max_streams);
    let raw_protocol_version = match required_proxy_tunnel_header(
        &headers,
        aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
        3,
        ProxyTunnelSecurityError::MalformedHeader,
    )
    .and_then(|value| {
        value
            .parse::<u8>()
            .ok()
            .filter(|value| {
                (1..=aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION).contains(value)
            })
            .ok_or(ProxyTunnelSecurityError::MalformedHeader)
    }) {
        Ok(version) => version,
        Err(error) => return error.status_code().into_response(),
    };
    let protocol_version = resolve_proxy_protocol_version(&headers);
    if protocol_version != raw_protocol_version {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }
    let stored_security_binding = match state
        .secure_tunnel_binding_for_handshake(&node_id, &requested_generation)
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            warn!(node_id = %node_id, error = %redact_error_detail(&error), "proxy connection rejected: tunnel security key lookup unavailable");
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let tunnel_security =
        if headers.contains_key(aether_contracts::tunnel_security::TUNNEL_SECURITY_HEADER) {
            match required_proxy_tunnel_header(
                &headers,
                aether_contracts::tunnel_security::TUNNEL_SECURITY_HEADER,
                32,
                ProxyTunnelSecurityError::MissingMode,
            ) {
                Ok(mode) => Some(mode),
                Err(error) => return error.status_code().into_response(),
            }
        } else {
            None
        };
    let stored_security_key = stored_security_binding.as_ref().map(|(key, _)| key.clone());
    if stored_security_binding
        .as_ref()
        .is_some_and(|(_, generation)| generation != &requested_generation)
    {
        warn!(node_id = %node_id, "proxy connection rejected: stale tunnel generation");
        return ProxyTunnelSecurityError::InvalidProof
            .status_code()
            .into_response();
    }
    let (security_key, security_session, management_token_credential, node_generation) =
        match tunnel_security {
            Some(aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED) => {
                match state
                    .authenticate_proxy_tunnel_security(
                        &headers,
                        &node_id,
                        &requested_generation,
                        protocol_version,
                        stored_security_key,
                    )
                    .await
                {
                    Ok(security) => (
                        Some(security.0),
                        security.1,
                        None,
                        requested_generation.clone(),
                    ),
                    Err(error) => {
                        warn!(node_id = %node_id, error = %redact_error_debug(&error), "proxy connection rejected: invalid secure tunnel handshake");
                        return error.status_code().into_response();
                    }
                }
            }
            Some(_) => {
                return ProxyTunnelSecurityError::UnsupportedMode
                    .status_code()
                    .into_response()
            }
            None if stored_security_key.is_some() => {
                warn!(
                    node_id = %node_id,
                    "proxy connection rejected: registered secure tunnel requires encrypted frames"
                );
                return ProxyTunnelSecurityError::MissingMode
                    .status_code()
                    .into_response();
            }
            None => {
                let client_ip = crate::headers::effective_client_ip(&headers, &remote_addr);
                let (credential, node_generation) = match state
                    .authenticate_proxy_tunnel_management_token(&headers, &node_id, client_ip)
                    .await
                {
                    Ok(credential) => credential,
                    Err(error) => {
                        warn!(node_id = %node_id, error = %redact_error_debug(&error), "proxy connection rejected: invalid management token");
                        return error.status_code().into_response();
                    }
                };
                if node_generation != requested_generation {
                    warn!(node_id = %node_id, "proxy connection rejected: stale tunnel generation");
                    return ProxyTunnelSecurityError::InvalidAuthorization
                        .status_code()
                        .into_response();
                }
                (None, String::new(), Some(credential), node_generation)
            }
        };

    let request_permit = match state.try_acquire_request_permit().await {
        Ok(permit) => permit,
        Err(RequestAdmissionError::Local(ConcurrencyError::Saturated { .. }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Saturated { .. }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Unavailable { .. })) => {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(RequestAdmissionError::Local(ConcurrencyError::Closed { gate })) => {
            warn!(
                gate = gate,
                "standalone tunnel relay request concurrency gate is closed"
            );
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::InvalidConfiguration(
            message,
        ))) => {
            warn!(
                error = %message,
                "standalone tunnel relay distributed request gate is invalid"
            );
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    ws.max_frame_size(64 * 1024 * 1024)
        .on_upgrade(move |socket| {
            hold_admission_permit_until(request_permit, async move {
                proxy_conn::handle_proxy_connection(
                    socket,
                    state.hub,
                    node_id,
                    node_name,
                    node_generation,
                    max_streams,
                    protocol_version,
                    security_key,
                    security_session,
                    management_token_credential,
                    state.proxy_conn_cfg,
                )
                .await
            })
        })
        .into_response()
}
