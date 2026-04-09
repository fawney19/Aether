use crate::handlers::admin::request::AdminAppState;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn build_admin_provider_keys_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
    skip: usize,
    limit: usize,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }
    let provider = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await
        .ok()
        .and_then(|mut providers| providers.drain(..).next())?;
    let mut keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .ok()
        .unwrap_or_default();
    keys.sort_by(|left, right| {
        left.internal_priority
            .cmp(&right.internal_priority)
            .then_with(|| {
                left.created_at_unix_secs
                    .unwrap_or_default()
                    .cmp(&right.created_at_unix_secs.unwrap_or_default())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Some(serde_json::Value::Array(
        keys.into_iter()
            .skip(skip)
            .take(limit)
            .map(|key| {
                state.build_admin_provider_key_response(&key, &provider.provider_type, now_unix_secs)
            })
            .collect(),
    ))
}
