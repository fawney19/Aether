use crate::control::{
    management_token_permission_catalog_payload,
    management_token_permissions_cover_all_assignable_permissions,
    normalize_assignable_management_token_permissions,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_optional_bool, query_param_value};
use crate::handlers::admin::system::shared::paths::{
    admin_management_token_id_from_path, admin_management_token_regenerate_id_from_path,
    admin_management_token_status_id_from_path, is_admin_management_tokens_root,
};
use crate::handlers::internal::build_management_token_payload;
use crate::handlers::shared::{generate_gateway_secret_plaintext, parse_json_ip_rules};
use crate::{GatewayError, LocalMutationOutcome};
use aether_data::repository::management_tokens::{
    CreateManagementTokenRecord, ManagementTokenListQuery, RegenerateManagementTokenSecret,
    StoredManagementTokenUserSummary, UpdateManagementTokenRecord,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const ADMIN_MANAGEMENT_TOKEN_PREFIX: &str = "ae";
const ADMIN_MANAGEMENT_TOKEN_SEPARATOR: &str = "-";
const ADMIN_MANAGEMENT_TOKEN_DISPLAY_PREFIX_LEN: usize = 10;

fn admin_management_token_bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn admin_management_token_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Management Token 不存在" })),
    )
        .into_response()
}

fn admin_management_token_read_only_response() -> Response<Body> {
    (
        http::StatusCode::CONFLICT,
        Json(json!({
            "detail": "Management Token 本地写入仓库不可用",
            "error_code": "read_only_mode",
        })),
    )
        .into_response()
}

#[derive(Debug, Clone)]
struct AdminManagementTokenCreateInput {
    name: String,
    description: Option<String>,
    allowed_ips: Option<serde_json::Value>,
    permissions: serde_json::Value,
    expires_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct AdminManagementTokenUpdateInput {
    name: Option<String>,
    description: Option<String>,
    clear_description: bool,
    allowed_ips: Option<serde_json::Value>,
    clear_allowed_ips: bool,
    permissions: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
    clear_expires_at: bool,
}

fn generate_admin_management_token_plaintext() -> String {
    generate_gateway_secret_plaintext(
        ADMIN_MANAGEMENT_TOKEN_PREFIX,
        ADMIN_MANAGEMENT_TOKEN_SEPARATOR,
    )
}

fn hash_admin_management_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn admin_management_token_prefix(value: &str) -> Option<String> {
    (!value.is_empty())
        .then(|| value[..value.len().min(ADMIN_MANAGEMENT_TOKEN_DISPLAY_PREFIX_LEN)].to_string())
}

fn admin_parse_management_token_allowed_ips(
    value: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    parse_json_ip_rules(value)
}

fn admin_parse_management_token_expires_at(
    value: &serde_json::Value,
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
            if unix_secs <= Utc::now().timestamp() {
                return Err("过期时间必须在未来".to_string());
            }
            u64::try_from(unix_secs)
                .map(Some)
                .map_err(|_| format!("无效的时间格式: {raw}"))
        }
        _ => Err("expires_at 必须是字符串或 null".to_string()),
    }
}

fn admin_parse_management_token_create_input(
    request_body: &[u8],
) -> Result<AdminManagementTokenCreateInput, String> {
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
    let allowed_ips = admin_parse_management_token_allowed_ips(payload.get("allowed_ips"))?;
    let permissions =
        normalize_assignable_management_token_permissions(payload.get("permissions"))?;
    let expires_at_unix_secs = match payload.get("expires_at") {
        Some(value) => admin_parse_management_token_expires_at(value)?,
        None => None,
    };

    Ok(AdminManagementTokenCreateInput {
        name,
        description,
        allowed_ips,
        permissions,
        expires_at_unix_secs,
    })
}

fn admin_parse_management_token_update_input(
    request_body: &[u8],
) -> Result<AdminManagementTokenUpdateInput, String> {
    let payload =
        serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(request_body)
            .map_err(|_| "输入验证失败".to_string())?;
    let mut input = AdminManagementTokenUpdateInput::default();

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
            input.allowed_ips = admin_parse_management_token_allowed_ips(Some(value))?;
        }
    }
    if let Some(value) = payload.get("permissions") {
        input.permissions = Some(normalize_assignable_management_token_permissions(Some(
            value,
        ))?);
    }
    if let Some(value) = payload.get("expires_at") {
        if value.is_null() || value.as_str().is_some_and(|value| value.trim().is_empty()) {
            input.clear_expires_at = true;
        } else {
            input.expires_at_unix_secs = admin_parse_management_token_expires_at(value)?;
        }
    }

    Ok(input)
}

async fn admin_management_token_user_summary(
    state: &AdminAppState<'_>,
    user_id: &str,
) -> Result<StoredManagementTokenUserSummary, Response<Body>> {
    let Some(user) = state.find_user_auth_by_id(user_id).await.map_err(|err| {
        (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": format!("management token user lookup failed: {err:?}") })),
        )
            .into_response()
    })?
    else {
        return Err((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "管理员用户不存在" })),
        )
            .into_response());
    };
    StoredManagementTokenUserSummary::new(
        user.id,
        user.email,
        user.username,
        user.role,
    )
    .map_err(|err| {
        (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": format!("management token user summary build failed: {err:?}") })),
        )
            .into_response()
    })
}

pub(crate) async fn maybe_build_local_admin_management_tokens_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("management_tokens_manage") {
        return Ok(None);
    }

    let is_management_token = decision
        .admin_principal
        .as_ref()
        .and_then(|principal| principal.management_token_id.as_deref())
        .is_some();
    let management_token_is_full = decision
        .admin_principal
        .as_ref()
        .and_then(|principal| principal.management_token_permissions.as_deref())
        .is_none_or(management_token_permissions_cover_all_assignable_permissions);
    if is_management_token && !management_token_is_full {
        return Ok(Some(
            (
                http::StatusCode::FORBIDDEN,
                Json(json!({
                    "detail": "不允许使用 Management Token 管理其他 Token，请使用 Web 界面或 JWT 认证"
                })),
            )
                .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("permissions_catalog")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            Json(management_token_permission_catalog_payload()).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("list_tokens")
        && request_context.method() == http::Method::GET
        && is_admin_management_tokens_root(request_context.path())
    {
        if !state.has_management_token_reader() {
            return Ok(None);
        }
        let user_id = query_param_value(request_context.query_string(), "user_id");
        let is_active = query_param_optional_bool(request_context.query_string(), "is_active");
        let skip = query_param_value(request_context.query_string(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.query_string(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 100)
            .unwrap_or(50);
        let page = state
            .list_management_tokens(&ManagementTokenListQuery {
                user_id,
                is_active,
                offset: skip,
                limit,
            })
            .await?;
        let items = page
            .items
            .iter()
            .map(|item| build_management_token_payload(&item.token, Some(&item.user)))
            .collect::<Vec<_>>();
        return Ok(Some(
            Json(json!({
                "items": items,
                "total": page.total,
                "skip": skip,
                "limit": limit,
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("create_token")
        && request_context.method() == http::Method::POST
        && is_admin_management_tokens_root(request_context.path())
    {
        if !state.has_management_token_writer() {
            return Ok(None);
        }
        let Some(request_body) = request_body else {
            return Ok(Some(admin_management_token_bad_request_response(
                "缺少请求体",
            )));
        };
        let input = match admin_parse_management_token_create_input(request_body) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_management_token_bad_request_response(detail))),
        };
        let Some(admin_principal) = decision.admin_principal.as_ref() else {
            return Ok(None);
        };
        let user = match admin_management_token_user_summary(state, &admin_principal.user_id).await
        {
            Ok(value) => value,
            Err(response) => return Ok(Some(response)),
        };
        let raw_token = generate_admin_management_token_plaintext();
        let record = CreateManagementTokenRecord {
            id: Uuid::new_v4().to_string(),
            user_id: user.id.clone(),
            user,
            token_hash: hash_admin_management_token(&raw_token),
            token_prefix: admin_management_token_prefix(&raw_token),
            name: input.name.clone(),
            description: input.description,
            allowed_ips: input.allowed_ips,
            permissions: Some(input.permissions),
            expires_at_unix_secs: input.expires_at_unix_secs,
            is_active: true,
        };

        return Ok(Some(match state.create_management_token(&record).await? {
            LocalMutationOutcome::Applied(token) => (
                http::StatusCode::CREATED,
                Json(json!({
                    "message": "Management Token 创建成功",
                    "token": raw_token,
                    "data": build_management_token_payload(&token, Some(&record.user)),
                })),
            )
                .into_response(),
            LocalMutationOutcome::Invalid(detail) => {
                admin_management_token_bad_request_response(detail)
            }
            LocalMutationOutcome::Unavailable => admin_management_token_read_only_response(),
            LocalMutationOutcome::NotFound => admin_management_token_not_found_response(),
        }));
    }

    if decision.route_kind.as_deref() == Some("get_token")
        && request_context.method() == http::Method::GET
    {
        if !state.has_management_token_reader() {
            return Ok(None);
        }
        let Some(token_id) = admin_management_token_id_from_path(request_context.path()) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Management Token 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state.get_management_token_with_user(&token_id).await? {
                Some(token) => Json(build_management_token_payload(
                    &token.token,
                    Some(&token.user),
                ))
                .into_response(),
                None => admin_management_token_not_found_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("update_token")
        && request_context.method() == http::Method::PUT
    {
        if !state.has_management_token_writer() {
            return Ok(None);
        }
        let Some(token_id) = admin_management_token_id_from_path(request_context.path()) else {
            return Ok(Some(admin_management_token_not_found_response()));
        };
        let existing = match state.get_management_token_with_user(&token_id).await? {
            Some(token) => token,
            None => return Ok(Some(admin_management_token_not_found_response())),
        };
        let Some(request_body) = request_body else {
            return Ok(Some(admin_management_token_bad_request_response(
                "缺少请求体",
            )));
        };
        let input = match admin_parse_management_token_update_input(request_body) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_management_token_bad_request_response(detail))),
        };
        let record = UpdateManagementTokenRecord {
            token_id: existing.token.id.clone(),
            name: input.name,
            description: input.description,
            clear_description: input.clear_description,
            allowed_ips: input.allowed_ips,
            clear_allowed_ips: input.clear_allowed_ips,
            permissions: input.permissions,
            expires_at_unix_secs: input.expires_at_unix_secs,
            clear_expires_at: input.clear_expires_at,
            is_active: None,
        };

        return Ok(Some(match state.update_management_token(&record).await? {
            LocalMutationOutcome::Applied(token) => Json(json!({
                "message": "更新成功",
                "data": build_management_token_payload(&token, Some(&existing.user)),
            }))
            .into_response(),
            LocalMutationOutcome::NotFound => admin_management_token_not_found_response(),
            LocalMutationOutcome::Invalid(detail) => {
                admin_management_token_bad_request_response(detail)
            }
            LocalMutationOutcome::Unavailable => admin_management_token_read_only_response(),
        }));
    }

    if decision.route_kind.as_deref() == Some("delete_token")
        && request_context.method() == http::Method::DELETE
    {
        if !state.has_management_token_writer() {
            return Ok(None);
        }
        let Some(token_id) = admin_management_token_id_from_path(request_context.path()) else {
            return Ok(Some(admin_management_token_not_found_response()));
        };
        let existing = match state.get_management_token_with_user(&token_id).await? {
            Some(token) => token,
            None => return Ok(Some(admin_management_token_not_found_response())),
        };
        let deleted = state.delete_management_token(&existing.token.id).await?;
        return Ok(Some(if deleted {
            Json(json!({ "message": "删除成功" })).into_response()
        } else {
            admin_management_token_not_found_response()
        }));
    }

    if decision.route_kind.as_deref() == Some("toggle_status")
        && request_context.method() == http::Method::PATCH
    {
        if !state.has_management_token_writer() {
            return Ok(None);
        }
        let Some(token_id) = admin_management_token_status_id_from_path(request_context.path())
        else {
            return Ok(Some(admin_management_token_not_found_response()));
        };
        let existing = match state.get_management_token_with_user(&token_id).await? {
            Some(token) => token,
            None => return Ok(Some(admin_management_token_not_found_response())),
        };
        let Some(updated) = state
            .set_management_token_active(&existing.token.id, !existing.token.is_active)
            .await?
        else {
            return Ok(Some(admin_management_token_not_found_response()));
        };
        return Ok(Some(
            Json(json!({
                "message": format!("Token 已{}", if updated.is_active { "启用" } else { "禁用" }),
                "data": build_management_token_payload(&updated, Some(&existing.user)),
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("regenerate_token")
        && request_context.method() == http::Method::POST
    {
        if !state.has_management_token_writer() {
            return Ok(None);
        }
        let Some(token_id) = admin_management_token_regenerate_id_from_path(request_context.path())
        else {
            return Ok(Some(admin_management_token_not_found_response()));
        };
        let existing = match state.get_management_token_with_user(&token_id).await? {
            Some(token) => token,
            None => return Ok(Some(admin_management_token_not_found_response())),
        };
        let raw_token = generate_admin_management_token_plaintext();
        let mutation = RegenerateManagementTokenSecret {
            token_id: existing.token.id.clone(),
            token_hash: hash_admin_management_token(&raw_token),
            token_prefix: admin_management_token_prefix(&raw_token),
        };

        return Ok(Some(
            match state.regenerate_management_token_secret(&mutation).await? {
                LocalMutationOutcome::Applied(token) => Json(json!({
                    "message": "Token 已重新生成",
                    "token": raw_token,
                    "data": build_management_token_payload(&token, Some(&existing.user)),
                }))
                .into_response(),
                LocalMutationOutcome::NotFound => admin_management_token_not_found_response(),
                LocalMutationOutcome::Invalid(detail) => {
                    admin_management_token_bad_request_response(detail)
                }
                LocalMutationOutcome::Unavailable => admin_management_token_read_only_response(),
            },
        ));
    }

    Ok(None)
}
