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
use std::collections::{BTreeMap, BTreeSet};

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
    provider_key_policies: BTreeMap<String, Vec<String>>,
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
    let record = match state
        .normalize_user_group_provider_access_record(record)
        .await?
    {
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
    let record = match state
        .normalize_user_group_provider_access_record(record)
        .await?
    {
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
    let allowed_providers =
        normalize_admin_user_string_list(payload.allowed_providers, "allowed_providers")?;
    let allowed_providers_mode = normalize_list_mode(&payload.allowed_providers_mode)?;
    let provider_key_policies = normalize_provider_key_policy_input(payload.provider_key_policies)?;
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
        allowed_providers_mode,
        provider_key_policies,
        allowed_api_formats,
        allowed_api_formats_mode: normalize_list_mode(&payload.allowed_api_formats_mode)?,
        allowed_models,
        allowed_models_mode: normalize_list_mode(&payload.allowed_models_mode)?,
        rate_limit: payload.rate_limit,
        rate_limit_mode: normalize_rate_mode(&payload.rate_limit_mode)?,
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
        "provider_key_policies": group.provider_key_policies,
        "allowed_api_formats": group.allowed_api_formats,
        "allowed_api_formats_mode": group.allowed_api_formats_mode,
        "allowed_models": group.allowed_models,
        "allowed_models_mode": group.allowed_models_mode,
        "rate_limit": group.rate_limit,
        "rate_limit_mode": group.rate_limit_mode,
        "is_default": default_group_id == Some(group.id.as_str()),
        "created_at": format_optional_datetime_iso8601(group.created_at),
        "updated_at": format_optional_datetime_iso8601(group.updated_at),
    })
}

fn normalize_provider_key_policy_input(
    policies: BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    policies
        .into_iter()
        .map(|(provider_id, key_ids)| {
            let provider_id = provider_id.trim();
            if provider_id.is_empty() {
                return Err("provider_key_policies 包含空 Provider 标识".to_string());
            }
            let mut normalized_key_ids = BTreeSet::new();
            for key_id in key_ids {
                let key_id = key_id.trim();
                if key_id.is_empty() {
                    return Err(format!("Provider {provider_id} 的 Key 白名单包含空标识"));
                }
                normalized_key_ids.insert(key_id.to_string());
            }
            Ok((
                provider_id.to_string(),
                normalized_key_ids.into_iter().collect(),
            ))
        })
        .collect()
}

impl AdminAppState<'_> {
    pub(crate) async fn normalize_user_group_provider_access_record(
        &self,
        mut record: aether_data::repository::users::UpsertUserGroupRecord,
    ) -> Result<Result<aether_data::repository::users::UpsertUserGroupRecord, String>, GatewayError>
    {
        if record.allowed_providers_mode != "specific" {
            record.provider_key_policies.clear();
            return Ok(Ok(record));
        }
        if !self.has_provider_catalog_data_reader() {
            if record.provider_key_policies.is_empty() {
                return Ok(Ok(record));
            }
            return Err(GatewayError::Internal(
                "provider catalog unavailable while validating user group key policies".to_string(),
            ));
        }

        let providers = self.list_provider_catalog_providers(false).await?;
        let allowed_provider_ids = match resolve_allowed_provider_ids(
            record.allowed_providers.as_deref().unwrap_or_default(),
            &providers,
        ) {
            Ok(value) => value,
            Err(detail) => return Ok(Err(detail)),
        };
        if record.provider_key_policies.is_empty() {
            return Ok(Ok(record));
        }
        let policies = match canonicalize_provider_key_policy_providers(
            std::mem::take(&mut record.provider_key_policies),
            &allowed_provider_ids,
            &providers,
        ) {
            Ok(value) => value,
            Err(detail) => return Ok(Err(detail)),
        };
        let policy_provider_ids = policies.keys().cloned().collect::<Vec<_>>();
        let keys = self
            .list_provider_catalog_keys_by_provider_ids(&policy_provider_ids)
            .await?;
        let policies = match canonicalize_provider_key_policy_keys(policies, &keys) {
            Ok(value) => value,
            Err(detail) => return Ok(Err(detail)),
        };

        record.provider_key_policies = policies;
        Ok(Ok(record))
    }
}

fn resolve_allowed_provider_ids(
    allowed_providers: &[String],
    providers: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider],
) -> Result<BTreeSet<String>, String> {
    let mut provider_ids = BTreeSet::new();
    for reference in allowed_providers {
        let matches = matching_providers(reference, providers);
        if matches.is_empty() {
            return Err(format!("允许的 Provider 不存在: {}", reference.trim()));
        }
        provider_ids.extend(matches.into_iter().map(|provider| provider.id.clone()));
    }
    Ok(provider_ids)
}

fn canonicalize_provider_key_policy_providers(
    policies: BTreeMap<String, Vec<String>>,
    allowed_provider_ids: &BTreeSet<String>,
    providers: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider],
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut canonical = BTreeMap::new();
    for (provider_reference, key_ids) in policies {
        let mut matches = matching_providers(&provider_reference, providers);
        if matches.is_empty() {
            return Err(format!(
                "Key 策略引用的 Provider 不存在: {provider_reference}"
            ));
        }
        if matches.len() != 1 {
            return Err(format!(
                "Key 策略的 Provider 标识不唯一，请使用 Provider ID: {provider_reference}"
            ));
        }
        let provider_id = matches.remove(0).id.clone();
        if !allowed_provider_ids.contains(&provider_id) {
            return Err(format!(
                "Key 策略引用了未授权的 Provider: {provider_reference}"
            ));
        }
        if canonical.insert(provider_id.clone(), key_ids).is_some() {
            return Err(format!("Provider {provider_id} 存在重复的 Key 策略"));
        }
    }
    Ok(canonical)
}

fn matching_providers<'a>(
    reference: &str,
    providers: &'a [aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider],
) -> Vec<&'a aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider> {
    let reference = reference.trim();
    let exact_ids = providers
        .iter()
        .filter(|provider| provider.id.eq_ignore_ascii_case(reference))
        .collect::<Vec<_>>();
    if !exact_ids.is_empty() {
        return exact_ids;
    }
    let exact_names = providers
        .iter()
        .filter(|provider| provider.name.eq_ignore_ascii_case(reference))
        .collect::<Vec<_>>();
    if !exact_names.is_empty() {
        return exact_names;
    }
    providers
        .iter()
        .filter(|provider| provider.provider_type.eq_ignore_ascii_case(reference))
        .collect()
}

fn canonicalize_provider_key_policy_keys(
    policies: BTreeMap<String, Vec<String>>,
    keys: &[aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey],
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut canonical = BTreeMap::new();
    for (provider_id, key_references) in policies {
        let provider_keys = keys
            .iter()
            .filter(|key| key.provider_id == provider_id)
            .collect::<Vec<_>>();
        let mut key_ids = BTreeSet::new();
        for key_reference in key_references {
            if let Some(key) = keys
                .iter()
                .find(|key| key.id.eq_ignore_ascii_case(&key_reference))
            {
                if key.provider_id != provider_id {
                    return Err(format!(
                        "Provider Key {key_reference} 不属于 Provider {provider_id}"
                    ));
                }
                key_ids.insert(key.id.clone());
                continue;
            }

            let name_matches = provider_keys
                .iter()
                .filter(|key| key.name.eq_ignore_ascii_case(&key_reference))
                .copied()
                .collect::<Vec<_>>();
            match name_matches.as_slice() {
                [] => return Err(format!("Provider Key 不存在: {key_reference}")),
                [key] => {
                    key_ids.insert(key.id.clone());
                }
                _ => {
                    return Err(format!(
                        "Provider Key 名称不唯一，请使用 Key ID: {key_reference}"
                    ));
                }
            }
        }
        canonical.insert(provider_id, key_ids.into_iter().collect());
    }
    Ok(canonical)
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    use super::{
        canonicalize_provider_key_policy_keys, canonicalize_provider_key_policy_providers,
        resolve_allowed_provider_ids,
    };

    fn provider(id: &str, name: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            id.to_string(),
            name.to_string(),
            None,
            "custom".to_string(),
        )
        .expect("provider should build")
    }

    fn key(id: &str, provider_id: &str, name: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            id.to_string(),
            provider_id.to_string(),
            name.to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
    }

    #[test]
    fn key_policy_rejects_provider_outside_the_group_allowlist() {
        let providers = vec![provider("provider-a", "A"), provider("provider-b", "B")];
        let error = canonicalize_provider_key_policy_providers(
            BTreeMap::from([("provider-b".to_string(), vec!["key-b".to_string()])]),
            &BTreeSet::from(["provider-a".to_string()]),
            &providers,
        )
        .expect_err("policy provider must already be authorized");

        assert!(error.contains("未授权的 Provider"));
    }

    #[test]
    fn key_policy_rejects_key_owned_by_another_provider() {
        let policies = BTreeMap::from([("provider-a".to_string(), vec!["key-b".to_string()])]);
        let keys = vec![key("key-b", "provider-b", "B key")];

        let error = canonicalize_provider_key_policy_keys(policies, &keys)
            .expect_err("cross-provider key references must fail");

        assert!(error.contains("不属于 Provider provider-a"));
    }

    #[test]
    fn empty_key_allowlist_remains_an_explicit_deny_all_policy() {
        let policies = BTreeMap::from([("provider-a".to_string(), Vec::new())]);

        let canonical = canonicalize_provider_key_policy_keys(policies, &[])
            .expect("empty allowlist should be valid");

        assert_eq!(
            canonical,
            BTreeMap::from([("provider-a".to_string(), Vec::new())])
        );
    }

    #[test]
    fn allowed_provider_reference_resolves_all_matching_provider_types() {
        let providers = vec![provider("provider-a", "A"), provider("provider-b", "B")];

        let resolved = resolve_allowed_provider_ids(&["custom".to_string()], &providers)
            .expect("provider types remain valid group allowlist references");

        assert_eq!(
            resolved,
            BTreeSet::from(["provider-a".to_string(), "provider-b".to_string()])
        );
    }
}
