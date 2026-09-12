use std::future::Future;
use std::time::{Duration, Instant};

use crate::execution_runtime::transport::format_upstream_request_error;
use crate::handlers::admin::model::{
    acquire_admin_external_models_config_mutation_lock,
    release_admin_external_models_config_mutation_lock,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::maintenance::{
    cancel_proxy_upgrade_rollout, clear_proxy_upgrade_rollout_conflicts,
    restore_proxy_upgrade_rollout_skipped_nodes, retry_proxy_upgrade_rollout_node,
    skip_proxy_upgrade_rollout_node, ProxyUpgradeRolloutProbeConfig,
};
use crate::GatewayError;
use aether_admin::system::{
    admin_proxy_node_event_node_id_from_path, admin_proxy_node_metrics_node_id_from_path,
    build_admin_proxy_node_payload, build_admin_proxy_nodes_data_unavailable_response,
    build_admin_proxy_nodes_not_found_response,
};
use aether_contracts::tunnel::TUNNEL_RELAY_FORWARDED_BY_HEADER;
use aether_data::repository::management_tokens::{
    CreateManagementTokenRecord, StoredManagementToken, StoredManagementTokenUserSummary,
};
use aether_data::repository::proxy_nodes::{ProxyNodeEventQuery, ProxyNodeMetricsStep};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::warn;
use uuid::Uuid;

use crate::handlers::public::build_proxy_node_install_session_response;
use crate::handlers::shared::generate_gateway_secret_plaintext;
use crate::LocalMutationOutcome;

#[derive(Debug, Deserialize)]
struct ProxyNodeRegisterRequest {
    name: String,
    ip: String,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    heartbeat_interval: Option<i32>,
    #[serde(default)]
    active_connections: Option<i32>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    avg_latency_ms: Option<f64>,
    #[serde(default)]
    hardware_info: Option<Value>,
    #[serde(default)]
    estimated_max_concurrency: Option<i32>,
    #[serde(default)]
    proxy_metadata: Option<Value>,
    #[serde(default)]
    proxy_version: Option<String>,
    #[serde(default)]
    tunnel_mode: Option<bool>,
    #[serde(default)]
    tunnel_security: Option<String>,
    #[serde(default)]
    tunnel_encryption_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProxyNodeHeartbeatRequest {
    node_id: String,
    #[serde(default)]
    heartbeat_interval: Option<i32>,
    #[serde(default)]
    active_connections: Option<i32>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    avg_latency_ms: Option<f64>,
    #[serde(default)]
    failed_requests: Option<i64>,
    #[serde(default)]
    dns_failures: Option<i64>,
    #[serde(default)]
    stream_errors: Option<i64>,
    #[serde(default)]
    proxy_metadata: Option<Value>,
    #[serde(default)]
    proxy_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProxyNodeUnregisterRequest {
    node_id: String,
}

#[derive(Deserialize)]
struct ManualProxyNodeCreateRequest {
    name: String,
    proxy_url: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

#[derive(Deserialize)]
struct ManualProxyNodeUpdateRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    proxy_url: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

#[derive(Deserialize)]
struct ProxyNodeTestUrlRequest {
    proxy_url: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProxyNodeInstallSessionCreateRequest {
    node_name: String,
}

#[derive(Debug, Deserialize)]
struct ProxyNodeBatchUpgradeRequest {
    version: String,
    #[serde(default)]
    batch_size: Option<usize>,
    #[serde(default)]
    cooldown_secs: Option<u64>,
    #[serde(default)]
    probe_url: Option<String>,
    #[serde(default)]
    probe_timeout_secs: Option<u64>,
}

#[derive(Debug, Default)]
struct ProxyNodeBatchUpgradeDispatchSummary {
    version: String,
    eligible_total: usize,
    updated: usize,
    skipped: usize,
    node_ids: Vec<String>,
    rollout_cancelled: bool,
}

const JSON_OBJECT_REQUIRED_DETAIL: &str = "请求体必须是合法的 JSON 对象";
const DEFAULT_PROXY_UPGRADE_BATCH_SIZE: usize = 1;
const DEFAULT_PROXY_UPGRADE_COOLDOWN_SECS: u64 = 60;
const DEFAULT_PROXY_UPGRADE_PROBE_TIMEOUT_SECS: u64 = 10;
const DEFAULT_PROXY_CONNECTIVITY_PROBE_URL: &str = "https://www.cloudflare.com/cdn-cgi/trace";
const PROXY_CONNECTIVITY_TIMEOUT_SECS: u64 = 10;
const TUNNEL_RELAY_ENVELOPE_CONTENT_TYPE: &str = "application/vnd.aether.tunnel-envelope";
const MAX_PROXY_CONNECTIVITY_RESPONSE_BYTES: usize = 64 * 1024;
const PROXY_NODE_METRICS_MAX_POINTS: usize = 50_000;
const PROXY_NODE_METRICS_1M_MAX_WINDOW_SECS: u64 = 30 * 24 * 60 * 60;
const PROXY_NODE_METRICS_1H_MAX_WINDOW_SECS: u64 = 365 * 24 * 60 * 60;
const PROXY_INSTALL_INTERNAL_ERROR_DETAIL: &str = "Service temporarily unavailable";

#[cfg(test)]
fn manual_proxy_connectivity_probe_url_override() -> &'static std::sync::RwLock<Option<String>> {
    static OVERRIDE: std::sync::OnceLock<std::sync::RwLock<Option<String>>> =
        std::sync::OnceLock::new();
    OVERRIDE.get_or_init(|| std::sync::RwLock::new(None))
}

#[cfg(test)]
fn manual_proxy_connectivity_probe_url_override_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

fn proxy_connectivity_probe_url() -> String {
    #[cfg(test)]
    if let Some(url) = manual_proxy_connectivity_probe_url_override()
        .read()
        .expect("probe url override lock should read")
        .clone()
    {
        return url;
    }

    DEFAULT_PROXY_CONNECTIVITY_PROBE_URL.to_string()
}

#[cfg(test)]
pub(crate) struct ProxyConnectivityProbeUrlOverrideGuard(std::sync::MutexGuard<'static, ()>);

#[cfg(test)]
pub(crate) fn override_proxy_connectivity_probe_url_for_tests(
    url: impl Into<String>,
) -> ProxyConnectivityProbeUrlOverrideGuard {
    let guard = manual_proxy_connectivity_probe_url_override_lock()
        .lock()
        .expect("probe url override lock should acquire");
    *manual_proxy_connectivity_probe_url_override()
        .write()
        .expect("probe url override lock should write") = Some(url.into());
    ProxyConnectivityProbeUrlOverrideGuard(guard)
}

#[cfg(test)]
impl Drop for ProxyConnectivityProbeUrlOverrideGuard {
    fn drop(&mut self) {
        *manual_proxy_connectivity_probe_url_override()
            .write()
            .expect("probe url override lock should write") = None;
    }
}

pub(crate) async fn maybe_build_local_admin_proxy_nodes_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    headers: &http::HeaderMap,
    remote_addr: &std::net::SocketAddr,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("proxy_nodes_manage") {
        return Ok(None);
    }

    if decision.route_kind.as_deref() == Some("list_nodes")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/proxy-nodes" | "/api/admin/proxy-nodes/"
        )
    {
        let skip = query_param_value(request_context.query_string(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.query_string(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 1000)
            .unwrap_or(100);
        let status = query_param_value(request_context.query_string(), "status")
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        return Ok(Some(
            state
                .build_admin_proxy_nodes_list_response(skip, limit, status)
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("list_node_events")
        && request_context.method() == http::Method::GET
    {
        let Some(node_id) = admin_proxy_node_event_node_id_from_path(request_context.path()) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };

        let query = match parse_proxy_node_event_query(request_context.query_string()) {
            Ok(query) => query,
            Err(response) => return Ok(Some(response)),
        };
        return Ok(Some(
            state
                .build_admin_proxy_node_events_response(node_id, &query)
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("list_node_metrics")
        && request_context.method() == http::Method::GET
    {
        let Some(node_id) = admin_proxy_node_metrics_node_id_from_path(request_context.path())
        else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        let (step, from_unix_secs, to_unix_secs, limit) =
            match parse_proxy_node_metrics_query(request_context.query_string()) {
                Ok(query) => query,
                Err(response) => return Ok(Some(response)),
            };
        return Ok(Some(
            state
                .build_admin_proxy_node_metrics_response(
                    node_id,
                    step,
                    from_unix_secs,
                    to_unix_secs,
                    limit,
                )
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("list_fleet_metrics")
        && request_context.method() == http::Method::GET
    {
        let (step, from_unix_secs, to_unix_secs, limit) =
            match parse_proxy_node_metrics_query(request_context.query_string()) {
                Ok(query) => query,
                Err(response) => return Ok(Some(response)),
            };
        return Ok(Some(
            state
                .build_admin_proxy_fleet_metrics_response(step, from_unix_secs, to_unix_secs, limit)
                .await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("get_node")
        && request_context.method() == http::Method::GET
    {
        if !state.has_proxy_node_reader() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let Some(node_id) = admin_proxy_node_node_id_from_path(request_context.path()) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        let Some(node) = state.find_proxy_node(&node_id).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        return Ok(Some(
            Json(json!({
                "node": build_admin_proxy_node_detail_payload(&node),
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("register_node")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let input = match parse_json_body::<ProxyNodeRegisterRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let mutation = match validate_register_request(input, request_context) {
            Ok(mutation) => mutation,
            Err(response) => return Ok(Some(response)),
        };
        let tunnel_encryption_key = mutation
            .proxy_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/tunnel_security/encryption_key"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let Some(node) = state.register_proxy_node(&mutation).await? else {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        };
        if let Some(key) = tunnel_encryption_key {
            state
                .app()
                .tunnel
                .register_secure_tunnel_key(node.id.clone(), key);
        }
        state.app().tunnel.request_close_proxies_for_node(&node.id);
        return Ok(Some(
            Json(json!({
                "node_id": node.id,
                "tunnel_generation": node.tunnel_generation,
                "node": build_admin_proxy_node_payload(&node),
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("heartbeat_node")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let input = match parse_json_body::<ProxyNodeHeartbeatRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let mutation = match validate_heartbeat_request(input) {
            Ok(mutation) => mutation,
            Err(response) => return Ok(Some(response)),
        };
        let Some(existing) = state.find_proxy_node(&mutation.node_id).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        if !existing.tunnel_mode {
            return Ok(Some(bad_request_response(
                "non-tunnel mode is no longer supported, please upgrade aether-tunnel to use tunnel mode",
            )));
        }
        let Some(node) = state.apply_proxy_node_heartbeat(&mutation).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        return Ok(Some(
            Json(json!({
                "message": "heartbeat ok",
                "node": build_admin_proxy_node_payload(&node),
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("unregister_node")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let input = match parse_json_body::<ProxyNodeUnregisterRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let node_id = match validate_node_id(&input.node_id) {
            Ok(node_id) => node_id,
            Err(response) => return Ok(Some(response)),
        };
        let Some(node) = state.unregister_proxy_node(&node_id).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        state.app().tunnel.request_close_proxies_for_node(&node.id);
        return Ok(Some(
            Json(json!({
                "message": "unregistered",
                "node_id": node.id,
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("create_manual_node")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let input = match parse_json_body::<ManualProxyNodeCreateRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let mutation = match validate_manual_create_request(input, request_context) {
            Ok(mutation) => mutation,
            Err(response) => return Ok(Some(response)),
        };
        let Some(node) = state.create_manual_proxy_node(&mutation).await? else {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        };
        return Ok(Some(
            Json(json!({
                "node_id": node.id,
                "node": build_admin_proxy_node_payload(&node),
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("create_proxy_node_install_session")
        && request_context.method() == http::Method::POST
    {
        if !state.app().has_management_token_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let input = match parse_json_body::<ProxyNodeInstallSessionCreateRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let node_name = match validate_proxy_install_node_name(&input.node_name) {
            Ok(node_name) => node_name,
            Err(response) => return Ok(Some(response)),
        };
        let (token_record, raw_token) =
            match create_proxy_install_management_token(state, request_context, &node_name).await {
                Ok(token) => token,
                Err(response) => return Ok(Some(response)),
            };
        return Ok(Some(
            build_proxy_node_install_session_response(
                state.app(),
                request_context.public(),
                headers,
                remote_addr,
                node_name,
                &token_record,
                raw_token,
            )
            .await,
        ));
    }

    if decision.route_kind.as_deref() == Some("update_manual_node")
        && request_context.method() == http::Method::PATCH
    {
        if !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let Some(node_id) = admin_proxy_node_node_id_from_path(request_context.path()) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        let input = match parse_json_body::<ManualProxyNodeUpdateRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let mutation = match validate_manual_update_request(node_id, input) {
            Ok(mutation) => mutation,
            Err(response) => return Ok(Some(response)),
        };
        let Some(node) = state.update_manual_proxy_node(&mutation).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        return Ok(Some(
            Json(json!({
                "node_id": node.id,
                "node": build_admin_proxy_node_payload(&node),
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("delete_node")
        && request_context.method() == http::Method::DELETE
    {
        if !state.has_proxy_node_reader() || !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let Some(node_id) = admin_proxy_node_node_id_from_path(request_context.path()) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        let lock = match acquire_admin_external_models_config_mutation_lock(state).await {
            Ok(lock) => lock,
            Err((status, payload)) => {
                return Ok(Some((status, Json(payload)).into_response()));
            }
        };
        let delete_result: Result<Response<Body>, GatewayError> = async {
            if state.find_proxy_node(&node_id).await?.is_none() {
                return Ok(build_admin_proxy_nodes_not_found_response());
            }

            // Persistent references are cleared before the node itself. If a durable cleanup
            // fails, the node remains available and the operation can be retried safely.
            let cleanup = clear_proxy_node_references_before_delete(state, &node_id).await?;
            let Some(_deleted_node) = state.delete_proxy_node(&node_id).await? else {
                return Ok(build_admin_proxy_nodes_not_found_response());
            };
            state.app().tunnel.request_close_proxies_for_node(&node_id);
            Ok(Json(json!({
                "message": build_delete_proxy_node_message(&cleanup),
                "node_id": node_id,
                "cleared_system_proxy": cleanup.cleared_system_proxy,
                "cleared_external_models_proxy": cleanup.cleared_external_models_proxy,
                "cleared_providers": cleanup.cleared_providers,
                "cleared_endpoints": cleanup.cleared_endpoints,
                "cleared_keys": cleanup.cleared_keys,
            }))
            .into_response())
        }
        .await;
        release_admin_external_models_config_mutation_lock(state, &lock).await;
        return delete_result.map(Some);
    }

    if decision.route_kind.as_deref() == Some("test_node")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_reader() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let Some(node_id) = admin_proxy_node_test_node_id_from_path(request_context.path()) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        let Some(node) = state.find_proxy_node(&node_id).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        return Ok(Some(
            Json(test_proxy_node_connectivity(state, &node).await).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("test_proxy_url")
        && request_context.method() == http::Method::POST
    {
        let input = match parse_json_body::<ProxyNodeTestUrlRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let normalized = match validate_proxy_test_url_request(input) {
            Ok(normalized) => normalized,
            Err(response) => return Ok(Some(response)),
        };
        return Ok(Some(
            Json(test_manual_proxy_connectivity(&normalized).await).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("update_node_config")
        && request_context.method() == http::Method::PUT
    {
        if !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let Some(node_id) = admin_proxy_node_config_node_id_from_path(request_context.path())
        else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        let raw = match parse_json_object_body(request_body) {
            Ok(raw) => raw,
            Err(response) => return Ok(Some(response)),
        };
        let Some(existing) = state.find_proxy_node(&node_id).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        if existing.is_manual {
            return Ok(Some(bad_request_response("手动节点不支持远程配置下发")));
        }
        let mutation = match validate_remote_config_request(node_id, &raw) {
            Ok(mutation) => mutation,
            Err(response) => return Ok(Some(response)),
        };
        let Some(node) = state.update_proxy_node_remote_config(&mutation).await? else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };
        return Ok(Some(
            Json(json!({
                "node_id": node.id,
                "config_version": node.config_version,
                "remote_config": node.remote_config,
                "node": build_admin_proxy_node_payload(&node),
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("batch_upgrade_nodes")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_reader() || !state.has_proxy_node_writer() {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let input = match parse_json_body::<ProxyNodeBatchUpgradeRequest>(request_body) {
            Ok(input) => input,
            Err(response) => return Ok(Some(response)),
        };
        let version = match validate_version(&input.version) {
            Ok(version) => version,
            Err(response) => return Ok(Some(response)),
        };
        let summary = dispatch_proxy_node_upgrade_targets(state, &version).await?;

        return Ok(Some(
            Json(json!({
                "version": summary.version,
                "eligible_total": summary.eligible_total,
                "updated": summary.updated,
                "skipped": summary.skipped,
                "node_ids": summary.node_ids,
                "rollout_cancelled": summary.rollout_cancelled,
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("cancel_upgrade_rollout")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_reader()
            || !state.has_proxy_node_writer()
            || !state.app().data.has_system_config_store()
        {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }

        let summary = cancel_proxy_upgrade_rollout(&state.app().data)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(Some(
            Json(match summary {
                Some(summary) => json!({
                    "cancelled": true,
                    "version": summary.version,
                    "pending_node_ids": summary.pending_node_ids,
                    "conflict_node_ids": summary.conflict_node_ids,
                    "completed": summary.completed,
                    "remaining": summary.remaining,
                }),
                None => json!({
                    "cancelled": false,
                    "rollout_active": false,
                }),
            })
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("clear_upgrade_rollout_conflicts")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_reader()
            || !state.has_proxy_node_writer()
            || !state.app().data.has_system_config_store()
        {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }

        let summary = clear_proxy_upgrade_rollout_conflicts(&state.app().data)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(Some(
            Json(match summary {
                Some(summary) => json!({
                    "version": summary.version,
                    "cleared": summary.cleared_node_ids.len(),
                    "node_ids": summary.cleared_node_ids,
                    "updated": summary.updated,
                    "blocked": summary.blocked,
                    "pending_node_ids": summary.pending_node_ids,
                    "rollout_active": summary.rollout_active,
                    "completed": summary.completed,
                    "remaining": summary.remaining,
                }),
                None => json!({
                    "version": null,
                    "cleared": 0,
                    "node_ids": [],
                    "updated": 0,
                    "blocked": false,
                    "pending_node_ids": [],
                    "rollout_active": false,
                    "completed": 0,
                    "remaining": 0,
                }),
            })
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("restore_skipped_upgrade_rollout_nodes")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_reader()
            || !state.has_proxy_node_writer()
            || !state.app().data.has_system_config_store()
        {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }

        let summary = restore_proxy_upgrade_rollout_skipped_nodes(&state.app().data)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(Some(
            Json(match summary {
                Some(summary) => json!({
                    "version": summary.version,
                    "restored": summary.restored_node_ids.len(),
                    "node_ids": summary.restored_node_ids,
                    "skipped_node_ids": summary.skipped_node_ids,
                    "updated": summary.updated,
                    "blocked": summary.blocked,
                    "pending_node_ids": summary.pending_node_ids,
                    "rollout_active": summary.rollout_active,
                    "completed": summary.completed,
                    "remaining": summary.remaining,
                }),
                None => json!({
                    "version": null,
                    "restored": 0,
                    "node_ids": [],
                    "skipped_node_ids": [],
                    "updated": 0,
                    "blocked": false,
                    "pending_node_ids": [],
                    "rollout_active": false,
                    "completed": 0,
                    "remaining": 0,
                }),
            })
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("skip_upgrade_rollout_node")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_reader()
            || !state.has_proxy_node_writer()
            || !state.app().data.has_system_config_store()
        {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let Some(node_id) = admin_proxy_node_upgrade_action_node_id_from_path(
            request_context.path(),
            "/upgrade/skip",
        ) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };

        let summary = skip_proxy_upgrade_rollout_node(&state.app().data, &node_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(Some(
            Json(match summary {
                Some(summary) => json!({
                    "version": summary.version,
                    "node_id": summary.node_id,
                    "skipped_node_ids": summary.skipped_node_ids,
                    "updated": summary.updated,
                    "blocked": summary.blocked,
                    "pending_node_ids": summary.pending_node_ids,
                    "rollout_active": summary.rollout_active,
                    "completed": summary.completed,
                    "remaining": summary.remaining,
                }),
                None => json!({
                    "version": null,
                    "node_id": node_id,
                    "skipped_node_ids": [],
                    "updated": 0,
                    "blocked": false,
                    "pending_node_ids": [],
                    "rollout_active": false,
                    "completed": 0,
                    "remaining": 0,
                }),
            })
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("retry_upgrade_rollout_node")
        && request_context.method() == http::Method::POST
    {
        if !state.has_proxy_node_reader()
            || !state.has_proxy_node_writer()
            || !state.app().data.has_system_config_store()
        {
            return Ok(Some(build_admin_proxy_nodes_data_unavailable_response()));
        }
        let Some(node_id) = admin_proxy_node_upgrade_action_node_id_from_path(
            request_context.path(),
            "/upgrade/retry",
        ) else {
            return Ok(Some(build_admin_proxy_nodes_not_found_response()));
        };

        let summary = retry_proxy_upgrade_rollout_node(&state.app().data, &node_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(Some(
            Json(match summary {
                Some(summary) => json!({
                    "version": summary.version,
                    "node_id": summary.node_id,
                    "skipped_node_ids": summary.skipped_node_ids,
                    "updated": summary.updated,
                    "blocked": summary.blocked,
                    "pending_node_ids": summary.pending_node_ids,
                    "rollout_active": summary.rollout_active,
                    "completed": summary.completed,
                    "remaining": summary.remaining,
                }),
                None => json!({
                    "version": null,
                    "node_id": node_id,
                    "skipped_node_ids": [],
                    "updated": 0,
                    "blocked": false,
                    "pending_node_ids": [],
                    "rollout_active": false,
                    "completed": 0,
                    "remaining": 0,
                }),
            })
            .into_response(),
        ));
    }

    Ok(Some(build_admin_proxy_nodes_data_unavailable_response()))
}

#[derive(Debug, Default)]
struct DeletedProxyNodeCleanup {
    cleared_system_proxy: bool,
    cleared_external_models_proxy: bool,
    external_models_cache_clear_succeeded: Option<bool>,
    cleared_providers: usize,
    cleared_endpoints: usize,
    cleared_keys: usize,
}

fn build_admin_proxy_node_detail_payload(
    node: &aether_data::repository::proxy_nodes::StoredProxyNode,
) -> Value {
    build_admin_proxy_node_payload(node)
}

#[derive(Debug, Clone)]
struct NormalizedManualProxyEndpoint {
    proxy_url: String,
    host: String,
    port: u16,
    node_ip: String,
    node_port: i32,
}

async fn clear_proxy_node_references_before_delete(
    state: &AdminAppState<'_>,
    node_id: &str,
) -> Result<DeletedProxyNodeCleanup, GatewayError> {
    let external_models_cache_clear = state.clear_admin_external_models_cache();
    clear_proxy_node_references_before_delete_with_cache(
        state,
        node_id,
        external_models_cache_clear,
    )
    .await
}

async fn clear_proxy_node_references_before_delete_with_cache<F>(
    state: &AdminAppState<'_>,
    node_id: &str,
    external_models_cache_clear: F,
) -> Result<DeletedProxyNodeCleanup, GatewayError>
where
    F: Future<Output = Result<Value, GatewayError>>,
{
    let mut cleanup = DeletedProxyNodeCleanup::default();

    if state.app().data.has_system_config_store() {
        let is_system_proxy = state
            .read_system_config_json_value_strong("system_proxy_node_id")
            .await?
            .and_then(|value| value.as_str().map(str::trim).map(ToOwned::to_owned))
            .is_some_and(|value| value == node_id);
        if is_system_proxy {
            state
                .upsert_system_config_json_value(
                    "system_proxy_node_id",
                    &serde_json::Value::Null,
                    None,
                )
                .await?;
            cleanup.cleared_system_proxy = true;
        }

        let is_external_models_proxy = state
            .read_system_config_json_value_strong("external_models_proxy_node_id")
            .await?
            .and_then(|value| value.as_str().map(str::trim).map(ToOwned::to_owned))
            .is_some_and(|value| value == node_id);
        if is_external_models_proxy {
            state
                .upsert_system_config_json_value(
                    "external_models_proxy_node_id",
                    &serde_json::Value::Null,
                    None,
                )
                .await?;
            cleanup.external_models_cache_clear_succeeded =
                Some(match external_models_cache_clear.await {
                    Ok(_) => true,
                    Err(_) => {
                        // The selector is already persisted as null, and v2 cache entries carry
                        // their selector. A failed DEL therefore cannot route through this node.
                        warn!(
                            runtime_backend = state.app().runtime_state_backend(),
                            proxy_node_id = %node_id,
                            "failed to clear external models cache while deleting proxy node"
                        );
                        false
                    }
                });
            cleanup.cleared_external_models_proxy = true;
        }
    }

    if state.app().has_provider_catalog_data_reader()
        && state.app().has_provider_catalog_data_writer()
    {
        let providers = state.list_provider_catalog_providers(false).await?;
        let provider_ids = providers
            .iter()
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>();

        for mut provider in providers {
            if !proxy_reference_matches_node_id(provider.proxy.as_ref(), node_id) {
                continue;
            }
            provider.proxy = None;
            if state
                .update_provider_catalog_provider(&provider)
                .await?
                .is_some()
            {
                cleanup.cleared_providers = cleanup.cleared_providers.saturating_add(1);
            }
        }

        if !provider_ids.is_empty() {
            let endpoints = state
                .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
                .await?;
            for mut endpoint in endpoints {
                if !proxy_reference_matches_node_id(endpoint.proxy.as_ref(), node_id) {
                    continue;
                }
                endpoint.proxy = None;
                if state
                    .update_provider_catalog_endpoint(&endpoint)
                    .await?
                    .is_some()
                {
                    cleanup.cleared_endpoints = cleanup.cleared_endpoints.saturating_add(1);
                }
            }

            let keys = state
                .list_provider_catalog_keys_by_provider_ids(&provider_ids)
                .await?;
            for mut key in keys {
                if !proxy_reference_matches_node_id(key.proxy.as_ref(), node_id) {
                    continue;
                }
                key.proxy = None;
                if state.update_provider_catalog_key(&key).await?.is_some() {
                    cleanup.cleared_keys = cleanup.cleared_keys.saturating_add(1);
                }
            }
        }
    }

    Ok(cleanup)
}

#[cfg(test)]
pub(crate) async fn clear_proxy_node_references_with_cache_failure_for_tests(
    app: &crate::AppState,
    node_id: &str,
) -> Result<Value, GatewayError> {
    let state = AdminAppState::new(app);
    let cleanup = clear_proxy_node_references_before_delete_with_cache(&state, node_id, async {
        Err(GatewayError::Internal(
            "injected cache delete failure".to_string(),
        ))
    })
    .await?;
    Ok(json!({
        "cleared_system_proxy": cleanup.cleared_system_proxy,
        "cleared_external_models_proxy": cleanup.cleared_external_models_proxy,
        "external_models_cache_clear_succeeded": cleanup.external_models_cache_clear_succeeded,
        "cleared_providers": cleanup.cleared_providers,
        "cleared_endpoints": cleanup.cleared_endpoints,
        "cleared_keys": cleanup.cleared_keys,
    }))
}

fn build_delete_proxy_node_message(cleanup: &DeletedProxyNodeCleanup) -> String {
    let mut parts = vec!["deleted".to_string()];
    if cleanup.cleared_system_proxy {
        parts.push("system default proxy cleared".to_string());
    }
    if cleanup.cleared_external_models_proxy {
        parts.push("external models proxy cleared".to_string());
    }
    if cleanup.external_models_cache_clear_succeeded == Some(false) {
        parts.push("external models cache invalidation deferred".to_string());
    }
    if cleanup.cleared_providers > 0 || cleanup.cleared_endpoints > 0 || cleanup.cleared_keys > 0 {
        parts.push(format!(
            "cleared proxy refs from {} provider(s), {} endpoint(s), {} key(s)",
            cleanup.cleared_providers, cleanup.cleared_endpoints, cleanup.cleared_keys
        ));
    }
    parts.join(", ")
}

fn proxy_reference_matches_node_id(value: Option<&Value>, node_id: &str) -> bool {
    value
        .and_then(Value::as_object)
        .and_then(|object| object.get("node_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| value == node_id)
}

fn build_proxy_connectivity_result(
    probe_url: &str,
    timeout_secs: u64,
    success: bool,
    latency_ms: Option<u64>,
    exit_ip: Option<String>,
    error: Option<String>,
) -> Value {
    json!({
        "success": success,
        "latency_ms": latency_ms,
        "exit_ip": exit_ip,
        "error": error,
        "probe_url": probe_url.trim(),
        "timeout_secs": timeout_secs,
    })
}

async fn test_proxy_node_connectivity(
    state: &AdminAppState<'_>,
    node: &aether_data::repository::proxy_nodes::StoredProxyNode,
) -> Value {
    let probe_url = proxy_connectivity_probe_url();
    if node.is_manual {
        let Some(proxy_url) = node.proxy_url.as_deref() else {
            return build_proxy_connectivity_result(
                &probe_url,
                PROXY_CONNECTIVITY_TIMEOUT_SECS,
                false,
                None,
                None,
                Some("手动节点缺少 proxy_url".to_string()),
            );
        };
        let endpoint = match parse_manual_proxy_endpoint(proxy_url, "proxy_url") {
            Ok(endpoint) => endpoint,
            Err(detail) => {
                return build_proxy_connectivity_result(
                    &probe_url,
                    PROXY_CONNECTIVITY_TIMEOUT_SECS,
                    false,
                    None,
                    None,
                    Some(detail),
                );
            }
        };
        let proxy_password = match state.app().decrypt_proxy_node_password(&node.id).await {
            Ok(password) => password,
            Err(_) => {
                return build_proxy_connectivity_result(
                    &probe_url,
                    PROXY_CONNECTIVITY_TIMEOUT_SECS,
                    false,
                    None,
                    None,
                    Some("手动节点密码不可用".to_string()),
                );
            }
        };
        let Some(proxy_url) = proxy_url_with_auth(
            &endpoint.proxy_url,
            node.proxy_username.as_deref(),
            proxy_password.as_deref(),
        ) else {
            return build_proxy_connectivity_result(
                &probe_url,
                PROXY_CONNECTIVITY_TIMEOUT_SECS,
                false,
                None,
                None,
                Some("手动节点认证配置不可用".to_string()),
            );
        };
        return test_manual_proxy_connectivity(&proxy_url).await;
    }

    if !node.tunnel_mode {
        return build_proxy_connectivity_result(
            &probe_url,
            PROXY_CONNECTIVITY_TIMEOUT_SECS,
            false,
            None,
            None,
            Some(
                "non-tunnel mode is no longer supported, please upgrade aether-tunnel to use tunnel mode"
                    .to_string(),
            ),
        );
    }

    if !node.status.eq_ignore_ascii_case("online") || !node.tunnel_connected {
        return build_proxy_connectivity_result(
            &probe_url,
            PROXY_CONNECTIVITY_TIMEOUT_SECS,
            false,
            None,
            None,
            Some("tunnel 未连接".to_string()),
        );
    }

    match probe_tunnel_proxy_connectivity(
        state.app(),
        &node.id,
        &probe_url,
        PROXY_CONNECTIVITY_TIMEOUT_SECS,
    )
    .await
    {
        Ok(result) => {
            if let Ok(status) = reqwest::StatusCode::from_u16(result.status) {
                if status.is_success() {
                    return build_proxy_connectivity_result(
                        &probe_url,
                        PROXY_CONNECTIVITY_TIMEOUT_SECS,
                        true,
                        Some(result.latency_ms),
                        parse_proxy_probe_exit_ip(&result.body),
                        None,
                    );
                }

                return build_proxy_connectivity_result(
                    &probe_url,
                    PROXY_CONNECTIVITY_TIMEOUT_SECS,
                    false,
                    None,
                    None,
                    Some(sanitize_proxy_error(&format_proxy_probe_status_error(
                        status,
                        &result.body,
                    ))),
                );
            }

            build_proxy_connectivity_result(
                &probe_url,
                PROXY_CONNECTIVITY_TIMEOUT_SECS,
                false,
                None,
                None,
                Some(format!("代理探测返回非法状态码: {}", result.status)),
            )
        }
        Err(error) => build_proxy_connectivity_result(
            &probe_url,
            PROXY_CONNECTIVITY_TIMEOUT_SECS,
            false,
            None,
            None,
            Some(sanitize_proxy_error(&error)),
        ),
    }
}

async fn test_manual_proxy_connectivity(proxy_url: &str) -> Value {
    let probe_url = proxy_connectivity_probe_url();
    test_manual_proxy_connectivity_with_probe_url(
        proxy_url,
        &probe_url,
        PROXY_CONNECTIVITY_TIMEOUT_SECS,
    )
    .await
}

async fn test_manual_proxy_connectivity_with_probe_url(
    proxy_url: &str,
    probe_url: &str,
    timeout_secs: u64,
) -> Value {
    let started_at = Instant::now();
    // reqwest resolves the destination locally for `socks5://`, which would
    // bypass the gateway's private-address/DNS-rebinding guard.  Normalize
    // legacy SOCKS URLs to the remote-DNS form before probing; HTTP/HTTPS and
    // already-normalized `socks5h://` URLs are unchanged.
    let proxy_url =
        match crate::execution_runtime::transport::normalize_execution_proxy_url(proxy_url) {
            Ok(proxy_url) => proxy_url,
            Err(_) => {
                return build_proxy_connectivity_result(
                    probe_url,
                    timeout_secs,
                    false,
                    None,
                    None,
                    Some("代理 URL 无效".to_string()),
                );
            }
        };
    let proxy = match reqwest::Proxy::all(&proxy_url) {
        Ok(proxy) => proxy,
        Err(error) => {
            return build_proxy_connectivity_result(
                probe_url,
                timeout_secs,
                false,
                None,
                None,
                Some(sanitize_proxy_error(&error.to_string())),
            );
        }
    };
    let builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(timeout_secs))
        .proxy(proxy)
        .user_agent("aether-gateway/proxy-connectivity");
    let client = match builder.build() {
        Ok(client) => client,
        Err(error) => {
            return build_proxy_connectivity_result(
                probe_url,
                timeout_secs,
                false,
                None,
                None,
                Some(sanitize_proxy_error(&format_upstream_request_error(&error))),
            );
        }
    };

    let response = match client.get(probe_url).send().await {
        Ok(response) => response,
        Err(error) => {
            return build_proxy_connectivity_result(
                probe_url,
                timeout_secs,
                false,
                None,
                None,
                Some(sanitize_proxy_error(&format_upstream_request_error(&error))),
            );
        }
    };
    let status = response.status();
    let body = match aether_http::read_response_bytes_with_limit(
        response,
        MAX_PROXY_CONNECTIVITY_RESPONSE_BYTES,
    )
    .await
    {
        Ok(body) => String::from_utf8_lossy(&body).into_owned(),
        Err(error) => {
            return build_proxy_connectivity_result(
                probe_url,
                timeout_secs,
                false,
                None,
                None,
                Some(sanitize_proxy_error(&error.to_string())),
            );
        }
    };

    if !status.is_success() {
        return build_proxy_connectivity_result(
            probe_url,
            timeout_secs,
            false,
            None,
            None,
            Some(sanitize_proxy_error(&format_proxy_probe_status_error(
                status, &body,
            ))),
        );
    }

    build_proxy_connectivity_result(
        probe_url,
        timeout_secs,
        true,
        Some(started_at.elapsed().as_millis() as u64),
        parse_proxy_probe_exit_ip(&body),
        None,
    )
}

struct TunnelConnectivityProbeResult {
    status: u16,
    body: String,
    latency_ms: u64,
}

async fn probe_tunnel_proxy_connectivity(
    state: &crate::AppState,
    node_id: &str,
    probe_url: &str,
    timeout_secs: u64,
) -> Result<TunnelConnectivityProbeResult, String> {
    let trimmed_node_id = node_id.trim();
    if trimmed_node_id.is_empty() {
        return Err("proxy node id is empty".to_string());
    }

    if state.tunnel.has_local_proxy(trimmed_node_id) {
        return probe_tunnel_proxy_connectivity_locally(
            state,
            trimmed_node_id,
            probe_url,
            timeout_secs,
        )
        .await;
    }

    if let Some(owner) = state
        .tunnel
        .lookup_attachment_owner(state.data.as_ref(), trimmed_node_id)
        .await
        .map_err(|err| format!("lookup tunnel attachment owner failed: {err}"))?
    {
        if owner.gateway_instance_id != state.tunnel.local_instance_id() {
            return probe_tunnel_proxy_connectivity_via_owner(
                state,
                trimmed_node_id,
                probe_url,
                timeout_secs,
                &owner.relay_base_url,
                &owner.gateway_instance_id,
            )
            .await;
        }

        state
            .tunnel
            .clear_local_attachment_if_stale(state.data.as_ref(), trimmed_node_id)
            .await
            .map_err(|err| format!("clear stale local tunnel attachment failed: {err}"))?;
    }

    probe_tunnel_proxy_connectivity_locally(state, trimmed_node_id, probe_url, timeout_secs).await
}

async fn probe_tunnel_proxy_connectivity_locally(
    state: &crate::AppState,
    node_id: &str,
    probe_url: &str,
    timeout_secs: u64,
) -> Result<TunnelConnectivityProbeResult, String> {
    let started_at = Instant::now();
    let result = state
        .tunnel
        .probe_node_url_with_response(node_id, probe_url, timeout_secs)
        .await?;
    Ok(TunnelConnectivityProbeResult {
        status: result.status,
        body: result.body,
        latency_ms: started_at.elapsed().as_millis() as u64,
    })
}

async fn probe_tunnel_proxy_connectivity_via_owner(
    state: &crate::AppState,
    node_id: &str,
    probe_url: &str,
    timeout_secs: u64,
    relay_base_url: &str,
    owner_instance_id: &str,
) -> Result<TunnelConnectivityProbeResult, String> {
    let owner_url = crate::tunnel::build_tunnel_owner_relay_url(relay_base_url, node_id)?;
    let payload = build_tunnel_probe_relay_envelope(probe_url, timeout_secs)?;
    let relay_auth = state.tunnel.build_relay_auth_headers(
        owner_instance_id,
        node_id,
        true,
        false,
        &payload,
        &[],
    )?;
    let started_at = Instant::now();
    let owner_client =
        crate::tunnel::owner_forward_client_for_url(&state.owner_forward_client, &owner_url)
            .await?;
    let request = owner_client
        .post(owner_url)
        .header(
            http::header::CONTENT_TYPE,
            TUNNEL_RELAY_ENVELOPE_CONTENT_TYPE,
        )
        .header(
            TUNNEL_RELAY_FORWARDED_BY_HEADER,
            state.tunnel.local_instance_id(),
        )
        .timeout(Duration::from_secs(timeout_secs))
        .body(payload);
    let response = relay_auth
        .apply(request)
        .send()
        .await
        .map_err(|error| crate::tunnel::owner_forward_request_error(&error))?;
    let status = response.status();
    let body = aether_http::read_response_bytes_with_limit(
        response,
        MAX_PROXY_CONNECTIVITY_RESPONSE_BYTES,
    )
    .await
    .map_err(|_| "failed to read owner tunnel relay probe body".to_string())?;

    Ok(TunnelConnectivityProbeResult {
        status: status.as_u16(),
        body: String::from_utf8_lossy(&body).to_string(),
        latency_ms: started_at.elapsed().as_millis() as u64,
    })
}

fn build_tunnel_probe_relay_envelope(
    probe_url: &str,
    timeout_secs: u64,
) -> Result<Vec<u8>, String> {
    let meta = crate::tunnel::tunnel_protocol::RequestMeta {
        provider_id: None,
        endpoint_id: None,
        key_id: None,
        method: "GET".to_string(),
        url: probe_url.trim().to_string(),
        headers: std::collections::HashMap::new(),
        stream: false,
        request_timeout_ms: None,
        stream_first_byte_timeout_ms: None,
        timeout: timeout_secs,
        follow_redirects: Some(false),
        http1_only: false,
        transport_profile: None,
    };
    let meta_bytes = serde_json::to_vec(&meta)
        .map_err(|error| format!("encode tunnel probe metadata failed: {error}"))?;
    let mut envelope = Vec::with_capacity(4 + meta_bytes.len());
    envelope.extend_from_slice(&(meta_bytes.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_bytes);
    Ok(envelope)
}

fn validate_register_request(
    input: ProxyNodeRegisterRequest,
    request_context: &AdminRequestContext<'_>,
) -> Result<aether_data::repository::proxy_nodes::ProxyNodeRegistrationMutation, Response<Body>> {
    let name = normalize_required_string(&input.name, "name", 100)?;
    let ip = normalize_ip_address(&input.ip)?;
    let heartbeat_interval = validate_optional_i32_range(
        input.heartbeat_interval.unwrap_or(30),
        "heartbeat_interval",
        5,
        600,
    )?;
    if !input.tunnel_mode.unwrap_or(true) {
        return Err(bad_request_response("仅支持 tunnel_mode=true"));
    }
    validate_optional_counter(
        input.active_connections.map(i64::from),
        "active_connections",
    )?;
    validate_optional_counter(input.total_requests, "total_requests")?;
    validate_optional_counter(
        input.estimated_max_concurrency.map(i64::from),
        "estimated_max_concurrency",
    )?;
    if input
        .avg_latency_ms
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(bad_request_response("avg_latency_ms 必须是非负有限数值"));
    }
    validate_optional_object(input.hardware_info.as_ref(), "hardware_info")?;
    validate_optional_object(input.proxy_metadata.as_ref(), "proxy_metadata")?;
    let tunnel_security =
        normalize_optional_string(input.tunnel_security.as_deref(), "tunnel_security", 64)?;
    let tunnel_encryption_key = normalize_optional_string(
        input.tunnel_encryption_key.as_deref(),
        "tunnel_encryption_key",
        128,
    )?;

    let registered_by = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone());

    let mut proxy_metadata = input.proxy_metadata;
    if tunnel_security.as_deref()
        == Some(aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED)
    {
        let key = tunnel_encryption_key.as_deref().ok_or_else(|| {
            bad_request_response(
                "tunnel_encryption_key is required when tunnel_security=non_tls_required",
            )
        })?;
        aether_contracts::tunnel_security::decode_psk(key)
            .map_err(|err| bad_request_response(err.to_string()))?;
        let mut metadata = proxy_metadata
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        metadata.insert(
            "tunnel_security".to_string(),
            json!({
                "mode": aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "encryption_key": key,
            }),
        );
        proxy_metadata = Some(Value::Object(metadata));
    }

    Ok(
        aether_data::repository::proxy_nodes::ProxyNodeRegistrationMutation {
            node_id: None,
            name,
            ip,
            port: i32::from(input.port.unwrap_or_default()),
            region: normalize_optional_string(input.region.as_deref(), "region", 100)?,
            heartbeat_interval,
            active_connections: input.active_connections,
            total_requests: input.total_requests,
            avg_latency_ms: input.avg_latency_ms,
            hardware_info: input.hardware_info,
            estimated_max_concurrency: input.estimated_max_concurrency,
            proxy_metadata,
            proxy_version: normalize_optional_string(
                input.proxy_version.as_deref(),
                "proxy_version",
                20,
            )?,
            registered_by,
            tunnel_mode: true,
        },
    )
}

fn validate_manual_create_request(
    input: ManualProxyNodeCreateRequest,
    request_context: &AdminRequestContext<'_>,
) -> Result<aether_data::repository::proxy_nodes::ProxyNodeManualCreateMutation, Response<Body>> {
    let endpoint = normalize_manual_proxy_endpoint(&input.proxy_url)?;
    let registered_by = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone());

    Ok(
        aether_data::repository::proxy_nodes::ProxyNodeManualCreateMutation {
            node_id: None,
            name: normalize_required_string(&input.name, "name", 100)?,
            ip: endpoint.node_ip,
            port: endpoint.node_port,
            region: normalize_optional_string(input.region.as_deref(), "region", 100)?,
            proxy_url: endpoint.proxy_url,
            proxy_username: normalize_optional_string(input.username.as_deref(), "username", 255)?,
            proxy_password: normalize_optional_string(input.password.as_deref(), "password", 500)?,
            registered_by,
        },
    )
}

fn validate_manual_update_request(
    node_id: String,
    input: ManualProxyNodeUpdateRequest,
) -> Result<aether_data::repository::proxy_nodes::ProxyNodeManualUpdateMutation, Response<Body>> {
    let endpoint = match input.proxy_url.as_deref() {
        Some(proxy_url) => Some(normalize_manual_proxy_endpoint(proxy_url)?),
        None => None,
    };
    let name = normalize_optional_string(input.name.as_deref(), "name", 100)?;
    let region = normalize_optional_string(input.region.as_deref(), "region", 100)?;
    let proxy_username = normalize_optional_string(input.username.as_deref(), "username", 255)?;
    let proxy_password = normalize_optional_string(input.password.as_deref(), "password", 500)?;

    if name.is_none()
        && region.is_none()
        && proxy_username.is_none()
        && proxy_password.is_none()
        && endpoint.is_none()
    {
        return Err(bad_request_response("至少提供一个可更新字段"));
    }

    Ok(
        aether_data::repository::proxy_nodes::ProxyNodeManualUpdateMutation {
            node_id,
            name,
            ip: endpoint.as_ref().map(|value| value.node_ip.clone()),
            port: endpoint.as_ref().map(|value| value.node_port),
            region,
            proxy_url: endpoint.map(|value| value.proxy_url),
            proxy_username,
            proxy_password,
        },
    )
}

fn validate_proxy_test_url_request(
    input: ProxyNodeTestUrlRequest,
) -> Result<String, Response<Body>> {
    let username = normalize_optional_string(input.username.as_deref(), "username", 255)?;
    let password = normalize_optional_string(input.password.as_deref(), "password", 500)?;
    let endpoint = normalize_manual_proxy_endpoint(&input.proxy_url)?;
    proxy_url_with_auth(
        &endpoint.proxy_url,
        username.as_deref(),
        password.as_deref(),
    )
    .ok_or_else(|| bad_request_response("password 需要非空 username，且代理 URL 必须支持认证"))
}

fn admin_proxy_node_upgrade_action_node_id_from_path(path: &str, suffix: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let node_id = normalized.strip_prefix("/api/admin/proxy-nodes/")?;
    let node_id = node_id.strip_suffix(suffix)?;
    if node_id.is_empty() || node_id.contains('/') {
        None
    } else {
        Some(node_id.to_string())
    }
}

fn admin_proxy_node_node_id_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let node_id = normalized.strip_prefix("/api/admin/proxy-nodes/")?;
    if node_id.is_empty() || node_id.contains('/') {
        None
    } else {
        Some(node_id.to_string())
    }
}

fn admin_proxy_node_test_node_id_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let node_id = normalized.strip_prefix("/api/admin/proxy-nodes/")?;
    let node_id = node_id.strip_suffix("/test")?;
    if node_id.is_empty() || node_id.contains('/') {
        None
    } else {
        Some(node_id.to_string())
    }
}

fn normalize_proxy_upgrade_version(value: &str) -> String {
    value
        .trim()
        .strip_prefix("tunnel-v")
        .or_else(|| value.trim().strip_prefix("proxy-v"))
        .unwrap_or(value.trim())
        .to_ascii_lowercase()
}

async fn dispatch_proxy_node_upgrade_targets(
    state: &AdminAppState<'_>,
    version: &str,
) -> Result<ProxyNodeBatchUpgradeDispatchSummary, GatewayError> {
    let mut nodes = state.list_proxy_nodes().await?;
    nodes.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));

    let rollout_cancelled = if state.app().data.has_system_config_store() {
        cancel_proxy_upgrade_rollout(&state.app().data)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .is_some()
    } else {
        false
    };

    let normalized_target = normalize_proxy_upgrade_version(version);
    let mut summary = ProxyNodeBatchUpgradeDispatchSummary {
        version: version.to_string(),
        rollout_cancelled,
        ..Default::default()
    };

    for node in nodes {
        if node.is_manual || !node.tunnel_mode {
            continue;
        }

        summary.eligible_total = summary.eligible_total.saturating_add(1);

        let current_version = aether_data::repository::proxy_nodes::proxy_reported_version(
            node.proxy_metadata.as_ref(),
        );
        let pending_target = aether_data::repository::proxy_nodes::remote_config_upgrade_target(
            node.remote_config.as_ref(),
        );
        if pending_target.as_deref() == Some(normalized_target.as_str())
            || current_version.as_deref() == Some(normalized_target.as_str())
        {
            continue;
        }

        let Some(updated) = state
            .update_proxy_node_remote_config(
                &aether_data::repository::proxy_nodes::ProxyNodeRemoteConfigMutation {
                    node_id: node.id.clone(),
                    expected_tunnel_generation: None,
                    node_name: None,
                    allowed_ports: None,
                    log_level: None,
                    heartbeat_interval: None,
                    scheduling_state: None,
                    upgrade_to: Some(Some(version.to_string())),
                },
            )
            .await?
        else {
            continue;
        };
        summary.node_ids.push(updated.id);
    }

    summary.updated = summary.node_ids.len();
    summary.skipped = summary.eligible_total.saturating_sub(summary.updated);
    Ok(summary)
}

fn validate_batch_size(batch_size: Option<usize>) -> Result<usize, Response<Body>> {
    let batch_size = batch_size.unwrap_or(DEFAULT_PROXY_UPGRADE_BATCH_SIZE);
    if (1..=100).contains(&batch_size) {
        Ok(batch_size)
    } else {
        Err(bad_request_response("batch_size 必须在 1 到 100 之间"))
    }
}

fn validate_cooldown_secs(cooldown_secs: Option<u64>) -> Result<u64, Response<Body>> {
    let cooldown_secs = cooldown_secs.unwrap_or(DEFAULT_PROXY_UPGRADE_COOLDOWN_SECS);
    if cooldown_secs <= 3600 {
        Ok(cooldown_secs)
    } else {
        Err(bad_request_response("cooldown_secs 不能超过 3600"))
    }
}

fn validate_probe_config(
    probe_url: Option<&str>,
    probe_timeout_secs: Option<u64>,
) -> Result<Option<ProxyUpgradeRolloutProbeConfig>, Response<Body>> {
    let Some(probe_url) = probe_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed = reqwest::Url::parse(probe_url)
        .map_err(|_| bad_request_response("probe_url 必须是合法的 http/https URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(bad_request_response("probe_url 仅支持 http 或 https"));
    }
    if parsed.as_str().len() > 2048 {
        return Err(bad_request_response("probe_url 长度不能超过 2048"));
    }
    let timeout_secs = probe_timeout_secs.unwrap_or(DEFAULT_PROXY_UPGRADE_PROBE_TIMEOUT_SECS);
    if !(5..=60).contains(&timeout_secs) {
        return Err(bad_request_response(
            "probe_timeout_secs 必须在 5 到 60 秒之间",
        ));
    }
    Ok(Some(ProxyUpgradeRolloutProbeConfig {
        url: parsed.to_string(),
        timeout_secs,
    }))
}

fn validate_heartbeat_request(
    input: ProxyNodeHeartbeatRequest,
) -> Result<aether_data::repository::proxy_nodes::ProxyNodeHeartbeatMutation, Response<Body>> {
    let node_id = validate_node_id(&input.node_id)?;
    if let Some(interval) = input.heartbeat_interval {
        validate_optional_i32_range(interval, "heartbeat_interval", 5, 600)?;
    }
    validate_optional_counter(
        input.active_connections.map(i64::from),
        "active_connections",
    )?;
    validate_optional_counter(input.total_requests, "total_requests")?;
    validate_optional_counter(input.failed_requests, "failed_requests")?;
    validate_optional_counter(input.dns_failures, "dns_failures")?;
    validate_optional_counter(input.stream_errors, "stream_errors")?;
    if input
        .avg_latency_ms
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(bad_request_response("avg_latency_ms 必须是非负有限数值"));
    }
    validate_optional_object(input.proxy_metadata.as_ref(), "proxy_metadata")?;

    Ok(
        aether_data::repository::proxy_nodes::ProxyNodeHeartbeatMutation {
            node_id,
            expected_tunnel_generation: None,
            heartbeat_interval: input.heartbeat_interval,
            active_connections: input.active_connections,
            total_requests_delta: input.total_requests,
            avg_latency_ms: input.avg_latency_ms,
            failed_requests_delta: input.failed_requests,
            dns_failures_delta: input.dns_failures,
            stream_errors_delta: input.stream_errors,
            proxy_metadata: input.proxy_metadata,
            proxy_version: normalize_optional_string(
                input.proxy_version.as_deref(),
                "proxy_version",
                20,
            )?,
        },
    )
}

fn validate_remote_config_request(
    node_id: String,
    raw: &serde_json::Map<String, Value>,
) -> Result<aether_data::repository::proxy_nodes::ProxyNodeRemoteConfigMutation, Response<Body>> {
    let node_name = match raw.get("node_name") {
        Some(Value::Null) | None => None,
        Some(Value::String(value)) => Some(normalize_required_string(value, "node_name", 100)?),
        Some(_) => return Err(bad_request_response("node_name 必须是字符串")),
    };

    let allowed_ports = match raw.get("allowed_ports") {
        Some(Value::Null) | None => None,
        Some(Value::Array(items)) => {
            let mut ports = Vec::with_capacity(items.len());
            for item in items {
                let Some(port) = item.as_u64() else {
                    return Err(bad_request_response("allowed_ports 必须是端口数字数组"));
                };
                if !(1..=65535).contains(&port) {
                    return Err(bad_request_response("allowed_ports 仅支持 1-65535"));
                }
                ports.push(port as u16);
            }
            Some(ports)
        }
        Some(_) => return Err(bad_request_response("allowed_ports 必须是端口数字数组")),
    };

    let log_level = match raw.get("log_level") {
        Some(Value::Null) | None => None,
        Some(Value::String(value)) => {
            let normalized = normalize_required_string(value, "log_level", 16)?;
            if !matches!(
                normalized.as_str(),
                "trace" | "debug" | "info" | "warn" | "error"
            ) {
                return Err(bad_request_response(
                    "log_level 必须是 trace/debug/info/warn/error 之一",
                ));
            }
            Some(normalized)
        }
        Some(_) => return Err(bad_request_response("log_level 必须是字符串")),
    };

    let heartbeat_interval = match raw.get("heartbeat_interval") {
        Some(Value::Null) | None => None,
        Some(value) => Some(validate_json_i32_range(
            value,
            "heartbeat_interval",
            5,
            600,
        )?),
    };

    let scheduling_state = if raw.contains_key("scheduling_state") {
        match raw.get("scheduling_state") {
            Some(Value::Null) | None => Some(None),
            Some(Value::String(value)) => {
                let normalized = normalize_required_string(value, "scheduling_state", 16)?;
                match normalized.as_str() {
                    "active" => Some(None),
                    "draining" | "cordoned" => Some(Some(normalized)),
                    _ => {
                        return Err(bad_request_response(
                            "scheduling_state 必须是 active/draining/cordoned 之一",
                        ));
                    }
                }
            }
            Some(_) => return Err(bad_request_response("scheduling_state 必须是字符串或 null")),
        }
    } else {
        None
    };

    let upgrade_to = if raw.contains_key("upgrade_to") {
        match raw.get("upgrade_to") {
            Some(Value::Null) | None => Some(None),
            Some(Value::String(value)) => {
                let normalized = value.trim();
                if normalized.is_empty() {
                    Some(None)
                } else {
                    Some(Some(validate_version(normalized)?))
                }
            }
            Some(_) => return Err(bad_request_response("upgrade_to 必须是字符串或 null")),
        }
    } else {
        None
    };

    Ok(
        aether_data::repository::proxy_nodes::ProxyNodeRemoteConfigMutation {
            node_id,
            expected_tunnel_generation: None,
            node_name,
            allowed_ports,
            log_level,
            heartbeat_interval,
            scheduling_state,
            upgrade_to,
        },
    )
}

fn admin_proxy_node_config_node_id_from_path(path: &str) -> Option<String> {
    let value = path
        .strip_prefix("/api/admin/proxy-nodes/")?
        .strip_suffix("/config")?;
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_json_body<T: DeserializeOwned>(request_body: Option<&Bytes>) -> Result<T, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(bad_request_response("请求体不能为空"));
    };
    let raw_value = serde_json::from_slice::<Value>(request_body)
        .map_err(|_| bad_request_response(JSON_OBJECT_REQUIRED_DETAIL))?;
    serde_json::from_value::<T>(raw_value)
        .map_err(|_| bad_request_response(JSON_OBJECT_REQUIRED_DETAIL))
}

fn parse_json_object_body(
    request_body: Option<&Bytes>,
) -> Result<serde_json::Map<String, Value>, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(bad_request_response("请求体不能为空"));
    };
    let raw_value = serde_json::from_slice::<Value>(request_body)
        .map_err(|_| bad_request_response(JSON_OBJECT_REQUIRED_DETAIL))?;
    raw_value
        .as_object()
        .cloned()
        .ok_or_else(|| bad_request_response(JSON_OBJECT_REQUIRED_DETAIL))
}

fn normalize_manual_proxy_endpoint(
    proxy_url: &str,
) -> Result<NormalizedManualProxyEndpoint, Response<Body>> {
    parse_manual_proxy_endpoint(proxy_url, "proxy_url").map_err(bad_request_response)
}

fn parse_manual_proxy_endpoint(
    proxy_url: &str,
    field: &str,
) -> Result<NormalizedManualProxyEndpoint, String> {
    let proxy_url = proxy_url.trim();
    if proxy_url.is_empty() {
        return Err(format!("{field} 不能为空"));
    }
    if proxy_url.chars().count() > 500 {
        return Err(format!("{field} 长度不能超过 500"));
    }

    let parsed =
        reqwest::Url::parse(proxy_url).map_err(|_| format!("{field} 必须是合法的代理 URL"))?;
    let scheme = parsed.scheme().trim().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "socks5" | "socks5h") {
        return Err(format!("{field} 仅支持 http/https/socks5/socks5h 协议"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("{field} 不应包含用户名或密码，请使用独立字段"));
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some()
    {
        return Err(format!(
            "{field} 必须是代理 origin，不能包含 path、query 或 fragment"
        ));
    }
    let host = parsed
        .host_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field} 缺少主机地址"))?
        .to_string();
    let port = parsed.port().unwrap_or(match scheme.as_str() {
        "https" => 443,
        "socks5" | "socks5h" => 1080,
        _ => 80,
    });
    let node_ip = if scheme == "http" {
        host.clone()
    } else {
        format!("{scheme}://{host}")
    };
    if node_ip.chars().count() > 255 {
        return Err("代理主机标识长度不能超过 255".to_string());
    }

    Ok(NormalizedManualProxyEndpoint {
        proxy_url: proxy_url.to_string(),
        host,
        port,
        node_ip,
        node_port: i32::from(port),
    })
}

fn validate_node_id(value: &str) -> Result<String, Response<Body>> {
    normalize_required_string(value, "node_id", 36)
}

fn validate_version(value: &str) -> Result<String, Response<Body>> {
    normalize_required_string(value, "version", 50)
}

fn normalize_required_string(
    value: &str,
    field: &str,
    max_len: usize,
) -> Result<String, Response<Body>> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(bad_request_response(format!("{field} 不能为空")));
    }
    if normalized.chars().count() > max_len {
        return Err(bad_request_response(format!(
            "{field} 长度不能超过 {max_len}"
        )));
    }
    Ok(normalized.to_string())
}

fn normalize_optional_string(
    value: Option<&str>,
    field: &str,
    max_len: usize,
) -> Result<Option<String>, Response<Body>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let normalized = value.trim();
    if normalized.is_empty() {
        return Ok(None);
    }
    if normalized.chars().count() > max_len {
        return Err(bad_request_response(format!(
            "{field} 长度不能超过 {max_len}"
        )));
    }
    Ok(Some(normalized.to_string()))
}

fn normalize_ip_address(value: &str) -> Result<String, Response<Body>> {
    let normalized = value.trim();
    normalized
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.to_string())
        .map_err(|_| bad_request_response("ip 必须是合法的 IPv4/IPv6 地址"))
}

fn sanitize_proxy_error(detail: &str) -> String {
    const HTTP_STATUS_PREFIX: &str = "代理探测返回 HTTP ";
    const CLASSIFICATION_PREFIX_BYTES: usize = 4 * 1024;

    if let Some(status) = detail
        .strip_prefix(HTTP_STATUS_PREFIX)
        .and_then(|rest| {
            rest.split(|character: char| !character.is_ascii_digit())
                .next()
        })
        .and_then(|status| status.parse::<u16>().ok())
        .filter(|status| (100..=599).contains(status))
    {
        return format!("{HTTP_STATUS_PREFIX}{status}");
    }

    let detail = if detail.len() <= CLASSIFICATION_PREFIX_BYTES {
        detail
    } else {
        let mut end = CLASSIFICATION_PREFIX_BYTES;
        while !detail.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        &detail[..end]
    };
    let normalized = detail.to_ascii_lowercase();
    if normalized.contains("timed out") || normalized.contains("timeout") {
        return "代理探测超时".to_string();
    }
    if normalized.contains("overloaded")
        || normalized.contains("backpressure")
        || normalized.contains("congested")
        || normalized.contains("busy")
    {
        return "代理探测服务繁忙".to_string();
    }
    if normalized.contains("unauthorized")
        || normalized.contains("forbidden")
        || normalized.contains("authentication")
        || normalized.contains("credential")
    {
        return "代理认证失败".to_string();
    }
    if normalized.contains("too large")
        || normalized.contains("body exceeds")
        || normalized.contains("response exceeds")
    {
        return "代理探测响应过大".to_string();
    }
    if normalized.contains("response body")
        || normalized.contains("body read")
        || normalized.contains("decode")
    {
        return "代理探测响应读取失败".to_string();
    }
    if normalized.contains("connect")
        || normalized.contains("dns")
        || normalized.contains("socket")
        || normalized.contains("not connected")
        || normalized.contains("unavailable")
        || normalized.contains("offline")
    {
        return "代理连接失败".to_string();
    }

    // Error strings can originate in reqwest, a remote gateway, or a tunnel
    // peer. Keep arbitrary URLs, credentials, paths, and control characters
    // out of the admin response by projecting unknown details to one category.
    "代理探测失败".to_string()
}

fn proxy_url_with_auth(
    proxy_url: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Option<String> {
    let username = username.filter(|value| !value.is_empty());
    let password = password.filter(|value| !value.is_empty());
    let mut parsed = url::Url::parse(proxy_url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h")
        || parsed.host_str().is_none()
    {
        return None;
    }
    if username.is_none() && password.is_none() {
        return Some(parsed.to_string());
    }
    let username = username.unwrap_or("");
    if parsed.set_username(username).is_err() {
        return None;
    }

    if parsed.set_password(password).is_err() {
        return None;
    }
    Some(parsed.to_string())
}

fn parse_proxy_probe_exit_ip(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if key.trim() != "ip" {
            return None;
        }
        value
            .trim()
            .parse::<std::net::IpAddr>()
            .ok()
            .map(|ip| ip.to_string())
    })
}

fn format_proxy_probe_status_error(status: reqwest::StatusCode, _body: &str) -> String {
    // The body is controlled by the probe target and may echo proxy
    // credentials or contain private upstream diagnostics.
    format!("代理探测返回 HTTP {}", status.as_u16())
}

fn validate_optional_counter(value: Option<i64>, field: &str) -> Result<(), Response<Body>> {
    if value.is_some_and(|value| value < 0) {
        return Err(bad_request_response(format!("{field} 必须是非负整数")));
    }
    Ok(())
}

fn validate_optional_i32_range(
    value: i32,
    field: &str,
    min: i32,
    max: i32,
) -> Result<i32, Response<Body>> {
    if !(min..=max).contains(&value) {
        return Err(bad_request_response(format!(
            "{field} 必须在 {min}-{max} 范围内"
        )));
    }
    Ok(value)
}

fn validate_json_i32_range(
    value: &Value,
    field: &str,
    min: i32,
    max: i32,
) -> Result<i32, Response<Body>> {
    let Some(raw) = value.as_i64() else {
        return Err(bad_request_response(format!("{field} 必须是整数")));
    };
    let parsed =
        i32::try_from(raw).map_err(|_| bad_request_response(format!("{field} 超出范围")))?;
    validate_optional_i32_range(parsed, field, min, max)
}

fn validate_optional_object(value: Option<&Value>, field: &str) -> Result<(), Response<Body>> {
    if value.is_some_and(|value| !value.is_object()) {
        return Err(bad_request_response(format!("{field} 必须是 JSON 对象")));
    }
    Ok(())
}

fn validate_proxy_install_node_name(value: &str) -> Result<String, Response<Body>> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 100 {
        return Err(bad_request_response(
            "节点名称不能为空，且不能超过 100 个字符",
        ));
    }
    Ok(trimmed.to_string())
}

fn generate_proxy_install_management_token_plaintext() -> String {
    generate_gateway_secret_plaintext("ae", "-")
}

fn hash_proxy_install_management_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn proxy_install_management_token_prefix(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.chars().take(12).collect())
}

async fn create_proxy_install_management_token(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    node_name: &str,
) -> Result<(StoredManagementToken, String), Response<Body>> {
    let Some(principal) = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
    else {
        return Err((
            http::StatusCode::UNAUTHORIZED,
            Json(json!({ "detail": "未认证管理员" })),
        )
            .into_response());
    };

    let (allowed_ips, expires_at_unix_secs) =
        if let Some(parent_token_id) = principal.management_token_id.as_deref() {
            let parent = match state.get_management_token_with_user(parent_token_id).await {
                Ok(Some(parent)) => parent,
                Ok(None) => return Err(proxy_install_parent_token_denied_response()),
                Err(_) => {
                    return Err(proxy_install_internal_error_response(
                        "proxy_install_parent_management_token_lookup",
                        "repository_lookup_failed",
                    ))
                }
            };
            let now = chrono::Utc::now().timestamp().max(0) as u64;
            if parent.token.user_id != principal.user_id
                || !parent.token.is_active
                || parent
                    .token
                    .expires_at_unix_secs
                    .is_some_and(|expires_at| expires_at <= now)
            {
                return Err(proxy_install_parent_token_denied_response());
            }
            let permissions = crate::control::management_token_permission_keys_from_value(
                parent.token.permissions.as_ref(),
            )
            .map_err(|_| proxy_install_parent_token_denied_response())?;
            let Some(decision) = request_context.decision() else {
                return Err(proxy_install_parent_token_denied_response());
            };
            crate::control::validate_management_token_admin_route_permission(
                request_context.method(),
                decision,
                permissions.as_deref(),
            )
            .map_err(|_| proxy_install_parent_token_denied_response())?;
            (
                parent.token.allowed_ips.clone(),
                parent.token.expires_at_unix_secs,
            )
        } else {
            (None, None)
        };

    let user = match state.app().find_user_auth_by_id(&principal.user_id).await {
        Ok(value) => value,
        Err(_) => {
            return Err(proxy_install_internal_error_response(
                "proxy_install_admin_user_lookup",
                "repository_lookup_failed",
            ))
        }
    };
    let Some(user) = user else {
        return Err(proxy_install_parent_token_denied_response());
    };
    if !user.is_active || user.is_deleted || !user.role.eq_ignore_ascii_case("admin") {
        return Err(proxy_install_parent_token_denied_response());
    }
    let user = StoredManagementTokenUserSummary::new(user.id, user.email, user.username, user.role)
        .map_err(|_| {
            proxy_install_internal_error_response(
                "proxy_install_management_token_user_summary_build",
                "invalid_user_summary",
            )
        })?;

    let raw_token = generate_proxy_install_management_token_plaintext();
    let short_id = Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();
    let record = CreateManagementTokenRecord {
        id: Uuid::new_v4().to_string(),
        user_id: user.id.clone(),
        user,
        token_hash: hash_proxy_install_management_token(&raw_token),
        token_prefix: proxy_install_management_token_prefix(&raw_token),
        name: format!("aether-tunnel {node_name} {short_id}"),
        description: Some("Created by proxy node one-click installer".to_string()),
        allowed_ips,
        permissions: Some(json!(["admin:proxy_nodes:write"])),
        expires_at_unix_secs,
        // The bearer secret is not usable until its one-time install session is consumed.
        is_active: false,
    };

    match state.app().create_management_token(&record).await {
        Ok(LocalMutationOutcome::Applied(stored)) => Ok((stored, raw_token)),
        Ok(LocalMutationOutcome::Invalid(detail)) => Err(bad_request_response(detail)),
        Ok(LocalMutationOutcome::Unavailable) => {
            Err(build_admin_proxy_nodes_data_unavailable_response())
        }
        Ok(LocalMutationOutcome::NotFound) => Err((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "管理员不存在" })),
        )
            .into_response()),
        Err(_) => Err(proxy_install_internal_error_response(
            "proxy_install_management_token_create",
            "repository_write_failed",
        )),
    }
}

fn proxy_install_internal_error_response(
    operation: &'static str,
    error_category: &'static str,
) -> Response<Body> {
    warn!(
        event_name = "proxy_install_internal_error",
        operation, error_category, "proxy install operation failed"
    );
    (
        http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "detail": PROXY_INSTALL_INTERNAL_ERROR_DETAIL })),
    )
        .into_response()
}

fn proxy_install_parent_token_denied_response() -> Response<Body> {
    (
        http::StatusCode::FORBIDDEN,
        Json(json!({
            "detail": "parent management token is no longer authorized to create install sessions"
        })),
    )
        .into_response()
}

fn parse_proxy_node_event_query(
    query: Option<&str>,
) -> Result<ProxyNodeEventQuery, Response<Body>> {
    let limit = query_param_value(query, "limit")
        .map(|value| parse_query_u64("limit", &value))
        .transpose()?
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0 && *value <= 200)
        .unwrap_or(50);
    let from_unix_secs = query_param_value(query, "from")
        .map(|value| parse_query_u64("from", &value))
        .transpose()?;
    let to_unix_secs = query_param_value(query, "to")
        .map(|value| parse_query_u64("to", &value))
        .transpose()?;
    if from_unix_secs
        .zip(to_unix_secs)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(bad_request_response("from 不能大于 to"));
    }
    let event_type = query_param_value(query, "event_type")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());

    Ok(ProxyNodeEventQuery {
        limit,
        from_unix_secs,
        to_unix_secs,
        event_type,
    })
}

fn parse_proxy_node_metrics_query(
    query: Option<&str>,
) -> Result<(ProxyNodeMetricsStep, u64, u64, usize), Response<Body>> {
    let step = match query_param_value(query, "step")
        .unwrap_or_else(|| "1m".to_string())
        .trim()
    {
        "1m" => ProxyNodeMetricsStep::OneMinute,
        "1h" => ProxyNodeMetricsStep::OneHour,
        _ => return Err(bad_request_response("step 仅支持 1m 或 1h")),
    };
    let from_unix_secs = query_param_value(query, "from")
        .ok_or_else(|| bad_request_response("from 为必填 Unix 秒时间戳"))?;
    let from_unix_secs = parse_query_u64("from", &from_unix_secs)?;
    let to_unix_secs = query_param_value(query, "to")
        .ok_or_else(|| bad_request_response("to 为必填 Unix 秒时间戳"))?;
    let to_unix_secs = parse_query_u64("to", &to_unix_secs)?;
    if from_unix_secs > to_unix_secs {
        return Err(bad_request_response("from 不能大于 to"));
    }

    let window_secs = to_unix_secs.saturating_sub(from_unix_secs);
    let max_window_secs = match step {
        ProxyNodeMetricsStep::OneMinute => PROXY_NODE_METRICS_1M_MAX_WINDOW_SECS,
        ProxyNodeMetricsStep::OneHour => PROXY_NODE_METRICS_1H_MAX_WINDOW_SECS,
    };
    if window_secs > max_window_secs {
        return Err(bad_request_response(match step {
            ProxyNodeMetricsStep::OneMinute => "1m 最大查询窗口为 30 天",
            ProxyNodeMetricsStep::OneHour => "1h 最大查询窗口为 365 天",
        }));
    }

    let points = window_secs / step.bucket_size_secs() + 1;
    let limit = usize::try_from(points)
        .ok()
        .filter(|value| *value > 0 && *value <= PROXY_NODE_METRICS_MAX_POINTS)
        .ok_or_else(|| bad_request_response("查询点数过多"))?;
    Ok((step, from_unix_secs, to_unix_secs, limit))
}

fn parse_query_u64(field: &str, value: &str) -> Result<u64, Response<Body>> {
    value
        .parse::<u64>()
        .map_err(|_| bad_request_response(format!("{field} 必须是非负 Unix 秒时间戳")))
}

fn bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_auth_url_construction_never_falls_back_to_unauthenticated() {
        assert_eq!(
            proxy_url_with_auth("http://proxy.example:8080", None, None).as_deref(),
            Some("http://proxy.example:8080/")
        );
        assert_eq!(
            proxy_url_with_auth("http://proxy.example:8080", None, Some("secret")).as_deref(),
            Some("http://:secret@proxy.example:8080/")
        );
        assert!(proxy_url_with_auth("not a proxy url", Some("alice"), Some("secret")).is_none());
        assert!(proxy_url_with_auth("mailto:proxy@example.com", Some("alice"), None).is_none());
    }

    #[test]
    fn manual_proxy_endpoint_accepts_only_an_origin_without_ambiguous_components() {
        for value in [
            "http://proxy.example:8080/path",
            "http://proxy.example:8080?token=secret",
            "http://proxy.example:8080#fragment",
            "http://alice:password@proxy.example:8080",
            "file:///tmp/proxy",
        ] {
            assert!(
                parse_manual_proxy_endpoint(value, "proxy_url").is_err(),
                "proxy URL should be rejected: {value}"
            );
        }

        for value in [
            "http://proxy.example:8080",
            "https://proxy.example:8443/",
            "socks5://proxy.example:1080",
            "socks5h://proxy.example:1080",
        ] {
            assert!(
                parse_manual_proxy_endpoint(value, "proxy_url").is_ok(),
                "proxy origin should be accepted: {value}"
            );
        }
    }

    #[test]
    fn proxy_connectivity_errors_are_projected_without_sensitive_details() {
        let details = [
            "request failed for https://alice:proxy-secret@10.0.0.8/probe?access_token=query-secret",
            "Bearer bearer-secret\r\nx-injected: true",
            "failed to open /private/var/proxy-secret.pem",
        ];

        for detail in details {
            let projected = sanitize_proxy_error(detail);
            for secret in [
                "alice",
                "proxy-secret",
                "10.0.0.8",
                "query-secret",
                "bearer-secret",
                "x-injected",
                "/private/var",
            ] {
                assert!(!projected.contains(secret), "leaked {secret}: {projected}");
            }
            assert!(!projected.contains(['\r', '\n']));
        }

        assert_eq!(
            sanitize_proxy_error("connection timed out for https://secret.internal"),
            "代理探测超时"
        );
        assert_eq!(
            sanitize_proxy_error("DNS connect error for http://10.0.0.2"),
            "代理连接失败"
        );
    }

    #[test]
    fn proxy_probe_status_error_does_not_echo_untrusted_body() {
        let detail = format_proxy_probe_status_error(
            reqwest::StatusCode::BAD_GATEWAY,
            "Bearer upstream-secret at http://10.0.0.9/private?token=query-secret",
        );

        assert_eq!(detail, "代理探测返回 HTTP 502");
        assert_eq!(sanitize_proxy_error(&detail), detail);
        assert!(!detail.contains("upstream-secret"));
        assert!(!detail.contains("10.0.0.9"));
    }

    #[test]
    fn proxy_probe_exit_ip_only_accepts_ip_addresses() {
        assert_eq!(
            parse_proxy_probe_exit_ip("fl=1\nip=2001:db8::1\nts=2"),
            Some("2001:db8::1".to_string())
        );
        assert_eq!(
            parse_proxy_probe_exit_ip("ip=Bearer upstream-secret\nx=1"),
            None
        );
    }

    #[tokio::test]
    async fn proxy_install_internal_error_response_hides_internal_details() {
        let response = proxy_install_internal_error_response(
            "secret-bearing operation https://internal.example/token",
            "Bearer super-secret",
        );

        assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("internal error response body should be readable");
        let payload: Value =
            serde_json::from_slice(&body).expect("internal error response should be JSON");

        assert_eq!(
            payload,
            json!({ "detail": PROXY_INSTALL_INTERNAL_ERROR_DETAIL })
        );
        let body = String::from_utf8_lossy(&body);
        assert!(!body.contains("internal.example"));
        assert!(!body.contains("super-secret"));
    }
}
