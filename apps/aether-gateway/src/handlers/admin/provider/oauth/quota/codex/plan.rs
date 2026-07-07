use super::super::shared::{
    build_provider_quota_execution_plan, execute_provider_quota_plan,
    resolve_provider_quota_execution_timeouts, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_provider_pool::{
    build_codex_pool_quota_request, build_codex_pool_reset_credit_consume_request,
    build_codex_pool_reset_credits_request, ProviderPoolQuotaRequestSpec,
};

fn codex_auth_config(
    transport: &AdminGatewayProviderTransportSnapshot,
) -> Option<serde_json::Value> {
    transport
        .key
        .decrypted_auth_config
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
}

pub(super) fn build_codex_quota_request_spec(
    transport: &AdminGatewayProviderTransportSnapshot,
    resolved_oauth_auth: Option<(String, String)>,
) -> Result<ProviderPoolQuotaRequestSpec, String> {
    let auth_config = codex_auth_config(transport);
    let mut request = build_codex_pool_quota_request(
        &transport.key.id,
        resolved_oauth_auth,
        Some(transport.key.decrypted_api_key.as_str()),
        auth_config.as_ref(),
    )?;
    crate::provider_transport::apply_local_auth_config_header_overrides(
        &mut request.headers,
        transport.key.decrypted_auth_config.as_deref(),
    );
    Ok(request)
}

pub(super) fn build_codex_reset_credits_request_spec(
    transport: &AdminGatewayProviderTransportSnapshot,
    resolved_oauth_auth: Option<(String, String)>,
) -> Result<ProviderPoolQuotaRequestSpec, String> {
    let auth_config = codex_auth_config(transport);
    let mut request = build_codex_pool_reset_credits_request(
        &transport.key.id,
        resolved_oauth_auth,
        Some(transport.key.decrypted_api_key.as_str()),
        auth_config.as_ref(),
    )?;
    crate::provider_transport::apply_local_auth_config_header_overrides(
        &mut request.headers,
        transport.key.decrypted_auth_config.as_deref(),
    );
    Ok(request)
}

pub(super) fn build_codex_reset_credit_consume_request_spec(
    transport: &AdminGatewayProviderTransportSnapshot,
    resolved_oauth_auth: Option<(String, String)>,
    idempotency_key: &str,
) -> Result<ProviderPoolQuotaRequestSpec, String> {
    let auth_config = codex_auth_config(transport);
    let mut request = build_codex_pool_reset_credit_consume_request(
        &transport.key.id,
        resolved_oauth_auth,
        Some(transport.key.decrypted_api_key.as_str()),
        auth_config.as_ref(),
        idempotency_key,
    )?;
    crate::provider_transport::apply_local_auth_config_header_overrides(
        &mut request.headers,
        transport.key.decrypted_auth_config.as_deref(),
    );
    Ok(request)
}

pub(super) async fn execute_codex_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    spec: ProviderPoolQuotaRequestSpec,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let proxy = match proxy_override {
        Some(proxy) => Some(proxy.clone()),
        None => {
            state
                .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
                .await
        }
    };
    let timeouts = Some(resolve_provider_quota_execution_timeouts(
        state.resolve_transport_execution_timeouts(transport),
        proxy.as_ref(),
    ));
    let plan = build_provider_quota_execution_plan(
        transport,
        spec,
        proxy,
        state.resolve_transport_profile(transport),
        timeouts,
    );
    execute_provider_quota_plan(state, transport, plan, "codex").await
}

pub(super) async fn execute_codex_reset_credit_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    spec: ProviderPoolQuotaRequestSpec,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let proxy = match proxy_override {
        Some(proxy) => Some(proxy.clone()),
        None => {
            state
                .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
                .await
        }
    };
    let timeouts = Some(resolve_provider_quota_execution_timeouts(
        state.resolve_transport_execution_timeouts(transport),
        proxy.as_ref(),
    ));
    let plan = build_provider_quota_execution_plan(
        transport,
        spec,
        proxy,
        state.resolve_transport_profile(transport),
        timeouts,
    );
    execute_provider_quota_plan(state, transport, plan, "codex_reset_credit").await
}
