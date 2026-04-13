use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use std::collections::{BTreeMap, BTreeSet};

pub(in super::super) async fn admin_usage_provider_key_names(
    state: &AdminAppState<'_>,
    usage: &[StoredRequestUsageAudit],
) -> Result<BTreeMap<String, String>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(BTreeMap::new());
    }

    let key_ids = usage
        .iter()
        .filter_map(|item| item.provider_api_key_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if key_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    Ok(state
        .list_provider_catalog_keys_by_ids(&key_ids)
        .await?
        .into_iter()
        .map(|key| (key.id, key.name))
        .collect())
}

pub(in super::super) async fn admin_usage_api_key_names(
    state: &AdminAppState<'_>,
    usage: &[StoredRequestUsageAudit],
) -> Result<BTreeMap<String, String>, GatewayError> {
    if !state.has_auth_api_key_data_reader() {
        return Ok(BTreeMap::new());
    }

    let api_key_ids = usage
        .iter()
        .filter_map(|item| item.api_key_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if api_key_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    state.resolve_auth_api_key_names_by_ids(&api_key_ids).await
}
