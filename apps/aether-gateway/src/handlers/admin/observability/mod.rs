mod monitoring;
mod routes;
mod stats;
mod usage;

pub(super) use self::monitoring::maybe_build_local_admin_monitoring_response;
pub(super) use self::routes::maybe_build_local_admin_observability_response;
pub(crate) use self::stats::{
    admin_stats_bad_request_response, maybe_build_local_admin_stats_response, parse_bounded_u32,
    round_to,
};
pub(crate) use self::stats::{AdminStatsTimeRange, AdminStatsUsageFilter};
pub(crate) use self::usage::maybe_build_local_admin_usage_response;

pub(crate) async fn resolve_usage_user_group_scope(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
    query: Option<&str>,
    include_inactive: bool,
    exclude_admin: bool,
) -> Result<Result<Option<Vec<String>>, String>, crate::GatewayError> {
    let group_id = crate::handlers::admin::shared::query_param_value(query, "user_group_id");
    let Some(group_id) = group_id else {
        return Ok(Ok(None));
    };
    if crate::handlers::admin::shared::query_param_value(query, "user_id").is_some() {
        return Ok(Err(
            "user_id and user_group_id cannot be used together".to_string()
        ));
    }
    if !state.has_user_data_reader() {
        return Ok(Err("user group data is unavailable".to_string()));
    }

    if group_id == UNGROUPED_USAGE_ID {
        let ids = ungrouped_usage_users(state)
            .await?
            .into_iter()
            .filter(|user| include_inactive || user.is_active)
            .filter(|user| !exclude_admin || !user.role.eq_ignore_ascii_case("admin"))
            .map(|user| user.id)
            .collect();
        return Ok(Ok(Some(ids)));
    }
    match state
        .resolve_usage_user_group_member_ids(&group_id, include_inactive, exclude_admin)
        .await?
    {
        Some(user_ids) => Ok(Ok(Some(user_ids))),
        None => Ok(Err("user_group_id does not exist".to_string())),
    }
}

/// Reserved statistics-only scope; never a permission group.
pub(crate) const UNGROUPED_USAGE_ID: &str = "__ungrouped__";

pub(crate) async fn ungrouped_usage_users(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
) -> Result<Vec<aether_data::repository::users::StoredUserSummary>, crate::GatewayError> {
    use aether_data::repository::users::UserExportListQuery;
    let mut users = Vec::new();
    let mut skip = 0;
    loop {
        let page = state
            .list_export_users_page(&UserExportListQuery {
                skip,
                limit: 500,
                ..Default::default()
            })
            .await?;
        let count = page.len();
        if count == 0 {
            break;
        }
        let ids = page.into_iter().map(|user| user.id).collect::<Vec<_>>();
        let grouped = state
            .list_user_group_memberships_by_user_ids(&ids)
            .await?
            .into_iter()
            .map(|membership| membership.user_id)
            .collect::<std::collections::BTreeSet<_>>();
        let ids = ids
            .into_iter()
            .filter(|id| !grouped.contains(id))
            .collect::<Vec<_>>();
        users.extend(
            state
                .list_users_by_ids(&ids)
                .await?
                .into_iter()
                .filter(|user| !user.is_deleted),
        );
        skip += count;
        if count < 500 {
            break;
        }
    }
    Ok(users)
}

/// Current group provider policy, resolved to the provider-name dimension used by usage rollups.
/// None is unrestricted; Some(empty) deliberately matches no usage.
pub(crate) async fn usage_group_provider_names(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
    group: &aether_data::repository::users::StoredUserGroup,
) -> Result<Option<Vec<String>>, crate::GatewayError> {
    if matches!(
        group.allowed_providers_mode.as_str(),
        "unrestricted" | "inherit"
    ) {
        return Ok(None);
    }
    if group.allowed_providers_mode != "specific" {
        return Ok(Some(Vec::new()));
    }
    let allowed = group.allowed_providers.as_deref().unwrap_or_default();
    let providers = state.list_provider_catalog_providers(false).await?;
    let mut names = providers
        .into_iter()
        .filter(|provider| {
            allowed.iter().any(|value| {
                let value = value.trim();
                value.eq_ignore_ascii_case(&provider.id)
                    || value.eq_ignore_ascii_case(&provider.name)
                    || value.eq_ignore_ascii_case(&provider.provider_type)
            })
        })
        .map(|provider| provider.name)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    Ok(Some(names))
}

pub(crate) async fn resolve_usage_group_provider_names(
    state: &crate::handlers::admin::request::AdminAppState<'_>,
    query: Option<&str>,
) -> Result<Option<Vec<String>>, crate::GatewayError> {
    let Some(id) = crate::handlers::admin::shared::query_param_value(query, "user_group_id") else {
        return Ok(None);
    };
    if id == UNGROUPED_USAGE_ID {
        return Ok(None);
    }
    let Some(group) = state.find_user_group_by_id(&id).await? else {
        return Ok(Some(Vec::new()));
    };
    usage_group_provider_names(state, &group).await
}
