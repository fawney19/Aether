use super::cache_types::AdminMonitoringCacheAffinityRecord;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;

pub(super) async fn admin_monitoring_list_export_api_key_records_by_ids(
    state: &AdminAppState<'_>,
    api_key_ids: &[String],
) -> Result<
    std::collections::BTreeMap<String, aether_data::repository::auth::StoredAuthApiKeyExportRecord>,
    GatewayError,
> {
    if api_key_ids.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }

    Ok(state
        .list_auth_api_key_export_records_by_ids(api_key_ids)
        .await?
        .into_iter()
        .map(|record| (record.api_key_id.clone(), record))
        .collect())
}

async fn admin_monitoring_list_user_summaries_by_ids(
    state: &AdminAppState<'_>,
    user_ids: &[String],
) -> Result<
    std::collections::BTreeMap<String, aether_data::repository::users::StoredUserSummary>,
    GatewayError,
> {
    if user_ids.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }

    Ok(state
        .list_users_by_ids(user_ids)
        .await?
        .into_iter()
        .map(|user| (user.id.clone(), user))
        .collect())
}

pub(super) async fn admin_monitoring_load_affinity_identity_maps(
    state: &AdminAppState<'_>,
    affinities: &[AdminMonitoringCacheAffinityRecord],
) -> Result<
    (
        std::collections::BTreeMap<
            String,
            aether_data::repository::auth::StoredAuthApiKeyExportRecord,
        >,
        std::collections::BTreeMap<String, aether_data::repository::users::StoredUserSummary>,
    ),
    GatewayError,
> {
    let api_key_ids = affinities
        .iter()
        .map(|item| item.affinity_key.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let api_key_by_id =
        admin_monitoring_list_export_api_key_records_by_ids(state, &api_key_ids).await?;
    let user_ids = api_key_by_id
        .values()
        .map(|record| record.user_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let user_by_id = admin_monitoring_list_user_summaries_by_ids(state, &user_ids).await?;
    Ok((api_key_by_id, user_by_id))
}

pub(super) async fn admin_monitoring_find_user_summary_by_id(
    state: &AdminAppState<'_>,
    user_id: &str,
) -> Result<Option<aether_data::repository::users::StoredUserSummary>, GatewayError> {
    if user_id.trim().is_empty() {
        return Ok(None);
    }

    let user_ids = [user_id.to_string()];
    Ok(state.list_users_by_ids(&user_ids).await?.into_iter().next())
}
