use super::shared::{
    coerce_json_f64, coerce_json_string, execute_provider_quota_plan,
    extract_execution_error_message, persist_provider_quota_refresh_state,
    quota_refresh_success_invalid_state, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::provider::shared::payloads::ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH;
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::quota::parse_antigravity_usage_response;
use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

async fn execute_antigravity_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    authorization: (String, String),
    project_id: &str,
    mut identity_headers: BTreeMap<String, String>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let mut headers = std::mem::take(&mut identity_headers);
    headers.insert("authorization".to_string(), authorization.1);
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());
    headers
        .entry("user-agent".to_string())
        .or_insert_with(|| "antigravity".to_string());

    let body = json!({ "project": project_id });
    let plan = ExecutionPlan {
        request_id: format!("antigravity-quota:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("antigravity".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "POST".to_string(),
        url: format!(
            "{}{}",
            transport.endpoint.base_url.trim_end_matches('/'),
            ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH
        ),
        headers,
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody {
            json_body: Some(body),
            body_bytes_b64: None,
            body_ref: None,
        },
        stream: false,
        client_api_format: "gemini:chat".to_string(),
        provider_api_format: "antigravity:fetch_available_models".to_string(),
        model_name: Some("fetchAvailableModels".to_string()),
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

    execute_provider_quota_plan(state, transport, plan, "antigravity").await
}

pub(crate) async fn refresh_antigravity_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut failed_count = 0usize;

    for key in keys {
        let transport = match state
            .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
            .await?
        {
            Some(transport) => transport,
            None => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": "Provider transport snapshot unavailable",
                }));
                continue;
            }
        };

        let authorization = match state.resolve_local_oauth_header_auth(&transport).await? {
            Some(auth) => auth,
            _ => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": "缺少 OAuth 认证信息，请先授权/刷新 Token",
                }));
                continue;
            }
        };

        let Some((project_id, identity_headers)) =
            state.resolve_local_antigravity_identity_headers(&transport)
        else {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "缺少 OAuth 认证信息，请先授权/刷新 Token",
            }));
            continue;
        };

        let result = match execute_antigravity_quota_plan(
            state,
            &transport,
            authorization,
            &project_id,
            identity_headers,
        )
        .await?
        {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": format!("fetchAvailableModels 请求执行失败: {detail}"),
                    "status_code": 502,
                }));
                continue;
            }
        };

        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let mut metadata_update = None::<serde_json::Value>;
        let (mut oauth_invalid_at_unix_secs, mut oauth_invalid_reason) =
            quota_refresh_success_invalid_state(&key);
        let mut status = "error".to_string();
        let mut message = None::<String>;

        if result.status_code == 200 {
            if let Some(body_json) = result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
            {
                metadata_update = parse_antigravity_usage_response(body_json, now_unix_secs)
                    .map(|metadata| json!({ "antigravity": metadata }));
                if metadata_update.is_some() {
                    status = "success".to_string();
                } else {
                    status = "no_metadata".to_string();
                    message = Some("响应中未包含配额信息".to_string());
                }
            } else {
                status = "no_metadata".to_string();
                message = Some("响应中未包含配额信息".to_string());
            }
        } else {
            let err_msg = extract_execution_error_message(&result);
            message = Some(match err_msg.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!(
                        "fetchAvailableModels 返回状态码 {}: {}",
                        result.status_code, detail
                    )
                }
                _ => format!("fetchAvailableModels 返回状态码 {}", result.status_code),
            });
            if result.status_code == 403 {
                let reason = err_msg
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "账户访问被禁止".to_string());
                oauth_invalid_at_unix_secs = Some(now_unix_secs);
                oauth_invalid_reason = Some(format!("账户访问被禁止: {reason}"));
                metadata_update = Some(json!({
                    "antigravity": {
                        "is_forbidden": true,
                        "forbidden_reason": reason,
                        "forbidden_at": now_unix_secs,
                        "updated_at": now_unix_secs,
                    }
                }));
                status = "forbidden".to_string();
            }
        }

        if !persist_provider_quota_refresh_state(
            state,
            &key.id,
            metadata_update.as_ref(),
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason,
            None,
        )
        .await?
        {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "Key 状态写入失败",
            }));
            continue;
        }

        if status == "success" {
            success_count += 1;
        } else {
            failed_count += 1;
        }

        let mut payload = serde_json::Map::new();
        payload.insert("key_id".to_string(), json!(key.id));
        payload.insert("key_name".to_string(), json!(key.name));
        payload.insert("status".to_string(), json!(status));
        if let Some(message) = message {
            payload.insert("message".to_string(), json!(message));
        }
        if let Some(metadata) = metadata_update
            .as_ref()
            .and_then(|value| value.get("antigravity"))
            .cloned()
        {
            payload.insert("metadata".to_string(), metadata);
        }
        results.push(serde_json::Value::Object(payload));
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": success_count + failed_count,
        "results": results,
        "message": format!("已处理 {} 个 Key", success_count + failed_count),
        "auto_removed": 0,
    })))
}
