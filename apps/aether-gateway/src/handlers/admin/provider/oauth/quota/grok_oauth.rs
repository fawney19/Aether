use super::shared::{
    build_provider_quota_execution_plan, build_quota_snapshot_payload, execute_provider_quota_plan,
    extract_execution_error_message, oauth_refresh_auto_removed_result,
    persist_provider_quota_refresh_state, quota_key_auto_removed,
    quota_refresh_success_invalid_state, resolve_provider_quota_execution_timeouts,
    ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::provider::shared::payloads::OAUTH_EXPIRED_PREFIX;
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::quota::{
    parse_grok_oauth_monthly_billing_response, parse_grok_oauth_weekly_billing_response,
};
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_pool::build_grok_oauth_pool_billing_request;
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct GrokOAuthBillingPart {
    window: &'static str,
    metadata: Option<Map<String, Value>>,
    status_code: u16,
    message: Option<String>,
}

impl GrokOAuthBillingPart {
    fn succeeded(&self) -> bool {
        self.metadata.is_some()
    }
}

fn grok_oauth_auth_config(transport: &AdminGatewayProviderTransportSnapshot) -> Option<Value> {
    transport
        .key
        .decrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
}

fn parse_grok_oauth_billing_part(
    outcome: ProviderQuotaExecutionOutcome,
    weekly: bool,
    now_unix_secs: u64,
) -> GrokOAuthBillingPart {
    let window = if weekly { "weekly" } else { "monthly" };
    let endpoint_label = if weekly {
        "billing?format=credits"
    } else {
        "billing"
    };
    let result = match outcome {
        ProviderQuotaExecutionOutcome::Response(result) => result,
        ProviderQuotaExecutionOutcome::Failure(detail) => {
            return GrokOAuthBillingPart {
                window,
                metadata: None,
                status_code: 502,
                message: Some(format!("{endpoint_label} 请求执行失败: {detail}")),
            };
        }
    };

    if result.status_code != 200 {
        let detail = extract_execution_error_message(&result);
        return GrokOAuthBillingPart {
            window,
            metadata: None,
            status_code: result.status_code,
            message: Some(match detail.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!(
                        "{endpoint_label} 返回状态码 {}: {detail}",
                        result.status_code
                    )
                }
                _ => format!("{endpoint_label} 返回状态码 {}", result.status_code),
            }),
        };
    }

    let parsed = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .and_then(|body| {
            if weekly {
                parse_grok_oauth_weekly_billing_response(body, now_unix_secs)
            } else {
                parse_grok_oauth_monthly_billing_response(body, now_unix_secs)
            }
        })
        .and_then(|value| value.as_object().cloned());
    let message = parsed
        .is_none()
        .then(|| format!("{endpoint_label} 响应中未包含额度信息"));
    GrokOAuthBillingPart {
        window,
        metadata: parsed,
        status_code: result.status_code,
        message,
    }
}

fn remove_grok_oauth_billing_window(metadata: &mut Map<String, Value>, weekly: bool) {
    let fields: &[&str] = if weekly {
        &[
            "weekly_used_percent",
            "weekly_period_type",
            "weekly_period_start",
            "weekly_period_end",
            "weekly_reset_at",
            "weekly_product_usage",
            "weekly_updated_at",
        ]
    } else {
        &[
            "monthly_limit_cents",
            "monthly_used_cents",
            "monthly_included_used_cents",
            "monthly_used_percent",
            "monthly_period_start",
            "monthly_period_end",
            "monthly_reset_at",
            "monthly_updated_at",
            "plan_type",
        ]
    };
    for field in fields {
        metadata.remove(*field);
    }
}

fn build_grok_oauth_billing_metadata(
    key: &StoredProviderCatalogKey,
    weekly: &GrokOAuthBillingPart,
    monthly: &GrokOAuthBillingPart,
    now_unix_secs: u64,
) -> Option<Value> {
    if !weekly.succeeded() && !monthly.succeeded() {
        return None;
    }

    let mut metadata = key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("grok_oauth"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for part in [weekly, monthly] {
        let Some(update) = part.metadata.as_ref() else {
            continue;
        };
        remove_grok_oauth_billing_window(&mut metadata, part.window == "weekly");
        metadata.extend(update.clone());
    }

    metadata.insert("updated_at".to_string(), json!(now_unix_secs));
    metadata.insert("source".to_string(), json!("billing_probe"));
    metadata.insert("weekly_status_code".to_string(), json!(weekly.status_code));
    metadata.insert(
        "monthly_status_code".to_string(),
        json!(monthly.status_code),
    );
    let failed_windows = [weekly, monthly]
        .into_iter()
        .filter(|part| !part.succeeded())
        .map(|part| Value::String(part.window.to_string()))
        .collect::<Vec<_>>();
    metadata.insert("partial".to_string(), json!(!failed_windows.is_empty()));
    metadata.insert("failed_windows".to_string(), Value::Array(failed_windows));
    Some(json!({ "grok_oauth": metadata }))
}

fn grok_oauth_failure_message(
    weekly: &GrokOAuthBillingPart,
    monthly: &GrokOAuthBillingPart,
) -> Option<String> {
    let messages = [weekly, monthly]
        .into_iter()
        .filter_map(|part| {
            part.message
                .as_deref()
                .map(|message| format!("{}: {message}", part.window))
        })
        .collect::<Vec<_>>();
    (!messages.is_empty()).then(|| messages.join("；"))
}

fn grok_oauth_quota_invalid_state(
    key: &StoredProviderCatalogKey,
    weekly: &GrokOAuthBillingPart,
    monthly: &GrokOAuthBillingPart,
    now_unix_secs: u64,
) -> (Option<u64>, Option<String>) {
    if weekly.succeeded() || monthly.succeeded() {
        return quota_refresh_success_invalid_state(key);
    }
    if weekly.status_code == 401 || monthly.status_code == 401 {
        return (
            Some(now_unix_secs),
            Some(format!(
                "{OAUTH_EXPIRED_PREFIX}Grok OAuth Token 无效或已过期"
            )),
        );
    }
    (
        key.oauth_invalid_at_unix_secs,
        key.oauth_invalid_reason.clone(),
    )
}

pub(crate) async fn refresh_grok_oauth_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> Result<Option<Value>, GatewayError> {
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

        let authorization = match state.resolve_local_oauth_header_auth(&transport).await? {
            Some(authorization) => authorization,
            None => {
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
                    "message": "缺少 Grok OAuth 认证信息，请先授权/刷新 Token",
                }));
                continue;
            }
        };

        let auth_config = grok_oauth_auth_config(&transport);
        let weekly_spec = match build_grok_oauth_pool_billing_request(
            &key.id,
            &endpoint.base_url,
            authorization.clone(),
            auth_config.as_ref(),
            true,
        ) {
            Ok(spec) => spec,
            Err(message) => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": message,
                }));
                continue;
            }
        };
        let monthly_spec = match build_grok_oauth_pool_billing_request(
            &key.id,
            &endpoint.base_url,
            authorization,
            auth_config.as_ref(),
            false,
        ) {
            Ok(spec) => spec,
            Err(message) => {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": message,
                }));
                continue;
            }
        };

        let proxy = match proxy_override.as_ref() {
            Some(proxy) => Some(proxy.clone()),
            None => {
                state
                    .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
                    .await
            }
        };
        let timeouts = Some(resolve_provider_quota_execution_timeouts(
            state.resolve_transport_execution_timeouts(&transport),
            proxy.as_ref(),
        ));
        let weekly_plan = build_provider_quota_execution_plan(
            &transport,
            weekly_spec,
            proxy.clone(),
            state.resolve_transport_profile(&transport),
            timeouts.clone(),
        );
        let monthly_plan = build_provider_quota_execution_plan(
            &transport,
            monthly_spec,
            proxy,
            state.resolve_transport_profile(&transport),
            timeouts,
        );
        let (weekly_outcome, monthly_outcome) = tokio::join!(
            execute_provider_quota_plan(state, &transport, weekly_plan, "grok_oauth_weekly"),
            execute_provider_quota_plan(state, &transport, monthly_plan, "grok_oauth_monthly"),
        );
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let weekly = parse_grok_oauth_billing_part(weekly_outcome?, true, now_unix_secs);
        let monthly = parse_grok_oauth_billing_part(monthly_outcome?, false, now_unix_secs);
        let metadata_update =
            build_grok_oauth_billing_metadata(&key, &weekly, &monthly, now_unix_secs);
        let any_success = metadata_update.is_some();
        let (oauth_invalid_at, oauth_invalid_reason) =
            grok_oauth_quota_invalid_state(&key, &weekly, &monthly, now_unix_secs);

        if !persist_provider_quota_refresh_state(
            state,
            &key.id,
            metadata_update.as_ref(),
            oauth_invalid_at,
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

        let mut payload = Map::new();
        payload.insert("key_id".to_string(), json!(key.id));
        payload.insert("key_name".to_string(), json!(key.name));
        payload.insert(
            "status".to_string(),
            json!(if any_success { "success" } else { "error" }),
        );
        payload.insert(
            "partial".to_string(),
            json!(any_success && (!weekly.succeeded() || !monthly.succeeded())),
        );
        if let Some(message) = grok_oauth_failure_message(&weekly, &monthly) {
            payload.insert("message".to_string(), json!(message));
        }
        if let Some(metadata) = metadata_update
            .as_ref()
            .and_then(|value| value.get("grok_oauth"))
            .cloned()
        {
            payload.insert("metadata".to_string(), metadata);
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "grok_oauth",
            key.status_snapshot.as_ref(),
            metadata_update.as_ref(),
        ) {
            payload.insert("quota_snapshot".to_string(), quota_snapshot);
        }
        if any_success {
            success_count += 1;
        } else {
            failed_count += 1;
        }
        results.push(Value::Object(payload));
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

#[cfg(test)]
mod tests {
    use super::{
        build_grok_oauth_billing_metadata, grok_oauth_quota_invalid_state,
        parse_grok_oauth_billing_part, GrokOAuthBillingPart,
    };
    use aether_contracts::{ExecutionResult, ResponseBody};
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
    }

    fn successful_outcome(body: serde_json::Value) -> super::ProviderQuotaExecutionOutcome {
        super::ProviderQuotaExecutionOutcome::Response(ExecutionResult {
            request_id: "request-1".to_string(),
            candidate_id: None,
            status_code: 200,
            headers: BTreeMap::new(),
            body: Some(ResponseBody {
                json_body: Some(body),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        })
    }

    #[test]
    fn billing_metadata_keeps_previous_window_on_partial_success() {
        let mut key = sample_key();
        key.upstream_metadata = Some(json!({
            "grok_oauth": {
                "weekly_used_percent": 70.0,
                "weekly_reset_at": 2_000_000_000u64,
                "monthly_used_percent": 10.0
            }
        }));
        let weekly = GrokOAuthBillingPart {
            window: "weekly",
            metadata: None,
            status_code: 503,
            message: Some("temporary".to_string()),
        };
        let monthly = parse_grok_oauth_billing_part(
            successful_outcome(json!({
                "config": {
                    "monthlyLimit": { "val": 15000 },
                    "used": { "val": 3000 }
                }
            })),
            false,
            1_800_000_000,
        );

        let metadata = build_grok_oauth_billing_metadata(&key, &weekly, &monthly, 1_800_000_000)
            .expect("partial metadata should build");
        assert_eq!(metadata["grok_oauth"]["weekly_used_percent"], json!(70.0));
        assert_eq!(metadata["grok_oauth"]["monthly_used_percent"], json!(20.0));
        assert_eq!(metadata["grok_oauth"]["partial"], json!(true));
        assert_eq!(metadata["grok_oauth"]["failed_windows"], json!(["weekly"]));
    }

    #[test]
    fn billing_403_is_retained_without_marking_oauth_invalid() {
        let key = sample_key();
        let weekly = GrokOAuthBillingPart {
            window: "weekly",
            metadata: None,
            status_code: 403,
            message: None,
        };
        let monthly = GrokOAuthBillingPart {
            window: "monthly",
            metadata: None,
            status_code: 403,
            message: None,
        };

        assert_eq!(
            grok_oauth_quota_invalid_state(&key, &weekly, &monthly, 1_800_000_000),
            (None, None)
        );
    }

    #[test]
    fn billing_401_marks_oauth_expired_only_when_no_window_succeeds() {
        let key = sample_key();
        let failed = GrokOAuthBillingPart {
            window: "weekly",
            metadata: None,
            status_code: 401,
            message: None,
        };
        let succeeded = parse_grok_oauth_billing_part(
            successful_outcome(json!({
                "config": {
                    "monthlyLimit": { "val": 15000 },
                    "used": { "val": 3000 }
                }
            })),
            false,
            1_800_000_000,
        );

        assert_eq!(
            grok_oauth_quota_invalid_state(&key, &failed, &succeeded, 1_800_000_000),
            (None, None)
        );
        let monthly_failed = GrokOAuthBillingPart {
            window: "monthly",
            metadata: None,
            status_code: 401,
            message: None,
        };
        let invalid = grok_oauth_quota_invalid_state(&key, &failed, &monthly_failed, 1_800_000_000);
        assert_eq!(invalid.0, Some(1_800_000_000));
        assert!(invalid
            .1
            .as_deref()
            .is_some_and(|reason| reason.starts_with("[OAUTH_EXPIRED] ")));
    }
}
