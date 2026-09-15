use super::shared::{
    build_provider_quota_execution_plan, build_quota_snapshot_payload,
    default_provider_quota_execution_timeouts, execute_provider_quota_plan,
    extract_execution_error_message, oauth_refresh_auto_removed_result,
    persist_provider_quota_refresh_state, quota_key_auto_removed,
    quota_refresh_success_invalid_state, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::quota::parse_xai_billing_response;
use aether_admin::provider::redaction::admin_provider_metadata_bucket_safe_json;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_pool::{build_xai_pool_billing_request, build_xai_pool_user_request};
use aether_provider_transport::xai::{
    extract_xai_user_id_from_auth_config, extract_xai_user_id_from_value, xai_auth_uses_api,
};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

async fn execute_xai_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    spec: aether_provider_pool::ProviderPoolQuotaRequestSpec,
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
    let plan = build_provider_quota_execution_plan(
        transport,
        spec,
        proxy,
        state.resolve_transport_profile(transport),
        timeouts,
    );

    execute_provider_quota_plan(state, transport, plan, "xai").await
}

fn xai_authorization_from_header(authorization: &(String, String)) -> (String, String) {
    authorization.clone()
}

fn enrich_xai_subscription_title(mut metadata: Value, auth_config: Option<&str>) -> Value {
    if metadata
        .get("subscription_title")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return metadata;
    }
    let Some(config) = auth_config
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
    else {
        return metadata;
    };
    let title = ["subscription_tier", "subscriptionTier", "tier", "plan"]
        .iter()
        .find_map(|field| {
            config
                .get(*field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        });
    if let Some(title) = title {
        if let Some(object) = metadata.as_object_mut() {
            object.insert("subscription_title".to_string(), json!(title));
        }
    }
    metadata
}

pub(crate) async fn refresh_xai_provider_quota_locally(
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

        if xai_auth_uses_api(
            transport.key.auth_type.as_str(),
            transport.key.decrypted_auth_config.as_deref(),
        ) {
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "skipped",
                "message": "xAI API Key 账号没有 Grok Build 订阅额度接口，请使用设备授权账号查询额度。",
            }));
            continue;
        }

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

        let fallback_user_id =
            extract_xai_user_id_from_auth_config(transport.key.decrypted_auth_config.as_deref());
        let user_id = match execute_xai_quota_plan(
            state,
            &transport,
            build_xai_pool_user_request(
                &transport.key.id,
                xai_authorization_from_header(&authorization),
            ),
            proxy_override.as_ref(),
        )
        .await?
        {
            ProviderQuotaExecutionOutcome::Response(result) if result.status_code == 200 => result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(extract_xai_user_id_from_value)
                .or(fallback_user_id),
            _ => fallback_user_id,
        };

        let result = match execute_xai_quota_plan(
            state,
            &transport,
            build_xai_pool_billing_request(
                &transport.key.id,
                xai_authorization_from_header(&authorization),
                user_id.as_deref(),
            ),
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
                    "message": "xAI billing 请求执行失败",
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
                metadata_update =
                    parse_xai_billing_response(body_json, now_unix_secs).map(|metadata| {
                        json!({
                            "xai": enrich_xai_subscription_title(
                                metadata,
                                transport.key.decrypted_auth_config.as_deref(),
                            )
                        })
                    });
                if metadata_update.is_some() {
                    status = "success".to_string();
                } else {
                    status = "no_metadata".to_string();
                    message = Some("响应中未包含可用的 Grok Build 额度信息".to_string());
                }
            } else {
                status = "no_metadata".to_string();
                message = Some("响应中未包含配额信息".to_string());
            }
        } else {
            message = Some(
                extract_execution_error_message(&result)
                    .unwrap_or_else(|| format!("xAI billing 返回状态码 {}", result.status_code)),
            );
            if result.status_code == 401 || result.status_code == 403 {
                let reason = message
                    .clone()
                    .unwrap_or_else(|| "账户访问被禁止".to_string());
                oauth_invalid_at_unix_secs = Some(now_unix_secs);
                oauth_invalid_reason = Some(format!("账户访问被禁止: {reason}"));
                status = if result.status_code == 401 {
                    "unauthorized".to_string()
                } else {
                    "forbidden".to_string()
                };
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
        if let Some(metadata) = metadata_update.as_ref().and_then(|value| value.get("xai")) {
            payload.insert(
                "metadata".to_string(),
                admin_provider_metadata_bucket_safe_json("xai", Some(metadata)),
            );
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "xai",
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
