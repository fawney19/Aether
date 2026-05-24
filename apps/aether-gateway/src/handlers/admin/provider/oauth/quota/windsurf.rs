use super::shared::{
    build_provider_quota_execution_plan, build_quota_snapshot_payload,
    default_provider_quota_execution_timeouts, execute_provider_quota_plan,
    extract_execution_error_message, persist_provider_quota_refresh_state,
    quota_refresh_success_invalid_state, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_pool::{
    build_windsurf_pool_model_configs_request_with_base_url,
    build_windsurf_pool_quota_request_with_base_url,
    build_windsurf_pool_rate_limit_request_with_base_url, ProviderPoolQuotaRequestSpec,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

async fn execute_windsurf_probe_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    spec: ProviderPoolQuotaRequestSpec,
    proxy_override: Option<&ProxySnapshot>,
    quota_kind: &str,
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

    execute_provider_quota_plan(state, transport, plan, quota_kind).await
}

async fn execute_windsurf_user_status_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    api_key: &str,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let spec = build_windsurf_pool_quota_request_with_base_url(
        &transport.key.id,
        &transport.endpoint.base_url,
        api_key,
    );
    execute_windsurf_probe_plan(
        state,
        transport,
        spec,
        proxy_override,
        "windsurf:user_status",
    )
    .await
}

async fn execute_windsurf_model_configs_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    api_key: &str,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let spec = build_windsurf_pool_model_configs_request_with_base_url(
        &transport.key.id,
        &transport.endpoint.base_url,
        api_key,
    );
    execute_windsurf_probe_plan(
        state,
        transport,
        spec,
        proxy_override,
        "windsurf:model_configs",
    )
    .await
}

async fn execute_windsurf_rate_limit_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    api_key: &str,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let spec = build_windsurf_pool_rate_limit_request_with_base_url(
        &transport.key.id,
        &transport.endpoint.base_url,
        api_key,
    );
    execute_windsurf_probe_plan(
        state,
        transport,
        spec,
        proxy_override,
        "windsurf:rate_limit",
    )
    .await
}

fn merge_windsurf_probe_metadata(
    mut user_status_metadata: serde_json::Value,
    model_configs_metadata: Option<serde_json::Value>,
    rate_limit_metadata: Option<serde_json::Value>,
) -> serde_json::Value {
    let Some(target) = user_status_metadata.as_object_mut() else {
        return user_status_metadata;
    };
    for metadata in [model_configs_metadata, rate_limit_metadata]
        .into_iter()
        .flatten()
    {
        if let Some(source) = metadata.as_object() {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    user_status_metadata
}

fn append_windsurf_probe_warning(metadata: &mut serde_json::Value, probe: &str, message: String) {
    let Some(target) = metadata.as_object_mut() else {
        return;
    };
    let warnings = target
        .entry("probe_warnings".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if let Some(items) = warnings.as_array_mut() {
        items.push(json!({
            "probe": probe,
            "message": message,
        }));
    }
}

fn build_windsurf_metadata_update(
    current_upstream_metadata: Option<&serde_json::Value>,
    patch: serde_json::Value,
) -> serde_json::Value {
    let Some(patch_object) = patch.as_object() else {
        return json!({ "windsurf": patch });
    };
    let mut merged_bucket = current_upstream_metadata
        .and_then(|value| value.get("windsurf"))
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (key, value) in patch_object {
        merged_bucket.insert(key.clone(), value.clone());
    }
    json!({ "windsurf": merged_bucket })
}

fn sanitize_windsurf_probe_detail(detail: impl AsRef<str>) -> String {
    let detail = detail.as_ref().trim();
    if detail.is_empty() {
        return "-".to_string();
    }
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(detail) {
        redact_windsurf_sensitive_json(&mut value);
        return value.to_string().chars().take(500).collect();
    }
    if contains_windsurf_sensitive_marker(detail) {
        "[REDACTED upstream error body]".to_string()
    } else {
        detail.chars().take(500).collect()
    }
}

fn redact_windsurf_sensitive_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if is_windsurf_sensitive_key(key) {
                    *value = json!("[REDACTED]");
                } else {
                    redact_windsurf_sensitive_json(value);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_windsurf_sensitive_json(item);
            }
        }
        serde_json::Value::String(text) if looks_like_windsurf_secret(text) => {
            *text = "[REDACTED]".to_string();
        }
        _ => {}
    }
}

fn is_windsurf_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("password")
        || normalized.contains("authorization")
        || normalized.contains("secret")
}

fn looks_like_windsurf_secret(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("devin-session-token$")
        || value.starts_with("sk-")
        || (value.len() > 80 && value.split('.').count() == 3)
}

fn contains_windsurf_sensitive_marker(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    [
        "token",
        "api_key",
        "apikey",
        "sessiontoken",
        "firebase_id_token",
        "idtoken",
        "authorization",
        "password",
        "secret",
        "devin-session-token$",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
        || value.contains("sk-")
}

pub(crate) async fn refresh_windsurf_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
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

        let api_key = transport.key.decrypted_api_key.trim();
        if api_key.is_empty() {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "缺少 Windsurf apiKey/sessionToken",
            }));
            continue;
        }

        let result = match execute_windsurf_user_status_plan(
            state,
            &transport,
            api_key,
            proxy_override.as_ref(),
        )
        .await?
        {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                failed_count += 1;
                let detail = sanitize_windsurf_probe_detail(detail);
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": format!("GetUserStatus 请求执行失败: {detail}"),
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
                let mut windsurf_metadata =
                    aether_admin::provider::quota::parse_windsurf_user_status_response(
                        body_json,
                        now_unix_secs,
                    );
                if let Some(mut metadata) = windsurf_metadata.take() {
                    let model_metadata = match execute_windsurf_model_configs_plan(
                        state,
                        &transport,
                        api_key,
                        proxy_override.as_ref(),
                    )
                    .await?
                    {
                        ProviderQuotaExecutionOutcome::Response(model_result)
                            if model_result.status_code == 200 =>
                        {
                            model_result
                                .body
                                .as_ref()
                                .and_then(|body| body.json_body.as_ref())
                                .and_then(|body_json| {
                                    aether_admin::provider::quota::parse_windsurf_model_configs_response(
                                        body_json,
                                        now_unix_secs,
                                    )
                                })
                        }
                        ProviderQuotaExecutionOutcome::Response(model_result) => {
                            let detail = extract_execution_error_message(&model_result)
                                .unwrap_or_else(|| format!("HTTP {}", model_result.status_code));
                            let detail = sanitize_windsurf_probe_detail(detail);
                            append_windsurf_probe_warning(
                                &mut metadata,
                                "model_configs",
                                format!("GetCascadeModelConfigs 返回: {detail}"),
                            );
                            None
                        }
                        ProviderQuotaExecutionOutcome::Failure(detail) => {
                            let detail = sanitize_windsurf_probe_detail(detail);
                            append_windsurf_probe_warning(
                                &mut metadata,
                                "model_configs",
                                format!("GetCascadeModelConfigs 执行失败: {detail}"),
                            );
                            None
                        }
                    };
                    let rate_limit_metadata = match execute_windsurf_rate_limit_plan(
                        state,
                        &transport,
                        api_key,
                        proxy_override.as_ref(),
                    )
                    .await?
                    {
                        ProviderQuotaExecutionOutcome::Response(rate_limit_result)
                            if rate_limit_result.status_code == 200 =>
                        {
                            rate_limit_result
                                .body
                                .as_ref()
                                .and_then(|body| body.json_body.as_ref())
                                .and_then(|body_json| {
                                    aether_admin::provider::quota::parse_windsurf_rate_limit_response(
                                        body_json,
                                        now_unix_secs,
                                    )
                                })
                        }
                        ProviderQuotaExecutionOutcome::Response(rate_limit_result) => {
                            let detail = extract_execution_error_message(&rate_limit_result)
                                .unwrap_or_else(|| format!("HTTP {}", rate_limit_result.status_code));
                            let detail = sanitize_windsurf_probe_detail(detail);
                            append_windsurf_probe_warning(
                                &mut metadata,
                                "rate_limit",
                                format!("CheckUserMessageRateLimit 返回: {detail}"),
                            );
                            None
                        }
                        ProviderQuotaExecutionOutcome::Failure(detail) => {
                            let detail = sanitize_windsurf_probe_detail(detail);
                            append_windsurf_probe_warning(
                                &mut metadata,
                                "rate_limit",
                                format!("CheckUserMessageRateLimit 执行失败: {detail}"),
                            );
                            None
                        }
                    };
                    metadata = merge_windsurf_probe_metadata(
                        metadata,
                        model_metadata,
                        rate_limit_metadata,
                    );
                    metadata_update = Some(build_windsurf_metadata_update(
                        key.upstream_metadata.as_ref(),
                        metadata,
                    ));
                    status = "success".to_string();
                } else {
                    status = "no_metadata".to_string();
                    message = Some("响应中未包含 Windsurf 限额信息".to_string());
                }
            } else {
                status = "no_metadata".to_string();
                message = Some("无法解析 GetUserStatus 响应".to_string());
            }
        } else {
            let err_msg =
                extract_execution_error_message(&result).map(sanitize_windsurf_probe_detail);
            message = Some(match err_msg.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!(
                        "GetUserStatus 返回状态码 {}: {}",
                        result.status_code, detail
                    )
                }
                _ => format!("GetUserStatus 返回状态码 {}", result.status_code),
            });
            let detail = err_msg
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("HTTP {}", result.status_code));
            let mut metadata = serde_json::Map::new();
            metadata.insert("updated_at".to_string(), json!(now_unix_secs));
            metadata.insert("last_error".to_string(), json!(detail));
            match result.status_code {
                401 | 403 => {
                    oauth_invalid_at_unix_secs = Some(now_unix_secs);
                    oauth_invalid_reason =
                        Some(format!("Windsurf token 无效或已被拒绝: {}", detail));
                    metadata.insert("banned".to_string(), json!(result.status_code == 403));
                    status = if result.status_code == 401 {
                        "auth_invalid".to_string()
                    } else {
                        "forbidden".to_string()
                    };
                }
                429 => {
                    metadata.insert(
                        "rate_limit".to_string(),
                        json!({
                            "limited": true,
                            "message": metadata
                                .get("last_error")
                                .cloned()
                                .unwrap_or_else(|| json!("rate limited")),
                        }),
                    );
                    status = "rate_limited".to_string();
                }
                _ => {}
            }
            metadata_update = Some(build_windsurf_metadata_update(
                key.upstream_metadata.as_ref(),
                serde_json::Value::Object(metadata),
            ));
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
        if result.status_code != 200 {
            payload.insert("status_code".to_string(), json!(result.status_code));
        }
        if let Some(metadata) = metadata_update
            .as_ref()
            .and_then(|value| value.get("windsurf"))
            .cloned()
        {
            payload.insert("metadata".to_string(), metadata);
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "windsurf",
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
        "total": success_count + failed_count,
        "results": results,
        "message": format!("已处理 {} 个 Key", success_count + failed_count),
        "auto_removed": 0,
    })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn windsurf_probe_metadata_merges_user_status_models_and_rate_limit() {
        let metadata = super::merge_windsurf_probe_metadata(
            json!({
                "plan_name": "Pro",
                "daily_remaining_percent": 42.0,
                "updated_at": 1_770_000_000u64,
            }),
            Some(json!({
                "allowed_models_count": 2u64,
                "models": [
                    {"model_uid": "claude-sonnet-4-5"},
                    {"model_uid": "gpt-5-mini"}
                ],
                "updated_at": 1_770_000_010u64,
            })),
            Some(json!({
                "rate_limit": {
                    "limited": true,
                    "messages_remaining": 0.0,
                    "retry_after_ms": 60_000u64
                },
                "updated_at": 1_770_000_020u64,
            })),
        );

        assert_eq!(metadata["plan_name"], json!("Pro"));
        assert_eq!(metadata["daily_remaining_percent"], json!(42.0));
        assert_eq!(metadata["allowed_models_count"], json!(2u64));
        assert_eq!(metadata["rate_limit"]["limited"], json!(true));
        assert_eq!(metadata["updated_at"], json!(1_770_000_020u64));
    }

    #[test]
    fn windsurf_probe_detail_redacts_sensitive_values() {
        let detail = super::sanitize_windsurf_probe_detail(
            r#"{"error":{"message":"bad"},"apiKey":"sk-secret","sessionToken":"devin-session-token$secret"}"#,
        );

        assert!(detail.contains("[REDACTED]"));
        assert!(!detail.contains("sk-secret"));
        assert!(!detail.contains("devin-session-token$secret"));
    }

    #[test]
    fn windsurf_metadata_update_preserves_existing_bucket_fields() {
        let update = super::build_windsurf_metadata_update(
            Some(&json!({
                "windsurf": {
                    "daily_remaining_percent": 0.0,
                    "allowed_models_count": 3,
                    "updated_at": 1u64
                }
            })),
            json!({
                "last_error": "HTTP 429",
                "rate_limit": {"limited": true},
                "updated_at": 2u64
            }),
        );

        assert_eq!(
            update.pointer("/windsurf/daily_remaining_percent"),
            Some(&json!(0.0))
        );
        assert_eq!(
            update.pointer("/windsurf/allowed_models_count"),
            Some(&json!(3))
        );
        assert_eq!(update.pointer("/windsurf/updated_at"), Some(&json!(2u64)));
        assert_eq!(
            update.pointer("/windsurf/rate_limit/limited"),
            Some(&json!(true))
        );
    }
}
