use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::handlers::shared::{
    sync_provider_key_oauth_status_snapshot, sync_provider_key_quota_status_snapshot,
};
use crate::GatewayError;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTimeouts, ProxySnapshot, RequestBody,
    ResolvedTransportProfile, EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_provider_pool::{ProviderPoolQuotaRequestSpec, ProviderPoolService};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

const PROVIDER_QUOTA_DEFAULT_TIMEOUT_MS: u64 = 30_000;
const PROVIDER_QUOTA_PROXY_TIMEOUT_MS: u64 = 60_000;

pub(super) enum ProviderQuotaExecutionOutcome {
    Response(ExecutionResult),
    Failure(String),
}

pub(super) fn default_provider_quota_execution_timeouts(
    proxy: Option<&ProxySnapshot>,
) -> ExecutionTimeouts {
    let timeout_ms = if proxy.is_some() {
        PROVIDER_QUOTA_PROXY_TIMEOUT_MS
    } else {
        PROVIDER_QUOTA_DEFAULT_TIMEOUT_MS
    };
    ExecutionTimeouts {
        connect_ms: Some(timeout_ms),
        read_ms: Some(timeout_ms),
        write_ms: Some(timeout_ms),
        pool_ms: Some(timeout_ms),
        total_ms: Some(timeout_ms),
        ..ExecutionTimeouts::default()
    }
}

pub(super) fn provider_auto_remove_banned_keys(config: Option<&serde_json::Value>) -> bool {
    admin_provider_quota_pure::provider_auto_remove_banned_keys(config)
}

pub(super) fn should_auto_remove_structured_reason(reason: Option<&str>) -> bool {
    admin_provider_quota_pure::should_auto_remove_structured_reason(reason)
}

pub(crate) fn normalize_string_id_list(values: Option<Vec<String>>) -> Option<Vec<String>> {
    admin_provider_quota_pure::normalize_string_id_list(values)
}

pub(crate) fn provider_type_supports_quota_refresh(provider_type: &str) -> bool {
    ProviderPoolService::with_builtin_adapters().supports_quota_refresh(provider_type)
}

pub(crate) fn unsupported_provider_quota_refresh_message(provider_type: &str) -> String {
    ProviderPoolService::with_builtin_adapters().quota_refresh_unsupported_message(provider_type)
}

pub(crate) fn provider_quota_refresh_endpoint_for_provider(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
    include_inactive: bool,
) -> Option<StoredProviderCatalogEndpoint> {
    ProviderPoolService::with_builtin_adapters().quota_refresh_endpoint_for_provider(
        provider_type,
        endpoints,
        include_inactive,
    )
}

pub(crate) fn provider_quota_refresh_missing_endpoint_message(provider_type: &str) -> String {
    ProviderPoolService::with_builtin_adapters()
        .quota_refresh_missing_endpoint_message(provider_type)
}

pub(super) fn coerce_json_u64(value: &serde_json::Value) -> Option<u64> {
    admin_provider_quota_pure::coerce_json_u64(value)
}

pub(super) fn coerce_json_f64(value: &serde_json::Value) -> Option<f64> {
    admin_provider_quota_pure::coerce_json_f64(value)
}

pub(super) fn coerce_json_bool(value: &serde_json::Value) -> Option<bool> {
    admin_provider_quota_pure::coerce_json_bool(value)
}

fn merge_upstream_metadata(
    current: Option<&serde_json::Value>,
    updates: &serde_json::Value,
) -> serde_json::Value {
    let mut merged = current
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(update_object) = updates.as_object() {
        for (key, value) in update_object {
            merged.insert(key.clone(), value.clone());
        }
    }
    serde_json::Value::Object(merged)
}

pub(super) fn extract_execution_error_message(result: &ExecutionResult) -> Option<String> {
    admin_provider_quota_pure::extract_execution_error_message(result)
}

pub(super) fn quota_refresh_success_invalid_state(
    key: &StoredProviderCatalogKey,
) -> (Option<u64>, Option<String>) {
    admin_provider_quota_pure::quota_refresh_success_invalid_state(key)
}

pub(super) fn coerce_json_string(value: Option<&serde_json::Value>) -> Option<String> {
    admin_provider_quota_pure::coerce_json_string(value)
}

pub(super) fn build_quota_snapshot_payload(
    provider_type: &str,
    current_status_snapshot: Option<&serde_json::Value>,
    metadata_update: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let updated_snapshot = sync_provider_key_quota_status_snapshot(
        current_status_snapshot,
        provider_type,
        metadata_update,
        "refresh_api",
    )?;
    updated_snapshot.get("quota").cloned()
}

pub(super) fn build_provider_quota_execution_plan(
    transport: &AdminGatewayProviderTransportSnapshot,
    spec: ProviderPoolQuotaRequestSpec,
    proxy: Option<ProxySnapshot>,
    transport_profile: Option<ResolvedTransportProfile>,
    timeouts: Option<ExecutionTimeouts>,
) -> ExecutionPlan {
    let ProviderPoolQuotaRequestSpec {
        request_id,
        provider_name,
        quota_kind: _,
        method,
        url,
        mut headers,
        content_type,
        json_body,
        client_api_format,
        provider_api_format,
        model_name,
        accept_invalid_certs,
    } = spec;
    if accept_invalid_certs {
        headers.insert(
            EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER.to_string(),
            "true".to_string(),
        );
    }
    let body = json_body
        .map(RequestBody::from_json)
        .unwrap_or(RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        });
    ExecutionPlan {
        request_id,
        candidate_id: None,
        provider_name: Some(provider_name),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method,
        url,
        headers,
        content_type,
        content_encoding: None,
        body,
        stream: false,
        client_api_format,
        provider_api_format,
        model_name,
        proxy,
        transport_profile,
        timeouts,
    }
}

pub(crate) async fn persist_provider_quota_refresh_state(
    state: &AdminAppState<'_>,
    key_id: &str,
    metadata_update: Option<&serde_json::Value>,
    oauth_invalid_at_unix_secs: Option<u64>,
    oauth_invalid_reason: Option<String>,
    encrypted_auth_config: Option<String>,
) -> Result<bool, GatewayError> {
    let Some(mut latest_key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };

    let mut quota_snapshot_provider_type = None::<String>;
    if let Some(metadata_update) = metadata_update {
        latest_key.upstream_metadata = Some(merge_upstream_metadata(
            latest_key.upstream_metadata.as_ref(),
            metadata_update,
        ));
        quota_snapshot_provider_type =
            aether_provider_pool::provider_pool_quota_metadata_provider_type(metadata_update);
    }
    if let Some(encrypted_auth_config) = encrypted_auth_config {
        latest_key.encrypted_auth_config = Some(encrypted_auth_config);
    }
    latest_key.oauth_invalid_at_unix_secs = oauth_invalid_at_unix_secs;
    latest_key.oauth_invalid_reason = oauth_invalid_reason;
    if let Some(provider_type) = quota_snapshot_provider_type.as_deref() {
        latest_key.status_snapshot = sync_provider_key_quota_status_snapshot(
            latest_key.status_snapshot.as_ref(),
            provider_type,
            latest_key.upstream_metadata.as_ref(),
            "refresh_api",
        );
    }
    latest_key.status_snapshot =
        sync_provider_key_oauth_status_snapshot(latest_key.status_snapshot.as_ref(), &latest_key);
    latest_key.updated_at_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    Ok(state
        .update_provider_catalog_key(&latest_key)
        .await?
        .is_some())
}

pub(super) async fn execute_provider_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    plan: ExecutionPlan,
    quota_kind: &str,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    match state.execute_execution_runtime_sync_plan(None, &plan).await {
        Ok(result) => Ok(ProviderQuotaExecutionOutcome::Response(result)),
        Err(err) => {
            let error = match err {
                GatewayError::UpstreamUnavailable { message, .. }
                | GatewayError::ControlUnavailable { message, .. }
                | GatewayError::Client { message, .. }
                | GatewayError::Internal(message) => message,
            };
            let proxy_node_id = plan
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.node_id.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let proxy_source = state
                .resolve_transport_proxy_source_with_tunnel_affinity(transport)
                .await;
            let proxy_url_present = plan
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.url.as_deref())
                .map(str::trim)
                .is_some_and(|value| !value.is_empty());
            warn!(
                key_id = %transport.key.id,
                endpoint_id = %transport.endpoint.id,
                url = %plan.url,
                proxy_source = ?proxy_source,
                proxy_node_id = ?proxy_node_id,
                proxy_url_present,
                error = %error,
                quota_kind = %quota_kind,
                "gateway provider quota execution runtime request failed"
            );
            Ok(ProviderQuotaExecutionOutcome::Failure(error))
        }
    }
}
