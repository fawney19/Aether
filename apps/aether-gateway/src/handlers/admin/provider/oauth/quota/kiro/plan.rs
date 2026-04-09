use super::super::shared::{execute_provider_quota_plan, ProviderQuotaExecutionOutcome};
use crate::handlers::admin::provider::shared::payloads::{
    KIRO_USAGE_LIMITS_PATH, KIRO_USAGE_SDK_VERSION,
};
use crate::handlers::admin::request::{
    AdminAppState, AdminGatewayProviderTransportSnapshot, AdminKiroRequestAuth,
};
use crate::GatewayError;
use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use std::collections::BTreeMap;
use url::form_urlencoded;
use uuid::Uuid;

fn build_kiro_usage_headers(auth: &AdminKiroRequestAuth) -> BTreeMap<String, String> {
    let kiro_version = auth.auth_config.effective_kiro_version();
    let machine_id = auth.machine_id.trim();
    let ide_tag = if machine_id.is_empty() {
        format!("KiroIDE-{kiro_version}")
    } else {
        format!("KiroIDE-{kiro_version}-{machine_id}")
    };
    let host = format!(
        "q.{}.amazonaws.com",
        auth.auth_config.effective_api_region()
    );

    BTreeMap::from([
        (
            "x-amz-user-agent".to_string(),
            format!("aws-sdk-js/{KIRO_USAGE_SDK_VERSION} {ide_tag}"),
        ),
        (
            "user-agent".to_string(),
            format!(
                "aws-sdk-js/{KIRO_USAGE_SDK_VERSION} ua/2.1 os/other#unknown lang/js md/nodejs#22.21.1 api/codewhispererruntime#1.0.0 m/N,E {ide_tag}"
            ),
        ),
        ("host".to_string(), host),
        ("amz-sdk-invocation-id".to_string(), Uuid::new_v4().to_string()),
        ("amz-sdk-request".to_string(), "attempt=1; max=1".to_string()),
        ("authorization".to_string(), auth.value.clone()),
        ("connection".to_string(), "close".to_string()),
    ])
}

fn build_kiro_usage_url(auth: &AdminKiroRequestAuth) -> String {
    let host = format!(
        "q.{}.amazonaws.com",
        auth.auth_config.effective_api_region()
    );
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("origin", "AI_EDITOR");
    serializer.append_pair("resourceType", "AGENTIC_REQUEST");
    serializer.append_pair("isEmailRequired", "true");
    if let Some(profile_arn) = auth.auth_config.profile_arn_for_payload() {
        serializer.append_pair("profileArn", profile_arn);
    }
    format!(
        "https://{host}{KIRO_USAGE_LIMITS_PATH}?{}",
        serializer.finish()
    )
}

pub(super) async fn execute_kiro_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    auth: &AdminKiroRequestAuth,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let plan = ExecutionPlan {
        request_id: format!("kiro-quota:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("kiro".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "GET".to_string(),
        url: build_kiro_usage_url(auth),
        headers: build_kiro_usage_headers(auth),
        content_type: None,
        content_encoding: None,
        body: RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        },
        stream: false,
        client_api_format: "claude:cli".to_string(),
        provider_api_format: "kiro:usage".to_string(),
        model_name: Some("kiro-usage-limits".to_string()),
        proxy: state
            .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
            .await,
        tls_profile: state.resolve_transport_tls_profile(transport),
        timeouts: state
            .resolve_transport_execution_timeouts(transport)
            .or(Some(ExecutionTimeouts {
                connect_ms: Some(30_000),
                read_ms: Some(30_000),
                write_ms: Some(30_000),
                pool_ms: Some(30_000),
                total_ms: Some(30_000),
                ..ExecutionTimeouts::default()
            })),
    };

    execute_provider_quota_plan(state, transport, plan, "kiro").await
}
