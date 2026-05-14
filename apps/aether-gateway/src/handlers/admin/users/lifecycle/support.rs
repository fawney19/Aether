use super::super::format_optional_datetime_iso8601;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use serde_json::json;

pub(super) async fn admin_user_password_policy(
    state: &AdminAppState<'_>,
) -> Result<String, GatewayError> {
    let config = state
        .read_system_config_json_value("password_policy_level")
        .await?;
    Ok(
        match config
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or("weak")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "medium" => "medium".to_string(),
            "strong" => "strong".to_string(),
            _ => "weak".to_string(),
        },
    )
}

pub(super) async fn find_admin_export_user(
    state: &AdminAppState<'_>,
    user_id: &str,
) -> Result<Option<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
    state.find_export_user_by_id(user_id).await
}

pub(super) fn build_admin_user_payload(
    user: &aether_data::repository::users::StoredUserAuthRecord,
    rate_limit: Option<i32>,
    unlimited: bool,
) -> serde_json::Value {
    build_admin_user_payload_with_groups(user, rate_limit, None, unlimited, &[])
}

pub(super) fn build_admin_user_payload_with_groups(
    user: &aether_data::repository::users::StoredUserAuthRecord,
    rate_limit: Option<i32>,
    rate_limit_mode: Option<&str>,
    unlimited: bool,
    groups: &[aether_data::repository::users::StoredUserGroup],
) -> serde_json::Value {
    json!({
        "id": user.id,
        "email": user.email,
        "username": user.username,
        "role": user.role,
        "allowed_providers": user.allowed_providers,
        "allowed_providers_mode": user.allowed_providers_mode,
        "allowed_api_formats": user.allowed_api_formats,
        "allowed_api_formats_mode": user.allowed_api_formats_mode,
        "allowed_models": user.allowed_models,
        "allowed_models_mode": user.allowed_models_mode,
        "rate_limit": rate_limit,
        "rate_limit_mode": rate_limit_mode.unwrap_or("system"),
        "unlimited": unlimited,
        "is_active": user.is_active,
        "created_at": format_optional_datetime_iso8601(user.created_at),
        "updated_at": serde_json::Value::Null,
        "last_login_at": format_optional_datetime_iso8601(user.last_login_at),
        "groups": groups.iter().map(user_group_badge_payload).collect::<Vec<_>>(),
        "effective_policy": effective_policy_payload(
            user.allowed_providers.as_ref(),
            &user.allowed_providers_mode,
            user.allowed_api_formats.as_ref(),
            &user.allowed_api_formats_mode,
            user.allowed_models.as_ref(),
            &user.allowed_models_mode,
            rate_limit,
            rate_limit_mode.unwrap_or("system"),
            groups,
        ),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_admin_user_export_payload(
    row: &aether_data::repository::users::StoredUserExportRow,
    unlimited: bool,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    request_count: u64,
    total_tokens: u64,
    groups: &[aether_data::repository::users::StoredUserGroup],
) -> serde_json::Value {
    json!({
        "id": row.id,
        "email": row.email,
        "username": row.username,
        "role": row.role,
        "allowed_providers": row.allowed_providers,
        "allowed_providers_mode": row.allowed_providers_mode,
        "allowed_api_formats": row.allowed_api_formats,
        "allowed_api_formats_mode": row.allowed_api_formats_mode,
        "allowed_models": row.allowed_models,
        "allowed_models_mode": row.allowed_models_mode,
        "rate_limit": row.rate_limit,
        "rate_limit_mode": row.rate_limit_mode,
        "feature_settings": row.feature_settings,
        "unlimited": unlimited,
        "is_active": row.is_active,
        "created_at": format_optional_datetime_iso8601(created_at),
        "updated_at": serde_json::Value::Null,
        "last_login_at": format_optional_datetime_iso8601(last_login_at),
        "request_count": request_count,
        "total_tokens": total_tokens,
        "groups": groups.iter().map(user_group_badge_payload).collect::<Vec<_>>(),
        "effective_policy": effective_policy_payload(
            row.allowed_providers.as_ref(),
            &row.allowed_providers_mode,
            row.allowed_api_formats.as_ref(),
            &row.allowed_api_formats_mode,
            row.allowed_models.as_ref(),
            &row.allowed_models_mode,
            row.rate_limit,
            &row.rate_limit_mode,
            groups,
        ),
    })
}

pub(super) fn user_group_badge_payload(
    group: &aether_data::repository::users::StoredUserGroup,
) -> serde_json::Value {
    json!({
        "id": group.id,
        "name": group.name,
    })
}

#[allow(clippy::too_many_arguments)]
fn effective_policy_payload(
    allowed_providers: Option<&Vec<String>>,
    allowed_providers_mode: &str,
    allowed_api_formats: Option<&Vec<String>>,
    allowed_api_formats_mode: &str,
    allowed_models: Option<&Vec<String>>,
    allowed_models_mode: &str,
    rate_limit: Option<i32>,
    rate_limit_mode: &str,
    groups: &[aether_data::repository::users::StoredUserGroup],
) -> serde_json::Value {
    let mut sorted_groups = groups.to_vec();
    sorted_groups.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    json!({
        "allowed_providers": effective_list_policy_payload(
            allowed_providers,
            allowed_providers_mode,
            &sorted_groups,
            |group| (&group.allowed_providers_mode, group.allowed_providers.as_ref()),
        ),
        "allowed_api_formats": effective_list_policy_payload(
            allowed_api_formats,
            allowed_api_formats_mode,
            &sorted_groups,
            |group| (&group.allowed_api_formats_mode, group.allowed_api_formats.as_ref()),
        ),
        "allowed_models": effective_list_policy_payload(
            allowed_models,
            allowed_models_mode,
            &sorted_groups,
            |group| (&group.allowed_models_mode, group.allowed_models.as_ref()),
        ),
        "rate_limit": effective_rate_limit_policy_payload(rate_limit, rate_limit_mode, &sorted_groups),
    })
}

fn effective_list_policy_payload(
    user_values: Option<&Vec<String>>,
    user_mode: &str,
    groups: &[aether_data::repository::users::StoredUserGroup],
    group_field: impl Fn(
        &aether_data::repository::users::StoredUserGroup,
    ) -> (&String, Option<&Vec<String>>),
) -> serde_json::Value {
    let mut effective = None;
    let mut group_sources = Vec::new();
    for group in groups {
        let (mode, values) = group_field(group);
        if let Some(restriction) = list_restriction_from_mode(mode, values.cloned()) {
            effective = intersect_list_policies(effective, Some(restriction));
            group_sources.push(group);
        }
    }
    let mut has_user_source = false;
    if let Some(restriction) = list_restriction_from_mode(user_mode, user_values.cloned()) {
        effective = intersect_list_policies(effective, Some(restriction));
        has_user_source = true;
    }

    let (mode, value) = match effective {
        Some(values) if values.is_empty() => ("deny_all", json!(Vec::<String>::new())),
        Some(values) => ("specific", json!(values)),
        None => ("unrestricted", serde_json::Value::Null),
    };
    let source = combined_policy_source(has_user_source, group_sources.len(), "fallback");
    policy_payload(mode, value, source, group_sources.as_slice())
}

fn effective_rate_limit_policy_payload(
    user_rate_limit: Option<i32>,
    user_mode: &str,
    groups: &[aether_data::repository::users::StoredUserGroup],
) -> serde_json::Value {
    let mut effective = None;
    let mut group_sources = Vec::new();
    for group in groups {
        if let Some(restriction) =
            rate_limit_restriction_from_mode(&group.rate_limit_mode, group.rate_limit)
        {
            effective = intersect_rate_limit_policies(effective, Some(restriction));
            group_sources.push(group);
        }
    }
    let mut has_user_source = false;
    if let Some(restriction) = rate_limit_restriction_from_mode(user_mode, user_rate_limit) {
        effective = intersect_rate_limit_policies(effective, Some(restriction));
        has_user_source = true;
    }

    let source = combined_policy_source(has_user_source, group_sources.len(), "fallback");
    match rate_limit_policy_value(effective) {
        Some(rate_limit) => policy_payload("custom", json!(rate_limit), source, &group_sources),
        None => policy_payload("system", serde_json::Value::Null, source, &group_sources),
    }
}

fn policy_payload(
    mode: &str,
    value: serde_json::Value,
    source: &str,
    groups: &[&aether_data::repository::users::StoredUserGroup],
) -> serde_json::Value {
    let single_group = groups.first().copied().filter(|_| groups.len() == 1);
    json!({
        "mode": mode,
        "value": value,
        "source": source,
        "group_id": single_group.map(|group| group.id.as_str()),
        "group_name": single_group.map(|group| group.name.as_str()),
        "group_ids": groups.iter().map(|group| group.id.as_str()).collect::<Vec<_>>(),
        "group_names": groups.iter().map(|group| group.name.as_str()).collect::<Vec<_>>(),
    })
}

fn list_restriction_from_mode(mode: &str, values: Option<Vec<String>>) -> Option<Vec<String>> {
    match mode {
        "specific" => Some(values.unwrap_or_default()),
        "deny_all" => Some(Vec::new()),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RateLimitRestriction {
    Unlimited,
    Limited(i32),
}

fn rate_limit_restriction_from_mode(
    mode: &str,
    rate_limit: Option<i32>,
) -> Option<RateLimitRestriction> {
    match mode {
        "custom" => {
            let rate_limit = rate_limit.unwrap_or(0).max(0);
            if rate_limit == 0 {
                Some(RateLimitRestriction::Unlimited)
            } else {
                Some(RateLimitRestriction::Limited(rate_limit))
            }
        }
        _ => None,
    }
}

fn intersect_list_policies(
    left: Option<Vec<String>>,
    right: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (left, right) {
        (None, None) => None,
        (Some(values), None) | (None, Some(values)) => Some(values),
        (Some(left_values), Some(right_values)) => {
            let right_values = right_values
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            Some(
                left_values
                    .into_iter()
                    .filter(|value| right_values.contains(value))
                    .collect(),
            )
        }
    }
}

fn intersect_rate_limit_policies(
    left: Option<RateLimitRestriction>,
    right: Option<RateLimitRestriction>,
) -> Option<RateLimitRestriction> {
    match (left, right) {
        (None, None) => None,
        (Some(value), None) | (None, Some(value)) => Some(value),
        (Some(RateLimitRestriction::Unlimited), Some(RateLimitRestriction::Unlimited)) => {
            Some(RateLimitRestriction::Unlimited)
        }
        (Some(RateLimitRestriction::Limited(value)), Some(RateLimitRestriction::Unlimited))
        | (Some(RateLimitRestriction::Unlimited), Some(RateLimitRestriction::Limited(value))) => {
            Some(RateLimitRestriction::Limited(value))
        }
        (Some(RateLimitRestriction::Limited(left)), Some(RateLimitRestriction::Limited(right))) => {
            Some(RateLimitRestriction::Limited(left.min(right)))
        }
    }
}

fn rate_limit_policy_value(policy: Option<RateLimitRestriction>) -> Option<i32> {
    match policy {
        None => None,
        Some(RateLimitRestriction::Unlimited) => Some(0),
        Some(RateLimitRestriction::Limited(value)) => Some(value),
    }
}

fn combined_policy_source(
    has_user_source: bool,
    group_source_count: usize,
    fallback_source: &'static str,
) -> &'static str {
    match (has_user_source, group_source_count) {
        (true, 0) => "user",
        (false, 1) => "group",
        (false, 0) => fallback_source,
        _ => "combined",
    }
}

pub(super) fn admin_user_id_from_detail_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/users/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}
