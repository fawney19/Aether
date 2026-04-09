use aether_admin::provider::verify as admin_provider_verify_pure;
use reqwest::header::HeaderMap;
use serde_json::Value;

pub(super) fn admin_provider_ops_verify_headers(
    architecture_id: &str,
    config: &serde_json::Map<String, Value>,
    credentials: &serde_json::Map<String, Value>,
) -> Result<HeaderMap, String> {
    admin_provider_verify_pure::admin_provider_ops_verify_headers(
        architecture_id,
        config,
        credentials,
    )
}
