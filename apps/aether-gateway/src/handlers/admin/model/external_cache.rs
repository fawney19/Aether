use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::mark_external_models_official_providers;
use crate::GatewayError;
use aether_contracts::{
    ExecutionPlan, ExecutionTimeouts, RequestBody, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
};
use aether_runtime_state::RuntimeLockLease;
use axum::body::Bytes;
use axum::http;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tracing::warn;

const ADMIN_EXTERNAL_MODELS_LEGACY_CACHE_KEY: &str = "aether:external:models_dev";
const ADMIN_EXTERNAL_MODELS_CACHE_KEY: &str = "aether:external:models_dev:v2";
const ADMIN_EXTERNAL_MODELS_CACHE_VERSION: u8 = 2;
const ADMIN_EXTERNAL_MODELS_CACHE_TTL_SECS: u64 = 15 * 60;
const ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV: &str = "AETHER_GATEWAY_EXTERNAL_MODELS_URL";
const ADMIN_EXTERNAL_MODELS_SOURCE_URL_DEFAULT: &str = "https://models.dev/api.json";
const ADMIN_EXTERNAL_MODELS_OFFICIAL_HOST: &str = "models.dev";
const ADMIN_EXTERNAL_MODELS_OFFICIAL_PATH: &str = "/api.json";
pub(in crate::handlers::admin) const ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY: &str =
    "external_models_proxy_node_id";
const ADMIN_EXTERNAL_MODELS_CONNECT_TIMEOUT_MS: u64 = 10_000;
const ADMIN_EXTERNAL_MODELS_TOTAL_TIMEOUT_MS: u64 = 30_000;
const ADMIN_EXTERNAL_MODELS_RESPONSE_LIMIT_BYTES: usize = 8 * 1024 * 1024;
// Keep the cache envelope bounded independently of the upstream body limit.
// Normalization adds a small amount of metadata, while a corrupted/shared
// runtime KV value must never be allowed to drive an unbounded serde
// allocation during cache reads.
const ADMIN_EXTERNAL_MODELS_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const ADMIN_EXTERNAL_MODELS_CONFIG_MUTATION_LOCK_KEY: &str =
    "admin:external_models_proxy_node_config:mutation";
const ADMIN_EXTERNAL_MODELS_CONFIG_MUTATION_LOCK_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdminExternalModelsCacheEnvelope {
    schema_version: u8,
    proxy_node_id: Option<String>,
    payload: Value,
}

#[derive(Debug)]
struct ResolvedAdminExternalModelsSource {
    url: url::Url,
    host: String,
    addresses: Vec<SocketAddr>,
}

#[cfg(test)]
pub(crate) struct AdminExternalModelsSourceUrlEnvGuard {
    previous: Option<String>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for AdminExternalModelsSourceUrlEnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.as_deref() {
            std::env::set_var(ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV, previous);
        } else {
            std::env::remove_var(ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV);
        }
    }
}

#[cfg(test)]
pub(crate) fn set_admin_external_models_source_url_for_tests(
    value: &str,
) -> AdminExternalModelsSourceUrlEnvGuard {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let lock = LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = std::env::var(ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV).ok();
    std::env::set_var(ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV, value);
    AdminExternalModelsSourceUrlEnvGuard {
        previous,
        _lock: lock,
    }
}

fn admin_external_models_source_url() -> String {
    std::env::var(ADMIN_EXTERNAL_MODELS_SOURCE_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ADMIN_EXTERNAL_MODELS_SOURCE_URL_DEFAULT.to_string())
}

fn parse_admin_external_models_source_url(
    raw_url: &str,
    allow_insecure_test_target: bool,
) -> Result<(url::Url, String, u16), GatewayError> {
    let url = url::Url::parse(raw_url)
        .map_err(|_| GatewayError::Internal("external models source URL is invalid".to_string()))?;
    let allowed_scheme =
        url.scheme() == "https" || (allow_insecure_test_target && url.scheme() == "http");
    if !allowed_scheme
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GatewayError::Internal(
            "external models source must be an HTTPS URL without credentials, query, or fragment"
                .to_string(),
        ));
    }
    let host = url.host_str().map(ToOwned::to_owned).ok_or_else(|| {
        GatewayError::Internal("external models source is missing a host".to_string())
    })?;
    let port = url.port_or_known_default().ok_or_else(|| {
        GatewayError::Internal("external models source is missing a port".to_string())
    })?;
    Ok((url, host, port))
}

fn validate_admin_external_models_source_addresses(
    url: &url::Url,
    addresses: &[SocketAddr],
    allow_insecure_test_target: bool,
) -> Result<(), GatewayError> {
    if addresses.is_empty() {
        return Err(GatewayError::Internal(
            "external models source DNS resolution returned no addresses".to_string(),
        ));
    }
    if !allow_insecure_test_target
        && addresses.iter().any(|address| {
            aether_http::is_private_or_reserved_ip(address.ip())
                && !(is_official_external_models_catalog_url(url)
                    && aether_http::is_ipv4_benchmarking_fake_ip(address.ip()))
        })
    {
        return Err(GatewayError::Internal(
            "external models source resolves to a private or reserved address".to_string(),
        ));
    }
    Ok(())
}

fn is_official_external_models_catalog_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(ADMIN_EXTERNAL_MODELS_OFFICIAL_HOST))
        && url.port_or_known_default() == Some(443)
        && url.path() == ADMIN_EXTERNAL_MODELS_OFFICIAL_PATH
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

async fn resolve_admin_external_models_source(
    raw_url: &str,
    allow_insecure_test_target: bool,
) -> Result<ResolvedAdminExternalModelsSource, GatewayError> {
    let (url, host, port) =
        parse_admin_external_models_source_url(raw_url, allow_insecure_test_target)?;
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        aether_http::lookup_host_with_limits(
            host.as_str(),
            port,
            aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
        )
        .await
        .map_err(|_| {
            GatewayError::Internal("external models source DNS resolution failed".to_string())
        })?
    };
    validate_admin_external_models_source_addresses(&url, &addresses, allow_insecure_test_target)?;
    Ok(ResolvedAdminExternalModelsSource {
        url,
        host,
        addresses,
    })
}

fn build_admin_external_models_direct_client(
    source: &ResolvedAdminExternalModelsSource,
) -> Result<reqwest::Client, GatewayError> {
    let mut builder = aether_http::apply_http_client_config(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none()),
        &aether_http::HttpClientConfig {
            connect_timeout_ms: Some(ADMIN_EXTERNAL_MODELS_CONNECT_TIMEOUT_MS),
            request_timeout_ms: Some(ADMIN_EXTERNAL_MODELS_TOTAL_TIMEOUT_MS),
            http2_adaptive_window: true,
            ..aether_http::HttpClientConfig::default()
        },
    );
    if source.host.parse::<IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&source.host, &source.addresses);
    }
    builder.build().map_err(|_| {
        GatewayError::Internal("external models HTTP client initialization failed".to_string())
    })
}

fn normalize_admin_external_models_payload(payload: serde_json::Value) -> serde_json::Value {
    mark_external_models_official_providers(&payload).unwrap_or(payload)
}

fn classify_admin_external_models_transport_error(message: &str) -> &'static str {
    let message = message.to_ascii_lowercase();
    if message.contains("dns resolution")
        || message.contains("dns lookup")
        || (message.contains("resolve") && message.contains("host"))
    {
        "dns_resolution"
    } else if message.contains("private or reserved")
        || message.contains("ssrf")
        || message.contains("address policy")
    {
        "ssrf_blocked"
    } else if message.contains("invalid") && message.contains("url") {
        "invalid_url"
    } else if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else if message.contains("relay") || message.contains("tunnel") {
        "relay"
    } else if message.contains("proxy") {
        "proxy_config"
    } else if message.contains("too large") || message.contains("exceeds") {
        "response_too_large"
    } else if message.contains("json") {
        "invalid_json"
    } else if message.contains("decode") || message.contains("content-encoding") {
        "response_decode"
    } else if message.contains("connect") || message.contains("dns") || message.contains("tcp") {
        "connect"
    } else if message.contains("source returned http") || message.contains("status ") {
        "upstream_http"
    } else if message.contains("header") || message.contains("method") || message.contains("build")
    {
        "request_build"
    } else {
        "unknown_transport"
    }
}

async fn store_admin_external_models_cache(
    state: &AdminAppState<'_>,
    proxy_node_id: Option<&str>,
    payload: &serde_json::Value,
) -> Result<(), GatewayError> {
    let envelope = AdminExternalModelsCacheEnvelope {
        schema_version: ADMIN_EXTERNAL_MODELS_CACHE_VERSION,
        proxy_node_id: proxy_node_id.map(ToOwned::to_owned),
        payload: payload.clone(),
    };
    let serialized =
        serde_json::to_string(&envelope).map_err(|err| GatewayError::Internal(err.to_string()))?;
    if serialized.len() > ADMIN_EXTERNAL_MODELS_CACHE_MAX_BYTES {
        return Err(GatewayError::Internal(
            "external models cache envelope exceeds the allowed size".to_string(),
        ));
    }
    state
        .as_ref()
        .runtime_kv_setex(
            ADMIN_EXTERNAL_MODELS_CACHE_KEY,
            &serialized,
            ADMIN_EXTERNAL_MODELS_CACHE_TTL_SECS,
        )
        .await?;
    Ok(())
}

fn parse_admin_external_models_cache(raw: &str) -> Option<AdminExternalModelsCacheEnvelope> {
    if raw.len() > ADMIN_EXTERNAL_MODELS_CACHE_MAX_BYTES {
        return None;
    }
    serde_json::from_str::<AdminExternalModelsCacheEnvelope>(raw).ok()
}

fn normalize_admin_external_models_proxy_node_id(
    value: Option<&Value>,
) -> Result<Option<String>, GatewayError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Err(GatewayError::Internal(format!(
                    "system config '{ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY}' must not be empty"
                )))
            } else {
                Ok(Some(value.to_string()))
            }
        }
        Some(_) => Err(GatewayError::Internal(format!(
            "system config '{ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY}' must be a string or null"
        ))),
    }
}

async fn read_admin_external_models_proxy_node_id(
    state: &AdminAppState<'_>,
) -> Result<Option<String>, GatewayError> {
    let value = state
        .read_system_config_json_value_strong(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY)
        .await?;
    normalize_admin_external_models_proxy_node_id(value.as_ref())
}

pub(crate) async fn build_admin_external_models_config_payload(
    state: &AdminAppState<'_>,
) -> Result<Value, GatewayError> {
    let proxy_node_id = read_admin_external_models_proxy_node_id(state).await?;
    Ok(json!({
        "proxy_node_id": proxy_node_id,
    }))
}

fn parse_admin_external_models_config_update(
    request_body: &[u8],
) -> Result<Option<String>, (http::StatusCode, Value)> {
    let payload = match serde_json::from_slice::<Value>(request_body) {
        Ok(Value::Object(payload)) => payload,
        Ok(_) | Err(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };
    match payload.get("proxy_node_id") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "proxy_node_id 不能为空" }),
                ));
            }
            Ok(Some(value.to_string()))
        }
        Some(_) | None => Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": "proxy_node_id 必须是字符串或 null" }),
        )),
    }
}

async fn clear_admin_external_models_cache_entries(
    state: &AdminAppState<'_>,
) -> Result<bool, GatewayError> {
    let cleared_current = state
        .as_ref()
        .runtime_kv_del(ADMIN_EXTERNAL_MODELS_CACHE_KEY)
        .await?;
    let cleared_legacy = state
        .as_ref()
        .runtime_kv_del(ADMIN_EXTERNAL_MODELS_LEGACY_CACHE_KEY)
        .await?;
    Ok(cleared_current || cleared_legacy)
}

pub(crate) async fn acquire_admin_external_models_config_mutation_lock(
    state: &AdminAppState<'_>,
) -> Result<RuntimeLockLease, (http::StatusCode, Value)> {
    match state
        .app()
        .runtime_state()
        .lock_try_acquire(
            ADMIN_EXTERNAL_MODELS_CONFIG_MUTATION_LOCK_KEY,
            state.app().tunnel.local_instance_id(),
            ADMIN_EXTERNAL_MODELS_CONFIG_MUTATION_LOCK_TTL,
        )
        .await
    {
        Ok(Some(lock)) => Ok(lock),
        Ok(None) => Err((
            http::StatusCode::CONFLICT,
            json!({ "detail": "外部模型目录代理配置正在更新，请稍后重试" }),
        )),
        Err(_) => {
            warn!(
                runtime_backend = state.app().runtime_state_backend(),
                "failed to acquire external models proxy config mutation lock"
            );
            Err((
                http::StatusCode::SERVICE_UNAVAILABLE,
                json!({ "detail": "外部模型目录代理配置暂时无法更新" }),
            ))
        }
    }
}

pub(crate) async fn release_admin_external_models_config_mutation_lock(
    state: &AdminAppState<'_>,
    lock: &RuntimeLockLease,
) {
    if state
        .app()
        .runtime_state()
        .lock_release(lock)
        .await
        .is_err()
    {
        warn!(
            runtime_backend = state.app().runtime_state_backend(),
            "failed to release external models proxy config mutation lock"
        );
    }
}

async fn apply_admin_external_models_config_update_locked(
    state: &AdminAppState<'_>,
    proxy_node_id: Option<String>,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    if let Some(node_id) = proxy_node_id.as_deref() {
        if state.find_proxy_node(node_id).await?.is_none() {
            return Ok(Err((
                http::StatusCode::NOT_FOUND,
                json!({ "detail": format!("代理节点 '{node_id}' 不存在") }),
            )));
        }
        if state
            .resolve_admin_proxy_node_snapshot(Some(node_id))
            .await
            .is_none()
        {
            return Ok(Err((
                http::StatusCode::CONFLICT,
                json!({ "detail": format!("代理节点 '{node_id}' 当前不可用") }),
            )));
        }
    }

    let config_value = proxy_node_id
        .as_ref()
        .map_or(Value::Null, |node_id| json!(node_id));
    state
        .upsert_system_config_json_value(
            ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY,
            &config_value,
            Some("外部模型目录代理节点 ID"),
        )
        .await?;
    let cache_cleared = match clear_admin_external_models_cache_entries(state).await {
        Ok(cache_cleared) => cache_cleared,
        Err(_) => {
            // The v2 cache envelope includes the selected node ID, so a stale entry cannot be
            // reused after this persisted selector changes. Cache invalidation is best-effort.
            warn!(
                runtime_backend = state.app().runtime_state_backend(),
                "failed to clear external models cache after proxy config update"
            );
            false
        }
    };
    let mut payload = build_admin_external_models_config_payload(state).await?;
    payload["cache_cleared"] = json!(cache_cleared);
    Ok(Ok(payload))
}

pub(crate) async fn apply_admin_external_models_config_update(
    state: &AdminAppState<'_>,
    request_body: &Bytes,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    let proxy_node_id = match parse_admin_external_models_config_update(request_body) {
        Ok(proxy_node_id) => proxy_node_id,
        Err(error) => return Ok(Err(error)),
    };
    if !state.app().data.has_system_config_store() {
        return Ok(Err((
            http::StatusCode::SERVICE_UNAVAILABLE,
            json!({ "detail": "Admin system config data unavailable" }),
        )));
    }
    if proxy_node_id.is_some() && !state.has_proxy_node_reader() {
        return Ok(Err((
            http::StatusCode::SERVICE_UNAVAILABLE,
            json!({ "detail": "Admin proxy node data unavailable" }),
        )));
    }

    let lock = match acquire_admin_external_models_config_mutation_lock(state).await {
        Ok(lock) => lock,
        Err(error) => return Ok(Err(error)),
    };
    let result = apply_admin_external_models_config_update_locked(state, proxy_node_id).await;
    release_admin_external_models_config_mutation_lock(state, &lock).await;
    result
}

async fn fetch_admin_external_models_from_source(
    state: &AdminAppState<'_>,
    request_id: &str,
    proxy_node_id: Option<&str>,
) -> Result<serde_json::Value, GatewayError> {
    let source_url = admin_external_models_source_url();
    // A proxy/tunnel resolves the target in its own network namespace. Do not
    // resolve it locally first: local DNS may be unavailable, may intentionally
    // return synthetic addresses, or may not be able to see an internal target
    // that the configured proxy can reach. URL shape is still validated below,
    // and the direct path retains the local DNS validation/pinning guard.
    let parsed_source = parse_admin_external_models_source_url(&source_url, cfg!(test))?;
    if let Some(node_id) = proxy_node_id {
        let (source_url, _host, _port) = parsed_source;
        let Some(proxy) = state.resolve_admin_proxy_node_snapshot(Some(node_id)).await else {
            warn!(
                request_id = %request_id,
                proxy_node_id = %node_id,
                proxy_mode = "unknown",
                transport_error_kind = "node_unavailable",
                "external models proxy execution failed"
            );
            return Err(GatewayError::Internal(
                "external models proxy request failed".to_string(),
            ));
        };
        let proxy_mode = if proxy.mode.as_deref() == Some("tunnel") {
            "tunnel"
        } else if proxy.url.is_some() {
            "manual"
        } else {
            "unknown"
        };
        let headers = BTreeMap::from([
            ("accept".to_string(), "application/json".to_string()),
            (
                "user-agent".to_string(),
                "aether-gateway/external-models".to_string(),
            ),
            (
                EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string(),
                "false".to_string(),
            ),
        ]);
        let plan = ExecutionPlan {
            request_id: format!("{request_id}:external-models"),
            candidate_id: None,
            provider_name: Some("external_models_catalog".to_string()),
            provider_id: String::new(),
            endpoint_id: String::new(),
            key_id: String::new(),
            method: http::Method::GET.as_str().to_string(),
            url: source_url.to_string(),
            headers,
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: false,
            client_api_format: "control:external_models".to_string(),
            provider_api_format: "control:external_models".to_string(),
            model_name: None,
            proxy: Some(proxy),
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(ADMIN_EXTERNAL_MODELS_CONNECT_TIMEOUT_MS),
                read_ms: Some(ADMIN_EXTERNAL_MODELS_TOTAL_TIMEOUT_MS),
                write_ms: Some(ADMIN_EXTERNAL_MODELS_TOTAL_TIMEOUT_MS),
                pool_ms: Some(ADMIN_EXTERNAL_MODELS_CONNECT_TIMEOUT_MS),
                total_ms: Some(ADMIN_EXTERNAL_MODELS_TOTAL_TIMEOUT_MS),
                ..ExecutionTimeouts::default()
            }),
        };
        let bounded_plan = crate::execution_runtime::transport::with_upstream_response_body_limit(
            &plan,
            ADMIN_EXTERNAL_MODELS_RESPONSE_LIMIT_BYTES,
        );
        let result = match state
            .execute_execution_runtime_sync_plan(Some(request_id), &bounded_plan)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                let error_message = err.clone().into_message();
                let transport_error_kind =
                    classify_admin_external_models_transport_error(&error_message);
                warn!(
                    request_id = %request_id,
                    proxy_node_id = %node_id,
                    proxy_mode,
                    transport_error_kind,
                    "external models proxy execution failed"
                );
                return Err(GatewayError::Internal(
                    "external models proxy request failed".to_string(),
                ));
            }
        };
        if !(200..300).contains(&result.status_code) {
            return Err(GatewayError::Internal(format!(
                "external models source returned HTTP {}",
                result.status_code
            )));
        }
        let payload = result.body.and_then(|body| body.json_body).ok_or_else(|| {
            GatewayError::Internal(
                "external models source returned a non-JSON response".to_string(),
            )
        })?;
        return Ok(normalize_admin_external_models_payload(payload));
    }

    let (url, host, port) = parsed_source;
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        aether_http::lookup_host_with_limits(
            host.as_str(),
            port,
            aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
        )
        .await
        .map_err(|_| {
            GatewayError::Internal("external models source DNS resolution failed".to_string())
        })?
    };
    validate_admin_external_models_source_addresses(&url, &addresses, cfg!(test))?;
    let source = ResolvedAdminExternalModelsSource {
        url,
        host,
        addresses,
    };

    let client = build_admin_external_models_direct_client(&source)?;
    let response = client
        .get(source.url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            "aether-gateway/external-models",
        )
        .send()
        .await
        .map_err(|err| {
            let error_message = err.to_string();
            let transport_error_kind =
                classify_admin_external_models_transport_error(&error_message);
            warn!(
                request_id = %request_id,
                transport_error_kind,
                "external models direct request failed"
            );
            GatewayError::Internal("external models source request failed".to_string())
        })?;
    if !response.status().is_success() {
        return Err(GatewayError::Internal(format!(
            "external models source returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let body = aether_http::read_response_bytes_with_limit(
        response,
        ADMIN_EXTERNAL_MODELS_RESPONSE_LIMIT_BYTES,
    )
    .await
    .map_err(|_| {
        GatewayError::Internal("external models source response read failed".to_string())
    })?;
    let payload = serde_json::from_slice::<serde_json::Value>(&body).map_err(|_| {
        GatewayError::Internal("external models source returned invalid JSON".to_string())
    })?;
    Ok(normalize_admin_external_models_payload(payload))
}

pub(crate) async fn read_admin_external_models_cache(
    state: &AdminAppState<'_>,
    request_id: &str,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let proxy_node_id = read_admin_external_models_proxy_node_id(state).await?;
    if let Some(raw) = state
        .as_ref()
        .runtime_kv_get(ADMIN_EXTERNAL_MODELS_CACHE_KEY)
        .await?
    {
        match parse_admin_external_models_cache(&raw) {
            Some(envelope)
                if envelope.schema_version == ADMIN_EXTERNAL_MODELS_CACHE_VERSION
                    && envelope.proxy_node_id == proxy_node_id =>
            {
                return Ok(Some(normalize_admin_external_models_payload(
                    envelope.payload,
                )));
            }
            Some(_) => {}
            None => warn!("failed to parse cached external models payload"),
        }
    }

    match fetch_admin_external_models_from_source(state, request_id, proxy_node_id.as_deref()).await
    {
        Ok(payload) => {
            if store_admin_external_models_cache(state, proxy_node_id.as_deref(), &payload)
                .await
                .is_err()
            {
                warn!("failed to store fetched external models cache");
            }
            Ok(Some(payload))
        }
        Err(error) => {
            // Keep the client-facing response generic, but leave an actionable,
            // low-cardinality diagnostic for operators.  The underlying error
            // is intentionally not logged here because a future transport
            // implementation could include URL, proxy, or credential details.
            let error_message = error.into_message();
            let transport_error_kind =
                classify_admin_external_models_transport_error(&error_message);
            warn!(
                request_id = %request_id,
                proxy_mode = if proxy_node_id.is_some() { "configured_node" } else { "direct" },
                transport_error_kind,
                "failed to fetch external models catalog"
            );
            Ok(None)
        }
    }
}

pub(crate) async fn clear_admin_external_models_cache(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let deleted = clear_admin_external_models_cache_entries(state).await?;
    Ok(json!({
        "cleared": deleted,
        "message": if deleted { "缓存已清除" } else { "缓存不存在" },
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        admin_external_models_source_url, classify_admin_external_models_transport_error,
        normalize_admin_external_models_payload, normalize_admin_external_models_proxy_node_id,
        parse_admin_external_models_cache, parse_admin_external_models_source_url,
        read_admin_external_models_cache, resolve_admin_external_models_source,
        set_admin_external_models_source_url_for_tests,
        validate_admin_external_models_source_addresses, ADMIN_EXTERNAL_MODELS_CACHE_MAX_BYTES,
    };
    use crate::handlers::admin::request::AdminAppState;
    use crate::tests::{start_server, AppState};
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;

    #[test]
    fn normalizes_external_models_payload_with_official_flags() {
        let payload = json!({
            "openai": {
                "name": "OpenAI",
                "models": {}
            },
            "openrouter": {
                "name": "OpenRouter",
                "models": {}
            }
        });

        let normalized = normalize_admin_external_models_payload(payload);

        assert_eq!(normalized["openai"]["official"], json!(true));
        assert_eq!(normalized["openrouter"]["official"], json!(false));
    }

    #[test]
    fn external_models_proxy_config_only_treats_missing_or_null_as_direct() {
        assert_eq!(
            normalize_admin_external_models_proxy_node_id(None).expect("missing config is direct"),
            None
        );
        assert_eq!(
            normalize_admin_external_models_proxy_node_id(Some(&serde_json::Value::Null))
                .expect("null config is direct"),
            None
        );
        assert!(normalize_admin_external_models_proxy_node_id(Some(&json!("   "))).is_err());
        assert!(normalize_admin_external_models_proxy_node_id(Some(&json!(false))).is_err());
    }

    #[test]
    fn classifies_external_models_transport_errors_without_exposing_details() {
        for (message, expected) in [
            ("request timeout after 300000ms", "timeout"),
            (
                "external models source DNS resolution failed",
                "dns_resolution",
            ),
            (
                "external models source resolves to a private or reserved address",
                "ssrf_blocked",
            ),
            ("external models source URL is invalid", "invalid_url"),
            ("hub relay request failed", "relay"),
            ("invalid proxy configuration", "proxy_config"),
            ("upstream response body exceeds limit", "response_too_large"),
            ("upstream response is not valid JSON", "invalid_json"),
            ("failed to decode content-encoding gzip", "response_decode"),
            ("tcp connect error", "connect"),
            ("external models source returned HTTP 503", "upstream_http"),
            ("invalid upstream header value", "request_build"),
            ("opaque execution failure", "unknown_transport"),
        ] {
            assert_eq!(
                classify_admin_external_models_transport_error(message),
                expected,
                "message={message}"
            );
        }
    }

    #[test]
    fn external_models_source_url_uses_env_override_when_present() {
        let _guard = set_admin_external_models_source_url_for_tests("http://127.0.0.1:12345/api");
        assert_eq!(
            admin_external_models_source_url(),
            "http://127.0.0.1:12345/api"
        );
    }

    #[test]
    fn production_external_models_source_requires_safe_https_url_shape() {
        assert!(
            parse_admin_external_models_source_url("https://models.dev/api.json", false).is_ok()
        );

        for source_url in [
            "http://models.dev/api.json",
            "file:///etc/passwd",
            "https://user:secret@models.dev/api.json",
            "https://models.dev/api.json?next=http://169.254.169.254",
            "https://models.dev/api.json#fragment",
        ] {
            assert!(
                parse_admin_external_models_source_url(source_url, false).is_err(),
                "source URL should be rejected: {source_url}"
            );
        }
    }

    #[test]
    fn external_models_cache_parser_rejects_oversized_runtime_values() {
        let oversized = "x".repeat(ADMIN_EXTERNAL_MODELS_CACHE_MAX_BYTES + 1);
        assert!(parse_admin_external_models_cache(&oversized).is_none());

        let valid = r#"{"schema_version":2,"proxy_node_id":null,"payload":{}}"#;
        assert!(parse_admin_external_models_cache(valid).is_some());
    }

    #[tokio::test]
    async fn production_external_models_source_rejects_private_ip_literals() {
        for source_url in [
            "https://127.0.0.1/api.json",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/api.json",
        ] {
            assert!(
                resolve_admin_external_models_source(source_url, false)
                    .await
                    .is_err(),
                "private source URL should be rejected: {source_url}"
            );
        }
    }

    #[test]
    fn official_external_models_catalog_allows_benchmarking_fake_ip_addresses() {
        let url = url::Url::parse("https://models.dev/api.json")
            .expect("official catalog URL should parse");

        for address in ["198.18.75.234:443", "198.19.255.254:443"] {
            let address = address
                .parse()
                .expect("fake IP socket address should parse");
            assert!(
                validate_admin_external_models_source_addresses(&url, &[address], false).is_ok(),
                "official catalog should accept local proxy fake IP {address}"
            );
        }
    }

    #[test]
    fn custom_external_models_sources_reject_benchmarking_fake_ip_addresses() {
        let fake_ip = "198.18.75.234:443"
            .parse()
            .expect("fake IP socket address should parse");

        for source_url in [
            "https://catalog.example/api.json",
            "https://models.dev/other.json",
            "https://models.dev:444/api.json",
        ] {
            let url = url::Url::parse(source_url).expect("custom catalog URL should parse");
            assert!(
                validate_admin_external_models_source_addresses(&url, &[fake_ip], false).is_err(),
                "custom source must reject the benchmarking fake IP: {source_url}"
            );
        }
    }

    #[test]
    fn official_external_models_catalog_still_rejects_private_addresses() {
        let url = url::Url::parse("https://models.dev/api.json")
            .expect("official catalog URL should parse");

        for address in ["127.0.0.1:443", "10.0.0.1:443", "169.254.169.254:443"] {
            let address = address
                .parse()
                .expect("private socket address should parse");
            assert!(
                validate_admin_external_models_source_addresses(&url, &[address], false).is_err(),
                "official catalog must reject private address {address}"
            );
        }
    }

    #[tokio::test]
    async fn read_external_models_fetches_remote_payload_when_cache_missing() {
        let upstream = Router::new().route(
            "/api.json",
            get(|| async {
                Json(json!({
                    "openai": {
                        "name": "OpenAI",
                        "models": {
                            "gpt-5": {
                                "name": "GPT-5"
                            }
                        }
                    }
                }))
            }),
        );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let _guard =
            set_admin_external_models_source_url_for_tests(&format!("{upstream_url}/api.json"));

        let state = AppState::new().expect("gateway should build");
        let payload =
            read_admin_external_models_cache(&AdminAppState::new(&state), "external-models-test")
                .await
                .expect("external models read should succeed")
                .expect("payload should be fetched");

        assert_eq!(payload["openai"]["official"], json!(true));
        assert_eq!(payload["openai"]["models"]["gpt-5"]["name"], json!("GPT-5"));

        upstream_handle.abort();
    }

    #[tokio::test]
    async fn direct_external_models_fetch_does_not_follow_redirects() {
        let upstream = Router::new()
            .route(
                "/redirect",
                get(|| async { axum::response::Redirect::temporary("/api.json") }),
            )
            .route(
                "/api.json",
                get(|| async {
                    Json(json!({
                        "openai": {
                            "name": "redirected payload",
                            "models": {}
                        }
                    }))
                }),
            );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let _guard =
            set_admin_external_models_source_url_for_tests(&format!("{upstream_url}/redirect"));

        let state = AppState::new().expect("gateway should build");
        let payload = read_admin_external_models_cache(
            &AdminAppState::new(&state),
            "external-models-redirect",
        )
        .await
        .expect("external models read should not fail");

        assert!(payload.is_none(), "redirected payload must not be accepted");
        upstream_handle.abort();
    }
}
