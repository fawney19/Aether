use super::super::shared::{
    default_provider_quota_execution_timeouts, execute_provider_quota_plan,
    ProviderQuotaExecutionOutcome,
};
use super::parse::normalize_codex_plan_type;
use crate::handlers::admin::provider::shared::payloads::CODEX_WHAM_USAGE_URL;
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_contracts::{ExecutionPlan, ProxySnapshot, RequestBody};
use std::collections::BTreeMap;

pub(super) fn build_codex_refresh_headers(
    transport: &AdminGatewayProviderTransportSnapshot,
    resolved_oauth_auth: Option<(String, String)>,
) -> Result<BTreeMap<String, String>, String> {
    let mut headers = BTreeMap::new();
    headers.insert("accept".to_string(), "application/json".to_string());

    if let Some((name, value)) = resolved_oauth_auth {
        headers.insert(name.to_ascii_lowercase(), value);
    } else {
        let decrypted_key = transport.key.decrypted_api_key.trim();
        if decrypted_key.is_empty() || decrypted_key == "__placeholder__" {
            return Err("缺少 OAuth 认证信息，请先授权/刷新 Token".to_string());
        }
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {decrypted_key}"),
        );
    }

    let auth_config = transport
        .key
        .decrypted_auth_config
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
    let oauth_plan_type = normalize_codex_plan_type(
        auth_config
            .as_ref()
            .and_then(|value| value.get("plan_type"))
            .and_then(serde_json::Value::as_str),
    );
    let oauth_account_id = auth_config
        .as_ref()
        .and_then(|value| value.get("account_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if oauth_account_id.is_some() && oauth_plan_type.as_deref() != Some("free") {
        headers.insert(
            "chatgpt-account-id".to_string(),
            oauth_account_id.unwrap_or_default().to_string(),
        );
    }

    Ok(headers)
}

pub(super) async fn execute_codex_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    headers: BTreeMap<String, String>,
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
    let timeouts = state
        .resolve_transport_execution_timeouts(transport)
        .or(Some(default_provider_quota_execution_timeouts(
            proxy.as_ref(),
        )));
    let plan = ExecutionPlan {
        request_id: format!("codex-quota:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("codex".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "GET".to_string(),
        url: CODEX_WHAM_USAGE_URL.to_string(),
        headers,
        content_type: None,
        content_encoding: None,
        body: RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        },
        stream: false,
        client_api_format: "openai:responses".to_string(),
        provider_api_format: "openai:responses".to_string(),
        model_name: Some("codex-wham-usage".to_string()),
        proxy,
        tls_profile: state.resolve_transport_tls_profile(transport),
        timeouts,
    };
    execute_provider_quota_plan(state, transport, plan, "codex").await
}
