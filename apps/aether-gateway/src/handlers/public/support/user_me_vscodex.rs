use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{
    ws::{CloseFrame as AxumCloseFrame, Message as AxumMessage, WebSocket, WebSocketUpgrade},
    ConnectInfo, State,
};
use axum::http::{self, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{
    CloseFrame as TungsteniteCloseFrame, WebSocketConfig,
};
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tracing::warn;

use super::{
    build_auth_error_response, build_auth_json_response, module_available_from_env,
    resolve_authenticated_local_user, AppState, GatewayPublicRequestContext,
};

const VSCODEX_ENABLED_ENV: &str = "AETHER_VSCODEX_ENABLED";
const VSCODEX_INTERNAL_URL_ENV: &str = "AETHER_VSCODEX_INTERNAL_URL";
const VSCODEX_INTERNAL_TOKEN_ENV: &str = "AETHER_VSCODEX_INTERNAL_TOKEN";
const VSCODEX_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const VSCODEX_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const VSCODEX_WS_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const VSCODEX_WS_MAX_CONNECTIONS: usize = 256;
const VSCODEX_WS_MAX_CONNECTIONS_PER_IP: usize = 16;
const VSCODEX_DEVICE_PATH_PREFIX: &str = "/api/users/me/vscodex/devices/";
const VSCODEX_CLIENT_IP_HEADER: &str = "x-aether-client-ip";

static VSCODEX_HTTP_CLIENT: LazyLock<Result<reqwest::Client, reqwest::Error>> =
    LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
    });
static VSCODEX_WS_CONNECTIONS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(VSCODEX_WS_MAX_CONNECTIONS)));
static VSCODEX_WS_CONNECTIONS_BY_IP: LazyLock<Arc<VscodexWsIpConnectionLimiter>> =
    LazyLock::new(|| {
        Arc::new(VscodexWsIpConnectionLimiter::new(
            VSCODEX_WS_MAX_CONNECTIONS_PER_IP,
        ))
    });

#[derive(Debug)]
struct VscodexWsIpConnectionLimiter {
    max_connections: usize,
    active: Mutex<HashMap<IpAddr, usize>>,
}

impl VscodexWsIpConnectionLimiter {
    fn new(max_connections: usize) -> Self {
        Self {
            max_connections: max_connections.max(1),
            active: Mutex::new(HashMap::new()),
        }
    }

    fn try_acquire(self: &Arc<Self>, client_ip: IpAddr) -> Option<VscodexWsIpConnectionPermit> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = active.get(&client_ip).copied().unwrap_or_default();
        if current >= self.max_connections {
            return None;
        }
        active.insert(client_ip, current.saturating_add(1));
        Some(VscodexWsIpConnectionPermit {
            limiter: Arc::clone(self),
            client_ip,
        })
    }

    fn release(&self, client_ip: IpAddr) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(current) = active.get_mut(&client_ip) else {
            return;
        };
        if *current <= 1 {
            active.remove(&client_ip);
        } else {
            *current -= 1;
        }
    }

    #[cfg(test)]
    fn active_ip_count(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

#[derive(Debug)]
struct VscodexWsIpConnectionPermit {
    limiter: Arc<VscodexWsIpConnectionLimiter>,
    client_ip: IpAddr,
}

impl Drop for VscodexWsIpConnectionPermit {
    fn drop(&mut self) {
        self.limiter.release(self.client_ip);
    }
}

#[derive(Debug)]
struct VscodexSidecarConfig {
    base_url: reqwest::Url,
    authorization: reqwest::header::HeaderValue,
    http_client: reqwest::Client,
}

#[derive(Debug, Default, Deserialize)]
struct CreatePairingRequest {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateWsTicketRequest {
    device_id: String,
}

#[derive(Debug, Deserialize)]
struct ExchangePairingRequest {
    code: String,
    name: Option<String>,
}

pub(crate) async fn vscodex_ws_proxy(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<std::net::SocketAddr>,
    ws: WebSocketUpgrade,
    headers: http::HeaderMap,
) -> Response<Body> {
    let request_permit = match state.try_acquire_request_permit().await {
        Ok(value) => value,
        Err(err) => {
            warn!(error = ?err, "VS Codex WebSocket request admission rejected");
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "服务繁忙，请稍后重试",
                false,
            );
        }
    };
    let client_ip = crate::headers::effective_client_ip(&headers, &remote_addr);
    match state.admin_security_ip_blacklisted(client_ip).await {
        Ok(true) => {
            return build_auth_error_response(
                http::StatusCode::FORBIDDEN,
                "当前 IP 已被禁止访问",
                false,
            )
        }
        Ok(false) => {}
        Err(err) => warn!(
            client_ip = %client_ip,
            error = ?err,
            "VS Codex WebSocket IP blacklist check failed open"
        ),
    }
    let connection_permit = match Arc::clone(&VSCODEX_WS_CONNECTIONS).try_acquire_owned() {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 连接数已达上限",
                false,
            )
        }
    };
    // Only active connections have entries, and each already owns one of the 256 global slots.
    let ip_connection_permit = match VSCODEX_WS_CONNECTIONS_BY_IP.try_acquire(client_ip) {
        Some(value) => value,
        None => {
            warn!(
                client_ip = %client_ip,
                limit = VSCODEX_WS_MAX_CONNECTIONS_PER_IP,
                "VS Codex per-IP WebSocket connection limit reached"
            );
            let mut response = build_auth_error_response(
                http::StatusCode::TOO_MANY_REQUESTS,
                "当前 IP 的 VS Codex 连接数已达上限",
                false,
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, http::HeaderValue::from_static("1"));
            return response;
        }
    };
    let config = match load_vscodex_sidecar_config() {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务未启用",
                false,
            )
        }
        Err(detail) => {
            warn!(error = %detail, "VS Codex WebSocket sidecar configuration is invalid");
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务配置不完整",
                false,
            );
        }
    };
    let sidecar_url = match build_vscodex_websocket_url(&config.base_url) {
        Ok(value) => value,
        Err(detail) => {
            warn!(error = %detail, "could not build VS Codex sidecar WebSocket URL");
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务配置不完整",
                false,
            );
        }
    };
    let mut sidecar_request = match sidecar_url.as_str().into_client_request() {
        Ok(value) => value,
        Err(err) => {
            warn!(error = %err, "could not build VS Codex sidecar WebSocket request");
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务配置不完整",
                false,
            );
        }
    };
    for header_name in [header::ORIGIN, header::SEC_WEBSOCKET_PROTOCOL] {
        if let Some(value) = headers.get(&header_name) {
            sidecar_request
                .headers_mut()
                .insert(header_name, value.clone());
        }
    }

    let mut sidecar_config = WebSocketConfig::default();
    sidecar_config.max_message_size = Some(VSCODEX_WS_MAX_MESSAGE_BYTES);
    sidecar_config.max_frame_size = Some(VSCODEX_WS_MAX_MESSAGE_BYTES);
    let (sidecar_socket, sidecar_response) = match tokio::time::timeout(
        VSCODEX_REQUEST_TIMEOUT,
        tokio_tungstenite::connect_async_with_config(sidecar_request, Some(sidecar_config), true),
    )
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            warn!(error = %err, "VS Codex sidecar WebSocket handshake failed");
            return build_auth_error_response(
                http::StatusCode::BAD_GATEWAY,
                "VS Codex 服务暂时不可用",
                false,
            );
        }
        Err(_) => {
            warn!("VS Codex sidecar WebSocket handshake timed out");
            return build_auth_error_response(
                http::StatusCode::GATEWAY_TIMEOUT,
                "VS Codex 服务请求超时",
                false,
            );
        }
    };

    let selected_protocol = sidecar_response
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let ws = ws
        .max_message_size(VSCODEX_WS_MAX_MESSAGE_BYTES)
        .max_frame_size(VSCODEX_WS_MAX_MESSAGE_BYTES);
    let ws = match selected_protocol {
        Some(protocol) => ws.protocols([protocol]),
        None => ws,
    };
    drop(request_permit);
    ws.on_upgrade(move |browser_socket| async move {
        let _connection_permit = connection_permit;
        bridge_vscodex_websockets(browser_socket, sidecar_socket, ip_connection_permit).await;
    })
}

pub(super) async fn maybe_build_local_vscodex_response(
    _state: &AppState,
    request_context: &GatewayPublicRequestContext,
    client_ip: std::net::IpAddr,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("vscodex") {
        return None;
    }
    if decision.route_kind.as_deref() != Some("pairing_exchange")
        || !matches!(
            request_context.request_path.as_str(),
            "/api/vscodex/pair" | "/api/vscodex/pair/"
        )
    {
        return Some(build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "VS Codex 接口不存在",
            false,
        ));
    }

    let config = match load_vscodex_sidecar_config() {
        Ok(Some(value)) => value,
        Ok(None) => {
            return Some(build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务未启用",
                false,
            ))
        }
        Err(detail) => {
            warn!(
                error = %detail,
                "VS Codex sidecar configuration is invalid"
            );
            return Some(build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务配置不完整",
                false,
            ));
        }
    };
    let payload = match parse_pairing_exchange_request(request_body) {
        Ok(value) => value,
        Err(response) => return Some(response),
    };
    let url = match append_vscodex_sidecar_path(&config.base_url, &["v1", "pairings", "exchange"]) {
        Ok(value) => value,
        Err(detail) => {
            warn!(error = %detail, "could not build VS Codex pairing exchange URL");
            return Some(build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务配置不完整",
                false,
            ));
        }
    };
    let request =
        build_authenticated_sidecar_request(&config, reqwest::Method::POST, url, Some(payload))
            .header(VSCODEX_CLIENT_IP_HEADER, client_ip.to_string());
    Some(send_vscodex_sidecar_request(request, "public", "pairing_exchange").await)
}

pub(super) async fn handle_users_me_vscodex_request(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let config = match load_vscodex_sidecar_config() {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务未启用",
                false,
            )
        }
        Err(detail) => {
            warn!(
                user_id = %auth.user.id,
                error = %detail,
                "VS Codex sidecar configuration is invalid"
            );
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "VS Codex 服务配置不完整",
                false,
            );
        }
    };

    let Some(route_kind) = request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_kind.as_deref())
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "VS Codex 接口不存在",
            false,
        );
    };

    let request = match build_vscodex_sidecar_request(
        &config,
        &auth.user.id,
        route_kind,
        &request_context.request_path,
        request_body,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };

    send_vscodex_sidecar_request(request, &auth.user.id, route_kind).await
}

fn load_vscodex_sidecar_config() -> Result<Option<VscodexSidecarConfig>, String> {
    if !module_available_from_env(VSCODEX_ENABLED_ENV, false) {
        return Ok(None);
    }

    let raw_url = required_env(VSCODEX_INTERNAL_URL_ENV)?;
    let base_url = reqwest::Url::parse(&raw_url)
        .map_err(|err| format!("{VSCODEX_INTERNAL_URL_ENV} is invalid: {err}"))?;
    if !matches!(base_url.scheme(), "http" | "https")
        || !base_url.has_host()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || base_url.cannot_be_a_base()
    {
        return Err(format!(
            "{VSCODEX_INTERNAL_URL_ENV} must be an HTTP(S) base URL without credentials, query, or fragment"
        ));
    }

    let token = required_env(VSCODEX_INTERNAL_TOKEN_ENV)?;
    let authorization = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| format!("{VSCODEX_INTERNAL_TOKEN_ENV} is not a valid HTTP credential"))?;
    let http_client = VSCODEX_HTTP_CLIENT
        .as_ref()
        .map_err(|err| format!("could not initialize VS Codex HTTP client: {err}"))?
        .clone();

    Ok(Some(VscodexSidecarConfig {
        base_url,
        authorization,
        http_client,
    }))
}

fn required_env(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn build_vscodex_sidecar_request(
    config: &VscodexSidecarConfig,
    user_id: &str,
    route_kind: &str,
    request_path: &str,
    request_body: Option<&Bytes>,
) -> Result<reqwest::RequestBuilder, Response<Body>> {
    let (method, suffix, payload) = match route_kind {
        "vscodex_devices_list" => (reqwest::Method::GET, vec!["devices"], None),
        "vscodex_pairing_create" => (
            reqwest::Method::POST,
            vec!["pairings"],
            Some(parse_pairing_request(request_body)?),
        ),
        "vscodex_device_delete" => {
            let Some(device_id) = vscodex_device_id_from_path(request_path) else {
                return Err(build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "设备标识无效",
                    false,
                ));
            };
            (reqwest::Method::DELETE, vec!["devices", device_id], None)
        }
        "vscodex_ws_ticket_create" => (
            reqwest::Method::POST,
            vec!["ws-tickets"],
            Some(parse_ws_ticket_request(request_body)?),
        ),
        _ => {
            return Err(build_auth_error_response(
                http::StatusCode::NOT_FOUND,
                "VS Codex 接口不存在",
                false,
            ))
        }
    };
    let url = build_vscodex_sidecar_url(&config.base_url, user_id, &suffix).map_err(|detail| {
        warn!(user_id = %user_id, error = %detail, "could not build VS Codex sidecar URL");
        build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "VS Codex 服务配置不完整",
            false,
        )
    })?;

    Ok(build_authenticated_sidecar_request(
        config, method, url, payload,
    ))
}

fn build_authenticated_sidecar_request(
    config: &VscodexSidecarConfig,
    method: reqwest::Method,
    url: reqwest::Url,
    payload: Option<Value>,
) -> reqwest::RequestBuilder {
    let mut request = config
        .http_client
        .request(method, url)
        .header(header::AUTHORIZATION, config.authorization.clone())
        .header(header::ACCEPT, "application/json")
        .timeout(VSCODEX_REQUEST_TIMEOUT);
    if let Some(payload) = payload {
        request = request.json(&payload);
    }
    request
}

fn build_vscodex_sidecar_url(
    base_url: &reqwest::Url,
    user_id: &str,
    suffix: &[&str],
) -> Result<reqwest::Url, String> {
    let mut segments = vec!["internal", "v1", "users", user_id];
    segments.extend(suffix.iter().copied());
    append_vscodex_sidecar_path(base_url, &segments)
}

fn append_vscodex_sidecar_path(
    base_url: &reqwest::Url,
    suffix: &[&str],
) -> Result<reqwest::Url, String> {
    let mut url = base_url.clone();
    let mut path_segments = url
        .path_segments_mut()
        .map_err(|_| "VS Codex sidecar URL cannot contain path segments".to_string())?;
    path_segments.pop_if_empty();
    path_segments.extend(suffix.iter().copied());
    drop(path_segments);
    Ok(url)
}

fn build_vscodex_websocket_url(base_url: &reqwest::Url) -> Result<reqwest::Url, String> {
    let mut url = append_vscodex_sidecar_path(base_url, &["api", "vscodex", "ws"])?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => return Err("VS Codex sidecar URL must use HTTP(S)".to_string()),
    };
    url.set_scheme(scheme)
        .map_err(|_| "could not convert VS Codex sidecar URL to WebSocket".to_string())?;
    Ok(url)
}

fn parse_pairing_request(request_body: Option<&Bytes>) -> Result<Value, Response<Body>> {
    let payload = parse_json_request::<CreatePairingRequest>(request_body, true)?;
    let mut object = Map::new();
    if let Some(name) = payload.name {
        object.insert("name".to_string(), Value::String(name));
    }
    Ok(Value::Object(object))
}

fn parse_ws_ticket_request(request_body: Option<&Bytes>) -> Result<Value, Response<Body>> {
    let payload = parse_json_request::<CreateWsTicketRequest>(request_body, false)?;
    let device_id = payload.device_id.trim();
    if !valid_vscodex_device_id(device_id) {
        return Err(build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "设备标识无效",
            false,
        ));
    }
    Ok(json!({ "device_id": device_id }))
}

fn parse_pairing_exchange_request(request_body: Option<&Bytes>) -> Result<Value, Response<Body>> {
    let payload = parse_json_request::<ExchangePairingRequest>(request_body, false)?;
    let code = payload.code.trim();
    if code.is_empty() || code.len() > 256 || code.chars().any(char::is_control) {
        return Err(build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "配对码无效",
            false,
        ));
    }
    let mut object = Map::from_iter([("code".to_string(), Value::String(code.to_string()))]);
    if let Some(name) = payload.name {
        object.insert("name".to_string(), Value::String(name));
    }
    Ok(Value::Object(object))
}

fn parse_json_request<T>(
    request_body: Option<&Bytes>,
    empty_object_allowed: bool,
) -> Result<T, Response<Body>>
where
    T: serde::de::DeserializeOwned,
{
    let body = request_body.filter(|body| !body.is_empty());
    let result = match body {
        Some(body) => serde_json::from_slice(body),
        None if empty_object_allowed => serde_json::from_slice(b"{}"),
        None => {
            return Err(build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "缺少请求体",
                false,
            ))
        }
    };
    result.map_err(|_| {
        build_auth_error_response(http::StatusCode::BAD_REQUEST, "请求数据验证失败", false)
    })
}

fn vscodex_device_id_from_path(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    let device_id = trimmed.strip_prefix(VSCODEX_DEVICE_PATH_PREFIX)?;
    if device_id.contains('/') || !valid_vscodex_device_id(device_id) {
        return None;
    }
    Some(device_id)
}

fn valid_vscodex_device_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

async fn send_vscodex_sidecar_request(
    request: reqwest::RequestBuilder,
    request_scope: &str,
    operation: &str,
) -> Response<Body> {
    let mut upstream = match request.send().await {
        Ok(value) => value,
        Err(err) => {
            warn!(
                request_scope = %request_scope,
                operation = %operation,
                error = %err,
                "VS Codex sidecar request failed"
            );
            let (status, detail) = if err.is_timeout() {
                (http::StatusCode::GATEWAY_TIMEOUT, "VS Codex 服务请求超时")
            } else {
                (http::StatusCode::BAD_GATEWAY, "VS Codex 服务暂时不可用")
            };
            return build_auth_error_response(status, detail, false);
        }
    };
    let status = http::StatusCode::from_u16(upstream.status().as_u16())
        .unwrap_or(http::StatusCode::BAD_GATEWAY);

    if matches!(
        status,
        http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN
    ) {
        warn!(
            request_scope = %request_scope,
            operation = %operation,
            upstream_status = status.as_u16(),
            "VS Codex sidecar rejected gateway credentials"
        );
        return build_auth_error_response(
            http::StatusCode::BAD_GATEWAY,
            "VS Codex 服务鉴权失败",
            false,
        );
    }
    if status.is_redirection() {
        warn!(
            request_scope = %request_scope,
            operation = %operation,
            upstream_status = status.as_u16(),
            "VS Codex sidecar returned an unexpected redirect"
        );
        return build_auth_error_response(
            http::StatusCode::BAD_GATEWAY,
            "VS Codex 服务返回无效响应",
            false,
        );
    }
    if status == http::StatusCode::NO_CONTENT {
        return vscodex_no_store_response(status.into_response(), None);
    }

    let mut response_body = Vec::new();
    while let Some(chunk) = match upstream.chunk().await {
        Ok(value) => value,
        Err(err) => {
            warn!(
                request_scope = %request_scope,
                operation = %operation,
                error = %err,
                "could not read VS Codex sidecar response"
            );
            return build_auth_error_response(
                http::StatusCode::BAD_GATEWAY,
                "VS Codex 服务返回无效响应",
                false,
            );
        }
    } {
        if response_body.len().saturating_add(chunk.len()) > VSCODEX_MAX_RESPONSE_BYTES {
            warn!(
                request_scope = %request_scope,
                operation = %operation,
                "VS Codex sidecar response exceeded the size limit"
            );
            return build_auth_error_response(
                http::StatusCode::BAD_GATEWAY,
                "VS Codex 服务返回无效响应",
                false,
            );
        }
        response_body.extend_from_slice(&chunk);
    }

    if response_body.is_empty() {
        return build_auth_error_response(
            http::StatusCode::BAD_GATEWAY,
            "VS Codex 服务返回无效响应",
            false,
        );
    }
    let payload = match serde_json::from_slice(&response_body) {
        Ok(value) => value,
        Err(err) => {
            warn!(
                request_scope = %request_scope,
                operation = %operation,
                upstream_status = status.as_u16(),
                error = %err,
                "VS Codex sidecar returned non-JSON data"
            );
            return build_auth_error_response(
                http::StatusCode::BAD_GATEWAY,
                "VS Codex 服务返回无效响应",
                false,
            );
        }
    };
    let retry_after = upstream.headers().get(header::RETRY_AFTER).cloned();
    vscodex_no_store_response(build_auth_json_response(status, payload, None), retry_after)
}

fn vscodex_no_store_response(
    mut response: Response<Body>,
    retry_after: Option<http::HeaderValue>,
) -> Response<Body> {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    if let Some(retry_after) = retry_after {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, retry_after);
    }
    response
}

async fn bridge_vscodex_websockets<S>(
    browser_socket: WebSocket,
    sidecar_socket: S,
    ip_connection_permit: VscodexWsIpConnectionPermit,
) where
    S: futures_util::Stream<
            Item = Result<TungsteniteMessage, tokio_tungstenite::tungstenite::Error>,
        > + futures_util::Sink<TungsteniteMessage, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin
        + Send
        + 'static,
{
    let (mut browser_tx, mut browser_rx) = browser_socket.split();
    let (mut sidecar_tx, mut sidecar_rx) = sidecar_socket.split();
    let mut ip_connection_permit = Some(ip_connection_permit);

    loop {
        tokio::select! {
            browser_message = browser_rx.next() => {
                match browser_message {
                    Some(Ok(message)) => {
                        let close = matches!(message, AxumMessage::Close(_));
                        if let Err(err) = sidecar_tx.send(axum_to_tungstenite_message(message)).await {
                            warn!(error = %err, "could not forward VS Codex browser WebSocket frame");
                            break;
                        }
                        if close {
                            break;
                        }
                    }
                    Some(Err(err)) => {
                        warn!(error = %err, "VS Codex browser WebSocket read failed");
                        break;
                    }
                    None => break,
                }
            }
            sidecar_message = sidecar_rx.next() => {
                match sidecar_message {
                    Some(Ok(TungsteniteMessage::Frame(_))) => continue,
                    Some(Ok(message)) => {
                        if ip_connection_permit.is_some() && vscodex_ws_authentication_succeeded(&message) {
                            ip_connection_permit.take();
                        }
                        let close = matches!(message, TungsteniteMessage::Close(_));
                        if let Err(err) = browser_tx.send(tungstenite_to_axum_message(message)).await {
                            warn!(error = %err, "could not forward VS Codex sidecar WebSocket frame");
                            break;
                        }
                        if close {
                            break;
                        }
                    }
                    Some(Err(err)) => {
                        warn!(error = %err, "VS Codex sidecar WebSocket read failed");
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    let _ = sidecar_tx.close().await;
    let _ = browser_tx.close().await;
}

fn vscodex_ws_authentication_succeeded(message: &TungsteniteMessage) -> bool {
    let TungsteniteMessage::Text(text) = message else {
        return false;
    };
    serde_json::from_str::<Value>(text.as_ref())
        .ok()
        .and_then(|payload| {
            payload
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .as_deref()
        == Some("auth.ok")
}

fn axum_to_tungstenite_message(message: AxumMessage) -> TungsteniteMessage {
    match message {
        AxumMessage::Text(text) => TungsteniteMessage::Text(text.to_string().into()),
        AxumMessage::Binary(bytes) => TungsteniteMessage::Binary(bytes),
        AxumMessage::Ping(bytes) => TungsteniteMessage::Ping(bytes),
        AxumMessage::Pong(bytes) => TungsteniteMessage::Pong(bytes),
        AxumMessage::Close(frame) => {
            TungsteniteMessage::Close(frame.map(|frame| TungsteniteCloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }))
        }
    }
}

fn tungstenite_to_axum_message(message: TungsteniteMessage) -> AxumMessage {
    match message {
        TungsteniteMessage::Text(text) => AxumMessage::Text(text.to_string().into()),
        TungsteniteMessage::Binary(bytes) => AxumMessage::Binary(bytes),
        TungsteniteMessage::Ping(bytes) => AxumMessage::Ping(bytes),
        TungsteniteMessage::Pong(bytes) => AxumMessage::Pong(bytes),
        TungsteniteMessage::Close(frame) => AxumMessage::Close(frame.map(|frame| AxumCloseFrame {
            code: frame.code.into(),
            reason: frame.reason.to_string().into(),
        })),
        TungsteniteMessage::Frame(_) => AxumMessage::Close(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{vscodex_ws_authentication_succeeded, VscodexWsIpConnectionLimiter};
    use std::sync::Arc;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn vscodex_ws_ip_limiter_releases_and_removes_inactive_ips() {
        let limiter = Arc::new(VscodexWsIpConnectionLimiter::new(1));
        let client_ip = "198.51.100.10".parse().expect("IP should parse");

        let permit = limiter
            .try_acquire(client_ip)
            .expect("first connection should acquire");
        assert_eq!(limiter.active_ip_count(), 1);
        assert!(limiter.try_acquire(client_ip).is_none());

        drop(permit);
        assert_eq!(limiter.active_ip_count(), 0);
        assert!(limiter.try_acquire(client_ip).is_some());
    }

    #[test]
    fn vscodex_ws_ip_limiter_releases_only_after_sidecar_auth_success() {
        assert!(vscodex_ws_authentication_succeeded(&Message::Text(
            r#"{"type":"auth.ok","role":"operator"}"#.into()
        )));
        assert!(!vscodex_ws_authentication_succeeded(&Message::Text(
            r#"{"type":"auth","token":"client-controlled"}"#.into()
        )));
        assert!(!vscodex_ws_authentication_succeeded(&Message::Binary(
            br#"{"type":"auth.ok"}"#.to_vec().into()
        )));
    }
}
