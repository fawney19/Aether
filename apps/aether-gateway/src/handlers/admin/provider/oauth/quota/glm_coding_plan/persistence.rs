use super::super::shared::{
    build_quota_snapshot_payload, persist_provider_quota_refresh_state,
    quota_refresh_success_invalid_state,
};
use super::RefreshOutcome;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;

pub(super) async fn persist_empty_probe_result(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    outcome: RefreshOutcome,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
        resolved_oauth_invalid_state(key, &outcome);
    let has_oauth_invalid = oauth_invalid_at_unix_secs.is_some() || oauth_invalid_reason.is_some();
    if has_oauth_invalid
        && !persist_provider_quota_refresh_state(
            state,
            &key.id,
            None,
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason.clone(),
            None,
        )
        .await?
    {
        return Ok(None);
    }
    let status = if has_oauth_invalid {
        "auth_invalid"
    } else {
        "no_metadata"
    };
    Ok(Some(json!({
        "key_id": key.id,
        "key_name": key.name,
        "status": status,
        "message": oauth_invalid_reason
            .unwrap_or_else(|| "响应中未包含可解析的额度信息".to_string()),
        "metadata": outcome.metadata,
    })))
}

pub(super) async fn persist_success_result(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    outcome: RefreshOutcome,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
        resolved_oauth_invalid_state(key, &outcome);
    let metadata_update = json!({ "glm_coding_plan": outcome.metadata });
    let quota_snapshot = build_quota_snapshot_payload(
        "glm_coding_plan",
        key.status_snapshot.as_ref(),
        Some(&metadata_update),
    );
    let Some(quota_snapshot) = quota_snapshot else {
        return Ok(Some(json!({
            "key_id": key.id,
            "key_name": key.name,
            "status": "no_metadata",
            "message": "响应中未包含可解析的额度信息",
            "metadata": metadata_update["glm_coding_plan"],
        })));
    };
    let status = if quota_snapshot
        .get("exhausted")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        "quota_exhausted"
    } else {
        "success"
    };
    if !persist_provider_quota_refresh_state(
        state,
        &key.id,
        Some(&metadata_update),
        oauth_invalid_at_unix_secs,
        oauth_invalid_reason,
        None,
    )
    .await?
    {
        return Ok(None);
    }
    Ok(Some(json!({
        "key_id": key.id,
        "key_name": key.name,
        "status": status,
        "metadata": metadata_update["glm_coding_plan"],
        "quota_snapshot": quota_snapshot,
    })))
}

fn resolved_oauth_invalid_state(
    key: &StoredProviderCatalogKey,
    outcome: &RefreshOutcome,
) -> (Option<u64>, Option<String>) {
    let preserved = quota_refresh_success_invalid_state(key);
    if preserved.1.is_some() || outcome.parsed_probe_count > 0 {
        return preserved;
    }
    (
        outcome.oauth_invalid_at_unix_secs,
        outcome.oauth_invalid_reason.clone(),
    )
}

#[cfg(test)]
mod tests;
