use super::{
    build_admin_users_bad_request_response, build_admin_users_read_only_response,
    format_optional_datetime_iso8601, normalize_admin_user_api_formats,
    normalize_admin_user_string_list,
};
use crate::constants::DEFAULT_USER_GROUP_CONFIG_KEY;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Debug, serde::Deserialize)]
struct AdminUserGroupPayload {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    allowed_providers: Option<Vec<String>>,
    #[serde(default = "default_list_mode")]
    allowed_providers_mode: String,
    #[serde(default)]
    allowed_api_formats: Option<Vec<String>>,
    #[serde(default = "default_list_mode")]
    allowed_api_formats_mode: String,
    #[serde(default)]
    allowed_models: Option<Vec<String>>,
    #[serde(default = "default_list_mode")]
    allowed_models_mode: String,
    #[serde(default)]
    rate_limit: Option<i32>,
    #[serde(default = "default_rate_limit_mode")]
    rate_limit_mode: String,
    #[serde(default)]
    daily_usage_limit_usd: Option<f64>,
    #[serde(default = "default_rate_limit_mode")]
    daily_usage_limit_mode: String,
}

#[derive(Debug, serde::Deserialize)]
struct AdminUserGroupMembersPayload {
    user_ids: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct AdminDefaultUserGroupPayload {
    #[serde(default)]
    group_id: Option<String>,
}

pub(in super::super) async fn build_admin_list_user_groups_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let default_group_id = read_default_user_group_id(state).await?;
    let items = state
        .list_user_groups()
        .await?
        .into_iter()
        .map(|group| user_group_payload(group, default_group_id.as_deref()))
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "items": items,
        "default_group_id": default_group_id,
    }))
    .into_response())
}

pub(in super::super) async fn build_admin_create_user_group_response(
    state: &AdminAppState<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法创建用户分组",
        ));
    }
    let record = match parse_group_record(request_body) {
        Ok(value) => value,
        Err(detail) => return Ok(bad_request_owned(detail)),
    };
    let group = match state.create_user_group(record).await {
        Ok(Some(group)) => group,
        Ok(None) => {
            return Ok(build_admin_users_read_only_response(
                "当前为只读模式，无法创建用户分组",
            ))
        }
        Err(err) if is_duplicate_group_name_error(&err) => {
            return Ok(bad_request_owned("用户分组名称已存在".to_string()))
        }
        Err(err) => return Err(err),
    };
    let default_group_id = read_default_user_group_id(state).await?;
    Ok(attach_admin_audit_response(
        Json(user_group_payload(group, default_group_id.as_deref())).into_response(),
        "admin_user_group_created",
        "create_user_group",
        "user_group",
        "user_groups",
    ))
}

pub(in super::super) async fn build_admin_update_user_group_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法更新用户分组",
        ));
    }
    let Some(group_id) = user_group_id_from_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 group_id"));
    };
    let record = match parse_group_record(request_body) {
        Ok(value) => value,
        Err(detail) => return Ok(bad_request_owned(detail)),
    };
    let group = match state.update_user_group(&group_id, record).await {
        Ok(Some(group)) => group,
        Ok(None) => return Ok(not_found("用户分组不存在")),
        Err(err) if is_duplicate_group_name_error(&err) => {
            return Ok(bad_request_owned("用户分组名称已存在".to_string()))
        }
        Err(err) => return Err(err),
    };
    let default_group_id = read_default_user_group_id(state).await?;
    Ok(attach_admin_audit_response(
        Json(user_group_payload(group, default_group_id.as_deref())).into_response(),
        "admin_user_group_updated",
        "update_user_group",
        "user_group",
        &group_id,
    ))
}

pub(in super::super) async fn build_admin_delete_user_group_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法删除用户分组",
        ));
    }
    let Some(group_id) = user_group_id_from_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 group_id"));
    };
    if read_default_user_group_id(state).await?.as_deref() == Some(group_id.as_str()) {
        return Ok(bad_request_owned("默认用户组不能删除".to_string()));
    }
    if !state.delete_user_group(&group_id).await? {
        return Ok(not_found("用户分组不存在"));
    }
    Ok(attach_admin_audit_response(
        Json(json!({ "deleted": true })).into_response(),
        "admin_user_group_deleted",
        "delete_user_group",
        "user_group",
        &group_id,
    ))
}

pub(in super::super) async fn build_admin_list_user_group_members_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(group_id) = user_group_member_group_id_from_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 group_id"));
    };
    if state.find_user_group_by_id(&group_id).await?.is_none() {
        return Ok(not_found("用户分组不存在"));
    }
    let items = state
        .list_user_group_members(&group_id)
        .await?
        .into_iter()
        .map(|member| {
            json!({
                "group_id": member.group_id,
                "user_id": member.user_id,
                "username": member.username,
                "email": member.email,
                "role": member.role,
                "is_active": member.is_active,
                "is_deleted": member.is_deleted,
                "created_at": format_optional_datetime_iso8601(member.created_at),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "items": items })).into_response())
}

pub(in super::super) async fn build_admin_replace_user_group_members_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法更新分组成员",
        ));
    }
    let Some(group_id) = user_group_member_group_id_from_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 group_id"));
    };
    if state.find_user_group_by_id(&group_id).await?.is_none() {
        return Ok(not_found("用户分组不存在"));
    }
    let payload = match parse_members_payload(request_body) {
        Ok(value) => value,
        Err(detail) => return Ok(bad_request_owned(detail)),
    };
    let user_ids = normalize_ids(payload.user_ids);
    if read_default_user_group_id(state).await?.as_deref() == Some(group_id.as_str()) {
        if let Some(response) =
            validate_default_group_member_replacement(state, &group_id, &user_ids).await?
        {
            return Ok(response);
        }
    }
    let known_users = state.resolve_auth_user_summaries_by_ids(&user_ids).await?;
    if known_users.len() != user_ids.len() {
        return Ok(bad_request_owned("成员包含不存在的用户".to_string()));
    }
    let items = state
        .replace_user_group_members(&group_id, &user_ids)
        .await?;
    Ok(attach_admin_audit_response(
        Json(json!({
            "items": items.into_iter().map(|member| json!({
                "group_id": member.group_id,
                "user_id": member.user_id,
                "username": member.username,
                "email": member.email,
                "role": member.role,
                "is_active": member.is_active,
                "is_deleted": member.is_deleted,
                "created_at": format_optional_datetime_iso8601(member.created_at),
            })).collect::<Vec<_>>()
        }))
        .into_response(),
        "admin_user_group_members_updated",
        "update_user_group_members",
        "user_group",
        &group_id,
    ))
}

async fn validate_default_group_member_replacement(
    state: &AdminAppState<'_>,
    group_id: &str,
    next_user_ids: &[String],
) -> Result<Option<Response<Body>>, GatewayError> {
    let next_user_ids = next_user_ids.iter().cloned().collect::<BTreeSet<String>>();
    let removed_user_ids = state
        .list_user_group_members(group_id)
        .await?
        .into_iter()
        .filter(|member| !next_user_ids.contains(&member.user_id))
        .map(|member| member.user_id)
        .collect::<Vec<_>>();
    if removed_user_ids.is_empty() {
        return Ok(None);
    }

    let summaries = state
        .resolve_auth_user_summaries_by_ids(&removed_user_ids)
        .await?;
    let users_with_other_groups = state
        .list_user_group_memberships_by_user_ids(&removed_user_ids)
        .await?
        .into_iter()
        .filter(|membership| membership.group_id != group_id)
        .map(|membership| membership.user_id)
        .collect::<BTreeSet<_>>();

    for user_id in removed_user_ids {
        let Some(summary) = summaries.get(&user_id) else {
            continue;
        };
        if crate::roles::can_access_admin_console(&summary.role) {
            continue;
        }
        if !users_with_other_groups.contains(&user_id) {
            return Ok(Some(bad_request_owned(format!(
                "用户 {} 移出默认组后将不属于任何用户组",
                summary.username
            ))));
        }
    }

    Ok(None)
}

pub(in super::super) async fn build_admin_set_default_user_group_response(
    state: &AdminAppState<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_user_write_capability() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法设置默认用户组",
        ));
    }
    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            serde_json::from_slice::<AdminDefaultUserGroupPayload>(body)
                .map_err(|_| "请求数据验证失败".to_string())
        }
        _ => Err("请求数据验证失败".to_string()),
    };
    let payload = match payload {
        Ok(value) => value,
        Err(detail) => return Ok(bad_request_owned(detail)),
    };
    let group_id = payload
        .group_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(group_id) = group_id.as_deref() {
        if state.find_user_group_by_id(group_id).await?.is_none() {
            return Ok(bad_request_owned("默认用户组不存在".to_string()));
        }
        state
            .upsert_system_config_json_value(
                DEFAULT_USER_GROUP_CONFIG_KEY,
                &json!(group_id),
                Some("Default group for self-registered users"),
            )
            .await?;
    } else {
        state
            .delete_system_config_value(DEFAULT_USER_GROUP_CONFIG_KEY)
            .await?;
    }
    let effective_group_id = read_default_user_group_id(state).await?;
    if let Some(group_id) = effective_group_id.as_deref() {
        state.add_all_users_to_group(group_id).await?;
    }
    Ok(attach_admin_audit_response(
        Json(json!({ "default_group_id": effective_group_id })).into_response(),
        "admin_default_user_group_set",
        "set_default_user_group",
        "user_group",
        group_id.as_deref().unwrap_or("default_user_group"),
    ))
}

pub(crate) async fn read_default_user_group_id(
    state: &AdminAppState<'_>,
) -> Result<Option<String>, GatewayError> {
    state.effective_default_user_group_id().await
}

fn parse_group_record(
    request_body: Option<&axum::body::Bytes>,
) -> Result<aether_data::repository::users::UpsertUserGroupRecord, String> {
    let Some(body) = request_body.filter(|body| !body.is_empty()) else {
        return Err("请求数据验证失败".to_string());
    };
    let payload = serde_json::from_slice::<AdminUserGroupPayload>(body)
        .map_err(|_| "请求数据验证失败".to_string())?;
    let name = aether_data::repository::users::normalize_user_group_name(&payload.name);
    if name.is_empty() {
        return Err("分组名称不能为空".to_string());
    }
    if payload.rate_limit.is_some_and(|value| value < 0) {
        return Err("rate_limit 必须大于等于 0".to_string());
    }
    if payload
        .daily_usage_limit_usd
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err("daily_usage_limit_usd 必须是大于等于 0 的有限数值".to_string());
    }
    let allowed_providers =
        normalize_admin_user_string_list(payload.allowed_providers, "allowed_providers")?;
    let allowed_api_formats = normalize_admin_user_api_formats(payload.allowed_api_formats)?;
    let allowed_models =
        normalize_admin_user_string_list(payload.allowed_models, "allowed_models")?;
    Ok(aether_data::repository::users::UpsertUserGroupRecord {
        name,
        description: payload
            .description
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        priority: 0,
        allowed_providers,
        allowed_providers_mode: normalize_list_mode(&payload.allowed_providers_mode)?,
        allowed_api_formats,
        allowed_api_formats_mode: normalize_list_mode(&payload.allowed_api_formats_mode)?,
        allowed_models,
        allowed_models_mode: normalize_list_mode(&payload.allowed_models_mode)?,
        rate_limit: payload.rate_limit,
        rate_limit_mode: normalize_rate_mode(&payload.rate_limit_mode)?,
        daily_usage_limit_usd: payload.daily_usage_limit_usd,
        daily_usage_limit_mode: normalize_rate_mode(&payload.daily_usage_limit_mode)?,
    })
}

fn parse_members_payload(
    request_body: Option<&axum::body::Bytes>,
) -> Result<AdminUserGroupMembersPayload, String> {
    let Some(body) = request_body.filter(|body| !body.is_empty()) else {
        return Err("请求数据验证失败".to_string());
    };
    serde_json::from_slice::<AdminUserGroupMembersPayload>(body)
        .map_err(|_| "请求数据验证失败".to_string())
}

fn user_group_payload(
    group: aether_data::repository::users::StoredUserGroup,
    default_group_id: Option<&str>,
) -> serde_json::Value {
    json!({
        "id": group.id,
        "name": group.name,
        "normalized_name": group.normalized_name,
        "description": group.description,
        "allowed_providers": group.allowed_providers,
        "allowed_providers_mode": group.allowed_providers_mode,
        "allowed_api_formats": group.allowed_api_formats,
        "allowed_api_formats_mode": group.allowed_api_formats_mode,
        "allowed_models": group.allowed_models,
        "allowed_models_mode": group.allowed_models_mode,
        "rate_limit": group.rate_limit,
        "rate_limit_mode": group.rate_limit_mode,
        "daily_usage_limit_usd": group.daily_usage_limit_usd,
        "daily_usage_limit_mode": group.daily_usage_limit_mode,
        "is_default": default_group_id == Some(group.id.as_str()),
        "created_at": format_optional_datetime_iso8601(group.created_at),
        "updated_at": format_optional_datetime_iso8601(group.updated_at),
    })
}

fn normalize_list_mode(value: &str) -> Result<String, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "inherit" | "unrestricted" | "specific" | "deny_all" => {
            Ok(value.trim().to_ascii_lowercase())
        }
        _ => Err("权限列表模式不合法".to_string()),
    }
}

fn normalize_rate_mode(value: &str) -> Result<String, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "inherit" | "system" | "custom" => Ok(value.trim().to_ascii_lowercase()),
        _ => Err("限速模式不合法".to_string()),
    }
}

fn default_list_mode() -> String {
    "inherit".to_string()
}

fn default_rate_limit_mode() -> String {
    "inherit".to_string()
}

fn normalize_ids(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn user_group_id_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/user-groups/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') || value == "default" {
        None
    } else {
        Some(value)
    }
}

fn user_group_member_group_id_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/user-groups/")?
        .trim()
        .trim_matches('/');
    let group_id = value.strip_suffix("/members")?.trim_matches('/');
    if group_id.is_empty() || group_id.contains('/') {
        None
    } else {
        Some(group_id.to_string())
    }
}

fn bad_request_owned(detail: String) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn not_found(detail: &'static str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn is_duplicate_group_name_error(err: &GatewayError) -> bool {
    match err {
        GatewayError::Internal(message) => message.contains("duplicate user group name"),
        _ => false,
    }
}
