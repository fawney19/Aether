use crate::api::ai::{admin_default_body_rules_for_signature, admin_endpoint_signature_parts};
use crate::handlers::public::normalize_admin_base_url;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) fn build_admin_fixed_provider_endpoint_record(
    provider: &StoredProviderCatalogProvider,
    api_format: &str,
    base_url: &str,
) -> Result<StoredProviderCatalogEndpoint, String> {
    let (normalized_api_format, api_family, endpoint_kind) =
        admin_endpoint_signature_parts(api_format)
            .ok_or_else(|| format!("无效的 api_format: {api_format}"))?;
    let body_rules = admin_default_body_rules_for_signature(
        normalized_api_format,
        Some(provider.provider_type.as_str()),
    )
    .and_then(|(_, rules)| (!rules.is_empty()).then_some(serde_json::Value::Array(rules)));
    let endpoint_config = if provider.provider_type == "codex"
        && matches!(normalized_api_format, "openai:cli" | "openai:image")
    {
        Some(json!({ "upstream_stream_policy": "force_stream" }))
    } else {
        None
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    StoredProviderCatalogEndpoint::new(
        Uuid::new_v4().to_string(),
        provider.id.clone(),
        normalized_api_format.to_string(),
        Some(api_family.to_string()),
        Some(endpoint_kind.to_string()),
        true,
    )
    .map_err(|err| err.to_string())?
    .with_timestamps(Some(now_unix_secs), Some(now_unix_secs))
    .with_transport_fields(
        normalize_admin_base_url(base_url)?,
        None,
        body_rules,
        Some(provider.max_retries.unwrap_or(2)),
        None,
        endpoint_config,
        None,
        None,
    )
    .map_err(|err| err.to_string())
}
