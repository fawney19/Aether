use super::shared::{
    build_provider_quota_execution_plan, build_quota_snapshot_payload, coerce_json_f64,
    coerce_json_string, execute_provider_quota_plan, extract_execution_error_message,
    oauth_refresh_auto_removed_result, persist_provider_quota_refresh_state,
    quota_key_auto_removed, quota_refresh_success_invalid_state,
    resolve_provider_quota_execution_timeouts, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::quota::{
    parse_antigravity_quota_summary_response, parse_antigravity_usage_response,
};
use aether_admin::provider::redaction::admin_provider_metadata_bucket_safe_json;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_pool::{
    build_antigravity_pool_quota_request, build_antigravity_pool_quota_summary_request,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

async fn execute_antigravity_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    authorization: (String, String),
    project_id: &str,
    identity_headers: BTreeMap<String, String>,
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
    let spec = build_antigravity_pool_quota_request(
        &transport.key.id,
        &transport.endpoint.base_url,
        authorization,
        project_id,
        identity_headers,
    );
    let plan = build_provider_quota_execution_plan(
        transport,
        spec,
        proxy,
        state.resolve_transport_profile(transport),
        timeouts,
    );

    execute_provider_quota_plan(state, transport, plan, "antigravity").await
}

async fn fetch_antigravity_quota_summary_best_effort(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    authorization: (String, String),
    project_id: &str,
    identity_headers: BTreeMap<String, String>,
    proxy_override: Option<&ProxySnapshot>,
) -> Option<serde_json::Value> {
    let mut request_project_id = Some(project_id);

    loop {
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
        let spec = build_antigravity_pool_quota_summary_request(
            &transport.key.id,
            &transport.endpoint.base_url,
            authorization.clone(),
            request_project_id,
            identity_headers.clone(),
        );
        let plan = build_provider_quota_execution_plan(
            transport,
            spec,
            proxy,
            state.resolve_transport_profile(transport),
            timeouts,
        );
        let outcome = match execute_provider_quota_plan(state, transport, plan, "antigravity").await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                warn!(error = ?error, "Antigravity grouped quota request failed");
                return None;
            }
        };
        let result = match outcome {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                warn!(detail = %detail, "Antigravity grouped quota execution failed");
                return None;
            }
        };

        if result.status_code == 200 {
            return result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(parse_antigravity_quota_summary_response);
        }
        if result.status_code == 403 && request_project_id.is_some() {
            request_project_id = None;
            continue;
        }

        warn!(
            status_code = result.status_code,
            "Antigravity grouped quota request returned a non-success status"
        );
        return None;
    }
}

pub(crate) async fn refresh_antigravity_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut failed_count = 0usize;
    let mut auto_removed_count = 0usize;

    for key in keys {
        let mut transport = match state
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
                if quota_key_auto_removed(state, &key.id).await? {
                    auto_removed_count += 1;
                    results.push(oauth_refresh_auto_removed_result(&key));
                    continue;
                }
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

        let identity = match state.resolve_local_antigravity_identity_headers(&transport) {
            Some(identity) => Some(identity),
            None => state
                .app()
                .hydrate_antigravity_project_metadata_for_transport(&transport)
                .await
                .and_then(|hydrated| {
                    let identity = state.resolve_local_antigravity_identity_headers(&hydrated);
                    transport = hydrated;
                    identity
                }),
        };
        let Some((project_id, identity_headers)) = identity else {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "缺少 Antigravity project_id，loadCodeAssist 未返回可用项目信息",
            }));
            continue;
        };

        let result = match execute_antigravity_quota_plan(
            state,
            &transport,
            authorization.clone(),
            &project_id,
            identity_headers.clone(),
            proxy_override.as_ref(),
        )
        .await?
        {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(_) => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": "fetchAvailableModels 请求执行失败",
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
                if let Some(mut metadata) =
                    parse_antigravity_usage_response(body_json, now_unix_secs)
                {
                    if let Some(metadata) = metadata.as_object_mut() {
                        metadata.insert("project_id".to_string(), json!(project_id));
                    }
                    if let Some(quota_groups) = fetch_antigravity_quota_summary_best_effort(
                        state,
                        &transport,
                        authorization,
                        &project_id,
                        identity_headers,
                        proxy_override.as_ref(),
                    )
                    .await
                    {
                        if let Some(metadata) = metadata.as_object_mut() {
                            metadata.insert("quota_groups".to_string(), quota_groups);
                            metadata.insert(
                                "quota_groups_updated_at".to_string(),
                                json!(now_unix_secs),
                            );
                        }
                    }
                    metadata_update = Some(json!({ "antigravity": metadata }));
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
            message = Some(format!(
                "fetchAvailableModels 返回状态码 {}",
                result.status_code
            ));
            if result.status_code == 403 {
                let reason = "账户访问被禁止".to_string();
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
        {
            payload.insert(
                "metadata".to_string(),
                admin_provider_metadata_bucket_safe_json("antigravity", Some(metadata)),
            );
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "antigravity",
            key.status_snapshot.as_ref(),
            metadata_update.as_ref(),
        ) {
            payload.insert("quota_snapshot".to_string(), quota_snapshot);
        }
        results.push(serde_json::Value::Object(payload));
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": results.len(),
        "results": results,
        "message": format!("已处理 {} 个 Key", results.len()),
        "auto_removed": auto_removed_count,
    })))
}
