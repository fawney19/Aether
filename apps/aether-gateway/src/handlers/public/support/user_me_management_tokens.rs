use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use aether_data::repository::management_tokens::{
    CreateManagementTokenRecord, ManagementTokenListQuery, RegenerateManagementTokenSecret,
    StoredManagementTokenUserSummary, UpdateManagementTokenRecord,
};

use super::{
    build_auth_error_response, build_management_token_payload, query_param_optional_bool,
    query_param_value, resolve_authenticated_local_user, AppState, AuthenticatedLocalUserContext,
    GatewayPublicRequestContext,
};
use crate::handlers::shared::generate_gateway_secret_plaintext;
use crate::LocalMutationOutcome;

const USERS_ME_MANAGEMENT_TOKEN_PREFIX: &str = "ae";
const USERS_ME_MANAGEMENT_TOKEN_SEPARATOR: &str = "-";
const USERS_ME_MANAGEMENT_TOKEN_DISPLAY_PREFIX_LEN: usize = 10;
const USERS_ME_MANAGEMENT_TOKEN_FETCH_LIMIT: usize = 10_000;
const USERS_ME_MANAGEMENT_TOKEN_DEFAULT_MAX_PER_USER: usize = 20;
const USERS_ME_MANAGEMENT_TOKEN_MAX_PER_USER_ENV: &str = "MANAGEMENT_TOKEN_MAX_PER_USER";
const USERS_ME_MANAGEMENT_TOKEN_READ_UNAVAILABLE_DETAIL: &str =
    "用户 Management Token 数据暂不可用";
const USERS_ME_MANAGEMENT_TOKEN_WRITE_UNAVAILABLE_DETAIL: &str =
    "用户 Management Token 写入暂不可用";

#[derive(Debug, Clone)]
struct UsersMeManagementTokenCreateInput {
    name: String,
    description: Option<String>,
    allowed_ips: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct UsersMeManagementTokenUpdateInput {
    name: Option<String>,
    description: Option<String>,
    clear_description: bool,
    allowed_ips: Option<serde_json::Value>,
    clear_allowed_ips: bool,
    expires_at_unix_secs: Option<u64>,
    clear_expires_at: bool,
}

impl UsersMeManagementTokenUpdateInput {
    fn is_noop(&self) -> bool {
        self.name.is_none()
            && self.description.is_none()
            && !self.clear_description
            && self.allowed_ips.is_none()
            && !self.clear_allowed_ips
            && self.expires_at_unix_secs.is_none()
            && !self.clear_expires_at
    }
}

pub(super) fn users_me_management_tokens_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/me/management-tokens" | "/api/me/management-tokens/"
    )
}

fn users_me_management_token_path_segments(request_path: &str) -> Option<Vec<&str>> {
    let raw = request_path
        .strip_prefix("/api/me/management-tokens/")?
        .trim()
        .trim_matches('/');
    if raw.is_empty() {
        return None;
    }
    let segments = raw
        .split('/')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!segments.is_empty()).then_some(segments)
}

fn users_me_management_token_id_from_path(request_path: &str) -> Option<String> {
    let segments = users_me_management_token_path_segments(request_path)?;
    (segments.len() == 1).then(|| segments[0].to_string())
}

fn users_me_management_token_status_id_from_path(request_path: &str) -> Option<String> {
    let segments = users_me_management_token_path_segments(request_path)?;
    (segments.len() == 2 && segments[1] == "status").then(|| segments[0].to_string())
}

fn users_me_management_token_regenerate_id_from_path(request_path: &str) -> Option<String> {
    let segments = users_me_management_token_path_segments(request_path)?;
    (segments.len() == 2 && segments[1] == "regenerate").then(|| segments[0].to_string())
}

pub(super) fn users_me_management_token_detail_path_matches(request_path: &str) -> bool {
    users_me_management_token_id_from_path(request_path).is_some()
}

pub(super) fn users_me_management_token_toggle_path_matches(request_path: &str) -> bool {
    users_me_management_token_status_id_from_path(request_path).is_some()
}

pub(super) fn users_me_management_token_regenerate_path_matches(request_path: &str) -> bool {
    users_me_management_token_regenerate_id_from_path(request_path).is_some()
}

fn users_me_management_token_max_per_user() -> usize {
    std::env::var(USERS_ME_MANAGEMENT_TOKEN_MAX_PER_USER_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(USERS_ME_MANAGEMENT_TOKEN_DEFAULT_MAX_PER_USER)
}

fn generate_users_me_management_token_plaintext() -> String {
    generate_gateway_secret_plaintext(
        USERS_ME_MANAGEMENT_TOKEN_PREFIX,
        USERS_ME_MANAGEMENT_TOKEN_SEPARATOR,
    )
}

fn hash_users_me_management_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn users_me_management_token_prefix(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| {
        value[..value
            .len()
            .min(USERS_ME_MANAGEMENT_TOKEN_DISPLAY_PREFIX_LEN)]
            .to_string()
    })
}

fn build_users_me_management_token_reader_unavailable_response() -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        USERS_ME_MANAGEMENT_TOKEN_READ_UNAVAILABLE_DETAIL,
        false,
    )
}

fn build_users_me_management_token_writer_unavailable_response() -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        USERS_ME_MANAGEMENT_TOKEN_WRITE_UNAVAILABLE_DETAIL,
        false,
    )
}

fn users_me_management_token_limit(query: Option<&str>) -> usize {
    query_param_value(query, "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=100).contains(value))
        .unwrap_or(50)
}

fn users_me_management_token_skip(query: Option<&str>) -> usize {
    query_param_value(query, "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

fn users_me_validate_ip_or_cidr(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if value.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let Some((host, prefix)) = value.split_once('/') else {
        return false;
    };
    let Ok(ip) = host.trim().parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.trim().parse::<u8>() else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(_) => prefix <= 32,
        std::net::IpAddr::V6(_) => prefix <= 128,
    }
}

fn users_me_parse_management_token_allowed_ips(
    value: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return Err("IP 白名单不能为空列表，如需取消限制请不提供此字段".to_string());
            }
            let mut normalized = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                let Some(raw) = item.as_str() else {
                    return Err("IP 白名单必须是字符串数组".to_string());
                };
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(format!("IP 白名单第 {} 项为空", index + 1));
                }
                if !users_me_validate_ip_or_cidr(trimmed) {
                    return Err(format!("无效的 IP 地址或 CIDR: {raw}"));
                }
                normalized.push(trimmed.to_string());
            }
            Ok(Some(json!(normalized)))
        }
        _ => Err("IP 白名单必须是字符串数组".to_string()),
    }
}

fn users_me_parse_management_token_expires_at(
    value: &serde_json::Value,
    allow_past: bool,
) -> Result<Option<u64>, String> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                return Ok(None);
            }
            let parsed = chrono::DateTime::parse_from_rfc3339(raw)
                .map(|value| value.with_timezone(&Utc))
                .or_else(|_| {
                    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                        .or_else(|_| {
                            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
                        })
                        .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M"))
                        .map(|value| value.and_utc())
                })
                .map_err(|_| format!("无效的时间格式: {raw}"))?;
            let unix_secs = parsed.timestamp();
            if !allow_past && unix_secs <= Utc::now().timestamp() {
                return Err("过期时间必须在未来".to_string());
            }
            u64::try_from(unix_secs)
                .map(Some)
                .map_err(|_| format!("无效的时间格式: {raw}"))
        }
        _ => Err("expires_at 必须是字符串或 null".to_string()),
    }
}

fn users_me_parse_management_token_create_input(
    request_body: &[u8],
) -> Result<UsersMeManagementTokenCreateInput, String> {
    let payload =
        serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(request_body)
            .map_err(|_| "输入验证失败".to_string())?;
    let name = match payload.get("name") {
        Some(serde_json::Value::String(value)) if (1..=100).contains(&value.chars().count()) => {
            value.clone()
        }
        _ => return Err("输入验证失败".to_string()),
    };
    let description = match payload.get("description") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) if value.chars().count() <= 500 => {
            Some(value.clone())
        }
        _ => return Err("输入验证失败".to_string()),
    };
    let allowed_ips = users_me_parse_management_token_allowed_ips(payload.get("allowed_ips"))?;
    let expires_at_unix_secs = match payload.get("expires_at") {
        Some(value) => users_me_parse_management_token_expires_at(value, false)?,
        None => None,
    };

    Ok(UsersMeManagementTokenCreateInput {
        name,
        description,
        allowed_ips,
        expires_at_unix_secs,
    })
}

fn users_me_parse_management_token_update_input(
    request_body: &[u8],
) -> Result<UsersMeManagementTokenUpdateInput, String> {
    let payload =
        serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(request_body)
            .map_err(|_| "输入验证失败".to_string())?;
    let mut input = UsersMeManagementTokenUpdateInput::default();

    if let Some(value) = payload.get("name") {
        match value {
            serde_json::Value::Null => {}
            serde_json::Value::String(value) if (1..=100).contains(&value.chars().count()) => {
                input.name = Some(value.clone());
            }
            _ => return Err("输入验证失败".to_string()),
        }
    }

    if let Some(value) = payload.get("description") {
        match value {
            serde_json::Value::Null => input.clear_description = true,
            serde_json::Value::String(value) if value.chars().count() <= 500 => {
                if value.is_empty() {
                    input.clear_description = true;
                } else {
                    input.description = Some(value.clone());
                }
            }
            _ => return Err("输入验证失败".to_string()),
        }
    }

    if let Some(value) = payload.get("allowed_ips") {
        if value.is_null() {
            input.clear_allowed_ips = true;
        } else {
            input.allowed_ips = users_me_parse_management_token_allowed_ips(Some(value))?;
        }
    }

    if let Some(value) = payload.get("expires_at") {
        if value.is_null() || value.as_str().is_some_and(|value| value.trim().is_empty()) {
            input.clear_expires_at = true;
        } else {
            input.expires_at_unix_secs = users_me_parse_management_token_expires_at(value, false)?;
        }
    }

    Ok(input)
}

fn build_users_me_management_token_user_summary(
    auth: &AuthenticatedLocalUserContext,
) -> Result<StoredManagementTokenUserSummary, Response<Body>> {
    StoredManagementTokenUserSummary::new(
        auth.user.id.clone(),
        auth.user.email.clone(),
        auth.user.username.clone(),
        auth.user.role.clone(),
    )
    .map_err(|err| {
        build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("management token user summary build failed: {err:?}"),
            false,
        )
    })
}

async fn list_users_me_management_tokens_for_user(
    state: &AppState,
    user_id: &str,
    is_active: Option<bool>,
    limit: usize,
) -> Result<aether_data::repository::management_tokens::StoredManagementTokenListPage, Response<Body>>
{
    if !state.has_management_token_reader() {
        return Err(build_users_me_management_token_reader_unavailable_response());
    }
    state
        .list_management_tokens(&ManagementTokenListQuery {
            user_id: Some(user_id.to_string()),
            is_active,
            offset: 0,
            limit,
        })
        .await
        .map_err(|err| {
            build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("management token list failed: {err:?}"),
                false,
            )
        })
}

async fn resolve_users_me_management_token(
    state: &AppState,
    user_id: &str,
    token_id: &str,
) -> Result<aether_data::repository::management_tokens::StoredManagementTokenWithUser, Response<Body>>
{
    if !state.has_management_token_reader() {
        return Err(build_users_me_management_token_reader_unavailable_response());
    }
    match state.get_management_token_with_user(token_id).await {
        Ok(Some(token)) if token.token.user_id == user_id => Ok(token),
        Ok(Some(_)) | Ok(None) => Err(build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        )),
        Err(err) => Err(build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("management token lookup failed: {err:?}"),
            false,
        )),
    }
}

pub(super) async fn handle_users_me_management_tokens_list(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_management_token_reader() {
        return build_users_me_management_token_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let is_active =
        query_param_optional_bool(request_context.request_query_string.as_deref(), "is_active");
    let skip = users_me_management_token_skip(request_context.request_query_string.as_deref());
    let limit = users_me_management_token_limit(request_context.request_query_string.as_deref());
    let page = match state
        .list_management_tokens(&ManagementTokenListQuery {
            user_id: Some(auth.user.id.clone()),
            is_active,
            offset: skip,
            limit,
        })
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("management token list failed: {err:?}"),
                false,
            )
        }
    };

    Json(json!({
        "items": page
            .items
            .iter()
            .map(|item| build_management_token_payload(&item.token, None))
            .collect::<Vec<_>>(),
        "total": page.total,
        "skip": skip,
        "limit": limit,
        "quota": {
            "used": page.total,
            "max": users_me_management_token_max_per_user(),
        },
    }))
    .into_response()
}

pub(super) async fn handle_users_me_management_token_create(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if !state.has_management_token_writer() {
        return build_users_me_management_token_writer_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let input = match users_me_parse_management_token_create_input(request_body) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };

    let existing = match list_users_me_management_tokens_for_user(
        state,
        &auth.user.id,
        None,
        USERS_ME_MANAGEMENT_TOKEN_FETCH_LIMIT,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let max_tokens = users_me_management_token_max_per_user();
    if existing.total >= max_tokens {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            format!("已达到 Token 数量上限（{max_tokens}）"),
            false,
        );
    }
    if existing
        .items
        .iter()
        .any(|item| item.token.name == input.name)
    {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            format!("已存在名为 '{}' 的 Token", input.name),
            false,
        );
    }

    let raw_token = generate_users_me_management_token_plaintext();
    let record = CreateManagementTokenRecord {
        id: Uuid::new_v4().to_string(),
        user_id: auth.user.id.clone(),
        user: match build_users_me_management_token_user_summary(&auth) {
            Ok(value) => value,
            Err(response) => return response,
        },
        token_hash: hash_users_me_management_token(&raw_token),
        token_prefix: users_me_management_token_prefix(&raw_token),
        name: input.name.clone(),
        description: input.description,
        allowed_ips: input.allowed_ips,
        expires_at_unix_secs: input.expires_at_unix_secs,
        is_active: true,
    };

    match state.create_management_token(&record).await {
        Ok(LocalMutationOutcome::Applied(token)) => (
            http::StatusCode::CREATED,
            Json(json!({
                "message": "Management Token 创建成功",
                "token": raw_token,
                "data": build_management_token_payload(&token, None),
            })),
        )
            .into_response(),
        Ok(LocalMutationOutcome::Invalid(detail)) => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
        Ok(LocalMutationOutcome::Unavailable) => {
            build_users_me_management_token_writer_unavailable_response()
        }
        Ok(LocalMutationOutcome::NotFound) => build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        ),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("management token create failed: {err:?}"),
            false,
        ),
    }
}

pub(super) async fn handle_users_me_management_token_detail_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(token_id) = users_me_management_token_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        );
    };
    match resolve_users_me_management_token(state, &auth.user.id, &token_id).await {
        Ok(token) => Json(build_management_token_payload(&token.token, None)).into_response(),
        Err(response) => response,
    }
}

pub(super) async fn handle_users_me_management_token_update(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if !state.has_management_token_writer() {
        return build_users_me_management_token_writer_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(token_id) = users_me_management_token_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        );
    };
    let existing = match resolve_users_me_management_token(state, &auth.user.id, &token_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let input = match users_me_parse_management_token_update_input(request_body) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if input.is_noop() {
        return Json(json!({
            "message": "更新成功",
            "data": build_management_token_payload(&existing.token, None),
        }))
        .into_response();
    }
    if let Some(name) = input.name.as_deref() {
        if name != existing.token.name {
            let page = match list_users_me_management_tokens_for_user(
                state,
                &auth.user.id,
                None,
                USERS_ME_MANAGEMENT_TOKEN_FETCH_LIMIT,
            )
            .await
            {
                Ok(value) => value,
                Err(response) => return response,
            };
            if page
                .items
                .iter()
                .any(|item| item.token.id != existing.token.id && item.token.name == name)
            {
                return build_auth_error_response(
                    http::StatusCode::BAD_REQUEST,
                    format!("已存在名为 '{}' 的 Token", name),
                    false,
                );
            }
        }
    }

    let record = UpdateManagementTokenRecord {
        token_id: existing.token.id.clone(),
        name: input.name,
        description: input.description,
        clear_description: input.clear_description,
        allowed_ips: input.allowed_ips,
        clear_allowed_ips: input.clear_allowed_ips,
        expires_at_unix_secs: input.expires_at_unix_secs,
        clear_expires_at: input.clear_expires_at,
        is_active: None,
    };

    match state.update_management_token(&record).await {
        Ok(LocalMutationOutcome::Applied(token)) => Json(json!({
            "message": "更新成功",
            "data": build_management_token_payload(&token, None),
        }))
        .into_response(),
        Ok(LocalMutationOutcome::NotFound) => build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        ),
        Ok(LocalMutationOutcome::Invalid(detail)) => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
        Ok(LocalMutationOutcome::Unavailable) => {
            build_users_me_management_token_writer_unavailable_response()
        }
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("management token update failed: {err:?}"),
            false,
        ),
    }
}

pub(super) async fn handle_users_me_management_token_delete(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_management_token_writer() {
        return build_users_me_management_token_writer_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(token_id) = users_me_management_token_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        );
    };
    let existing = match resolve_users_me_management_token(state, &auth.user.id, &token_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.delete_management_token(&existing.token.id).await {
        Ok(true) => Json(json!({ "message": "删除成功" })).into_response(),
        Ok(false) => build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        ),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("management token delete failed: {err:?}"),
            false,
        ),
    }
}

pub(super) async fn handle_users_me_management_token_toggle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_management_token_writer() {
        return build_users_me_management_token_writer_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(token_id) =
        users_me_management_token_status_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        );
    };
    let existing = match resolve_users_me_management_token(state, &auth.user.id, &token_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .set_management_token_active(&existing.token.id, !existing.token.is_active)
        .await
    {
        Ok(Some(token)) => Json(json!({
            "message": format!("Token 已{}", if token.is_active { "启用" } else { "禁用" }),
            "data": build_management_token_payload(&token, None),
        }))
        .into_response(),
        Ok(None) => build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        ),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("management token toggle failed: {err:?}"),
            false,
        ),
    }
}

pub(super) async fn handle_users_me_management_token_regenerate(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_management_token_writer() {
        return build_users_me_management_token_writer_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(token_id) =
        users_me_management_token_regenerate_id_from_path(&request_context.request_path)
    else {
        return build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        );
    };
    let existing = match resolve_users_me_management_token(state, &auth.user.id, &token_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let raw_token = generate_users_me_management_token_plaintext();
    let mutation = RegenerateManagementTokenSecret {
        token_id: existing.token.id.clone(),
        token_hash: hash_users_me_management_token(&raw_token),
        token_prefix: users_me_management_token_prefix(&raw_token),
    };

    match state.regenerate_management_token_secret(&mutation).await {
        Ok(LocalMutationOutcome::Applied(token)) => Json(json!({
            "message": "Token 已重新生成",
            "token": raw_token,
            "data": build_management_token_payload(&token, None),
        }))
        .into_response(),
        Ok(LocalMutationOutcome::NotFound) => build_auth_error_response(
            http::StatusCode::NOT_FOUND,
            "Management Token 不存在",
            false,
        ),
        Ok(LocalMutationOutcome::Invalid(detail)) => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
        Ok(LocalMutationOutcome::Unavailable) => {
            build_users_me_management_token_writer_unavailable_response()
        }
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("management token regenerate failed: {err:?}"),
            false,
        ),
    }
}
