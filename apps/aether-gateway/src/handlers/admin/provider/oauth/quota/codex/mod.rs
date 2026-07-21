mod invalid;
mod parse;
mod plan;

use self::invalid::{
    codex_build_invalid_state, codex_looks_like_token_expired, codex_looks_like_token_invalidated,
    codex_looks_like_workspace_deactivated, codex_soft_request_failure_reason,
    codex_structured_invalid_reason,
};
use self::parse::{
    build_codex_quota_exhausted_fallback_metadata, normalize_codex_reset_credit_consume_outcome,
    parse_codex_usage_headers, parse_codex_wham_reset_credits_detail_response,
    parse_codex_wham_usage_response,
};
use self::plan::{
    build_codex_quota_request_spec, build_codex_reset_credit_consume_request_spec,
    build_codex_reset_credits_request_spec, execute_codex_quota_plan,
    execute_codex_reset_credit_plan,
};
use super::shared::{
    build_quota_snapshot_payload, extract_execution_error_message,
    oauth_refresh_auto_removed_result, persist_provider_quota_refresh_state,
    provider_auto_remove_banned_keys, provider_auto_remove_quota_exhausted_keys,
    quota_key_auto_removed, quota_refresh_success_invalid_state,
    should_auto_remove_structured_reason, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::http::StatusCode;
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

fn merge_codex_quota_metadata(
    header_metadata: Option<&serde_json::Value>,
    body_metadata: &serde_json::Value,
) -> serde_json::Value {
    let mut merged = header_metadata
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(body_object) = body_metadata.as_object() {
        for (key, value) in body_object {
            merged.insert(key.clone(), value.clone());
        }
    }
    serde_json::Value::Object(merged)
}

fn truncate_codex_reset_credit_detail_error(message: impl Into<String>) -> String {
    let message = message.into();
    let mut sanitized = message.replace('\n', " ");
    if sanitized.len() > 240 {
        sanitized.truncate(240);
        sanitized.push('…');
    }
    sanitized
}

fn merge_codex_reset_credit_detail_metadata(
    codex_metadata: &mut Map<String, Value>,
    detail_metadata: &Value,
) {
    let Some(detail_reset_credits) = detail_metadata
        .get("reset_credits")
        .and_then(Value::as_object)
    else {
        return;
    };
    let mut reset_credits = codex_metadata
        .get("reset_credits")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for (key, value) in detail_reset_credits {
        reset_credits.insert(key.clone(), value.clone());
    }
    codex_metadata.insert("reset_credits".to_string(), Value::Object(reset_credits));
}

fn mark_codex_reset_credit_detail_failed(
    codex_metadata: &mut Map<String, Value>,
    updated_at_unix_secs: u64,
    detail_error: impl Into<String>,
) {
    let mut reset_credits = codex_metadata
        .get("reset_credits")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    reset_credits.insert("updated_at".to_string(), json!(updated_at_unix_secs));
    reset_credits.insert("detail_source".to_string(), json!("wham_readonly"));
    reset_credits.insert("detail_status".to_string(), json!("failed"));
    reset_credits.insert(
        "detail_error".to_string(),
        json!(truncate_codex_reset_credit_detail_error(detail_error)),
    );
    reset_credits
        .entry("credits".to_string())
        .or_insert_with(|| json!([]));
    codex_metadata.insert("reset_credits".to_string(), Value::Object(reset_credits));
}

async fn enrich_codex_reset_credit_details(
    state: &AdminAppState<'_>,
    transport: &crate::handlers::admin::request::AdminGatewayProviderTransportSnapshot,
    resolved_oauth_auth: Option<(String, String)>,
    proxy_override: Option<&ProxySnapshot>,
    codex_metadata: &mut Map<String, Value>,
    now_unix_secs: u64,
) -> Result<(), GatewayError> {
    let request_spec = match build_codex_reset_credits_request_spec(transport, resolved_oauth_auth)
    {
        Ok(request_spec) => request_spec,
        Err(message) => {
            mark_codex_reset_credit_detail_failed(codex_metadata, now_unix_secs, message);
            return Ok(());
        }
    };

    let result =
        match execute_codex_reset_credit_plan(state, transport, request_spec, proxy_override)
            .await?
        {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                mark_codex_reset_credit_detail_failed(
                    codex_metadata,
                    now_unix_secs,
                    format!("reset credit detail 请求执行失败: {detail}"),
                );
                return Ok(());
            }
        };

    if result.status_code != 200 {
        let detail = extract_execution_error_message(&result)
            .unwrap_or_else(|| format!("HTTP {}", result.status_code));
        mark_codex_reset_credit_detail_failed(
            codex_metadata,
            now_unix_secs,
            format!(
                "reset credit detail 返回状态码 {}: {detail}",
                result.status_code
            ),
        );
        return Ok(());
    }

    let Some(body_json) = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
    else {
        mark_codex_reset_credit_detail_failed(
            codex_metadata,
            now_unix_secs,
            "无法解析 reset credit detail 响应",
        );
        return Ok(());
    };
    if let Some(detail_metadata) =
        parse_codex_wham_reset_credits_detail_response(body_json, now_unix_secs)
    {
        merge_codex_reset_credit_detail_metadata(codex_metadata, &detail_metadata);
    } else {
        mark_codex_reset_credit_detail_failed(
            codex_metadata,
            now_unix_secs,
            "reset credit detail 响应为空",
        );
    }

    Ok(())
}

fn codex_oauth_refresh_issue_reason(reason: Option<&str>) -> bool {
    reason.is_some_and(|reason| {
        reason
            .lines()
            .map(str::trim)
            .any(|line| line.starts_with("[OAUTH_EXPIRED]") || line.starts_with("[REFRESH_FAILED]"))
    })
}

fn codex_consume_success_status(outcome: &str) -> &'static str {
    match outcome {
        "reset" | "already_redeemed" => "success",
        "nothing_to_reset" | "no_credit" => "noop",
        _ => "unknown",
    }
}

fn codex_extract_refresh_result_fields(
    refresh_payload: Option<&Value>,
    key_id: &str,
) -> (String, Option<String>, Option<Value>, Option<Value>) {
    let Some(result) = refresh_payload
        .and_then(|payload| payload.get("results"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find(|item| item.get("key_id").and_then(Value::as_str) == Some(key_id))
    else {
        return (
            "failed".to_string(),
            Some("刷新结果中缺少当前 key".to_string()),
            None,
            None,
        );
    };

    let status = result
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let refresh_status = if status.eq_ignore_ascii_case("success") {
        "success"
    } else {
        "failed"
    }
    .to_string();
    let refresh_error = if refresh_status == "success" {
        None
    } else {
        result
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    (
        refresh_status,
        refresh_error,
        result.get("metadata").cloned(),
        result.get("quota_snapshot").cloned(),
    )
}

pub(crate) async fn consume_codex_reset_credit_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: StoredProviderCatalogKey,
    idempotency_key: &str,
) -> Result<(StatusCode, Value), GatewayError> {
    let transport = match state
        .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
        .await?
    {
        Some(transport) => transport,
        None => {
            return Ok((
                StatusCode::BAD_GATEWAY,
                json!({
                    "key_id": key.id,
                    "status": "error",
                    "outcome": "error",
                    "message": "Provider transport snapshot unavailable",
                }),
            ));
        }
    };

    let is_oauth_managed = provider_key_is_oauth_managed(&key, provider.provider_type.as_str());
    let resolved_oauth_auth = if is_oauth_managed {
        state.resolve_local_oauth_header_auth(&transport).await?
    } else {
        None
    };
    if is_oauth_managed && resolved_oauth_auth.is_none() {
        return Ok((
            StatusCode::BAD_REQUEST,
            json!({
                "key_id": key.id,
                "status": "error",
                "outcome": "error",
                "message": "缺少 Codex OAuth 认证信息，请先重新授权/刷新 Token",
            }),
        ));
    }

    let request_spec = match build_codex_reset_credit_consume_request_spec(
        &transport,
        resolved_oauth_auth,
        idempotency_key,
    ) {
        Ok(request_spec) => request_spec,
        Err(message) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                json!({
                    "key_id": key.id,
                    "status": "error",
                    "outcome": "error",
                    "message": message,
                }),
            ));
        }
    };

    let result =
        match execute_codex_reset_credit_plan(state, &transport, request_spec, None).await? {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                return Ok((
                    StatusCode::BAD_GATEWAY,
                    json!({
                        "key_id": key.id,
                        "status": "error",
                        "outcome": "error",
                        "message": format!("reset credit consume 请求执行失败: {detail}"),
                    }),
                ));
            }
        };

    let body_json = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref());
    let outcome = normalize_codex_reset_credit_consume_outcome(body_json)
        .unwrap_or_else(|| "unknown".to_string());
    let known_non_error_outcome = matches!(
        outcome.as_str(),
        "reset" | "already_redeemed" | "nothing_to_reset" | "no_credit"
    );
    if result.status_code >= 400 && !known_non_error_outcome {
        let detail = extract_execution_error_message(&result)
            .unwrap_or_else(|| format!("HTTP {}", result.status_code));
        return Ok((
            StatusCode::BAD_GATEWAY,
            json!({
                "key_id": key.id,
                "status": "error",
                "outcome": "error",
                "idempotency_key": idempotency_key,
                "message": format!("reset credit consume 返回状态码 {}: {detail}", result.status_code),
                "status_code": result.status_code,
            }),
        ));
    }

    let (refresh_status, refresh_error, metadata, quota_snapshot) =
        match refresh_codex_provider_quota_locally(
            state,
            provider,
            endpoint,
            vec![key.clone()],
            None,
        )
        .await
        {
            Ok(refresh_payload) => {
                codex_extract_refresh_result_fields(refresh_payload.as_ref(), &key.id)
            }
            Err(err) => (
                "failed".to_string(),
                Some(truncate_codex_reset_credit_detail_error(err.into_message())),
                None,
                None,
            ),
        };

    let mut payload = Map::new();
    payload.insert("key_id".to_string(), json!(key.id));
    payload.insert(
        "status".to_string(),
        json!(codex_consume_success_status(&outcome)),
    );
    payload.insert("outcome".to_string(), json!(outcome));
    payload.insert("idempotency_key".to_string(), json!(idempotency_key));
    payload.insert("refresh_status".to_string(), json!(refresh_status));
    if let Some(refresh_error) = refresh_error {
        payload.insert("refresh_error".to_string(), json!(refresh_error));
    }
    if let Some(metadata) = metadata {
        payload.insert("metadata".to_string(), metadata);
    }
    if let Some(quota_snapshot) = quota_snapshot {
        payload.insert("quota_snapshot".to_string(), quota_snapshot);
    }

    Ok((StatusCode::OK, Value::Object(payload)))
}

pub(crate) async fn refresh_codex_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let auto_remove_abnormal_keys = provider_auto_remove_banned_keys(provider.config.as_ref());
    let auto_remove_quota_exhausted_keys =
        provider_auto_remove_quota_exhausted_keys(provider.config.as_ref());
    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut failed_count = 0usize;
    let mut auto_removed_count = 0usize;
    let mut refresh_fixed_count = 0usize;
    let mut refresh_failed_retained_count = 0usize;
    let mut auto_removed_hard_banned_count = 0usize;

    for key in keys {
        let had_oauth_refresh_issue =
            codex_oauth_refresh_issue_reason(key.oauth_invalid_reason.as_deref());
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

        let is_oauth_managed = provider_key_is_oauth_managed(&key, provider.provider_type.as_str());
        let resolved_oauth_auth = if is_oauth_managed {
            state.resolve_local_oauth_header_auth(&transport).await?
        } else {
            None
        };
        if is_oauth_managed && quota_key_auto_removed(state, &key.id).await? {
            auto_removed_count += 1;
            results.push(oauth_refresh_auto_removed_result(&key));
            continue;
        }
        if is_oauth_managed && resolved_oauth_auth.is_none() {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "缺少 Codex OAuth 认证信息，请先重新授权/刷新 Token",
            }));
            continue;
        }

        let request_spec =
            match build_codex_quota_request_spec(&transport, resolved_oauth_auth.clone()) {
                Ok(request_spec) => request_spec,
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

        let result = match execute_codex_quota_plan(
            state,
            &transport,
            request_spec,
            proxy_override.as_ref(),
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
                    "message": format!("wham/usage 请求执行失败: {detail}"),
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

        let header_metadata = parse_codex_usage_headers(&result.headers, now_unix_secs);
        let mut metadata_update = header_metadata
            .as_ref()
            .map(|metadata| json!({ "codex": metadata }));
        let (mut oauth_invalid_at_unix_secs, mut oauth_invalid_reason) = (None, None);
        let mut status = "error".to_string();
        let mut message = None::<String>;
        let mut status_code = Some(result.status_code);

        if result.status_code == 200 {
            if let Some(body_json) = result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
            {
                if let Some(parsed) = parse_codex_wham_usage_response(body_json, now_unix_secs) {
                    let mut codex_metadata =
                        match merge_codex_quota_metadata(header_metadata.as_ref(), &parsed) {
                            Value::Object(object) => object,
                            _ => Map::new(),
                        };
                    enrich_codex_reset_credit_details(
                        state,
                        &transport,
                        resolved_oauth_auth.clone(),
                        proxy_override.as_ref(),
                        &mut codex_metadata,
                        now_unix_secs,
                    )
                    .await?;
                    metadata_update = Some(json!({
                        "codex": codex_metadata
                    }));
                    (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
                        quota_refresh_success_invalid_state(&key);
                    status = "success".to_string();
                } else if metadata_update.is_some() {
                    (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
                        quota_refresh_success_invalid_state(&key);
                    status = "success".to_string();
                } else {
                    status = "no_metadata".to_string();
                    message = Some("响应中未包含限额信息".to_string());
                }
            } else {
                message = Some("无法解析 wham/usage API 响应".to_string());
            }
        } else {
            let err_msg = extract_execution_error_message(&result);
            message = Some(match err_msg.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!(
                        "wham/usage API 返回状态码 {}: {}",
                        result.status_code, detail
                    )
                }
                _ => format!("wham/usage API 返回状态码 {}", result.status_code),
            });

            match result.status_code {
                401 => {
                    let (at, reason) = codex_build_invalid_state(
                        &key,
                        codex_structured_invalid_reason(401, err_msg.as_deref()),
                        now_unix_secs,
                    );
                    oauth_invalid_at_unix_secs = at;
                    oauth_invalid_reason = reason;
                    status = "auth_invalid".to_string();
                }
                402 => {
                    if codex_looks_like_workspace_deactivated(err_msg.as_deref()) {
                        let mut codex_meta = metadata_update
                            .as_ref()
                            .and_then(|value| value.get("codex"))
                            .and_then(serde_json::Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        codex_meta.insert("updated_at".to_string(), json!(now_unix_secs));
                        codex_meta.insert("account_disabled".to_string(), json!(true));
                        codex_meta.insert("reason".to_string(), json!("deactivated_workspace"));
                        codex_meta.insert(
                            "message".to_string(),
                            json!(err_msg
                                .clone()
                                .unwrap_or_else(|| "deactivated_workspace".to_string())),
                        );
                        let plan_type = transport
                            .key
                            .decrypted_auth_config
                            .as_deref()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                            .and_then(|value| {
                                value
                                    .get("plan_type")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned)
                            });
                        if let Some(plan_type) = plan_type {
                            codex_meta
                                .entry("plan_type".to_string())
                                .or_insert_with(|| json!(plan_type.to_ascii_lowercase()));
                        }
                        metadata_update = Some(json!({ "codex": codex_meta }));
                        let (at, reason) = codex_build_invalid_state(
                            &key,
                            codex_structured_invalid_reason(402, err_msg.as_deref()),
                            now_unix_secs,
                        );
                        oauth_invalid_at_unix_secs = at;
                        oauth_invalid_reason = reason;
                        status = "workspace_deactivated".to_string();
                    } else {
                        let plan_type = transport
                            .key
                            .decrypted_auth_config
                            .as_deref()
                            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                            .and_then(|value| {
                                value
                                    .get("plan_type")
                                    .and_then(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned)
                            });
                        metadata_update = Some(json!({
                            "codex": build_codex_quota_exhausted_fallback_metadata(
                                plan_type.as_deref(),
                                now_unix_secs,
                            )
                        }));
                        (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
                            quota_refresh_success_invalid_state(&key);
                        status = "quota_exhausted".to_string();
                    }
                }
                403 => {
                    let candidate_reason = if codex_looks_like_token_invalidated(err_msg.as_deref())
                        || codex_looks_like_token_expired(err_msg.as_deref())
                    {
                        codex_structured_invalid_reason(403, err_msg.as_deref())
                    } else {
                        codex_soft_request_failure_reason(403, err_msg.as_deref())
                    };
                    let (at, reason) =
                        codex_build_invalid_state(&key, candidate_reason, now_unix_secs);
                    oauth_invalid_at_unix_secs = at;
                    oauth_invalid_reason = reason;
                    status = "forbidden".to_string();
                }
                _ => {}
            }
        }

        let auto_remove_candidate = auto_remove_abnormal_keys
            && should_auto_remove_structured_reason(oauth_invalid_reason.as_deref());
        let persisted = persist_provider_quota_refresh_state(
            state,
            &key.id,
            metadata_update.as_ref(),
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason.clone(),
            None,
        )
        .await?;
        if !persisted {
            failed_count += 1;
            results.push(json!({
                "key_id": key.id,
                "key_name": key.name,
                "status": "error",
                "message": "Key 状态写入失败",
            }));
            continue;
        }
        let auto_removed_hard_banned = if auto_remove_candidate {
            state
                .cleanup_provider_catalog_key_if_current(provider, &key.id, |latest_key| {
                    should_auto_remove_structured_reason(latest_key.oauth_invalid_reason.as_deref())
                })
                .await?
        } else {
            false
        };
        if auto_removed_hard_banned {
            auto_removed_count += 1;
            auto_removed_hard_banned_count += 1;
        }
        let auto_removed_quota_exhausted =
            if !auto_removed_hard_banned && auto_remove_quota_exhausted_keys {
                state
                    .cleanup_provider_catalog_key_if_current(provider, &key.id, |latest_key| {
                        aether_admin::provider::pool::admin_pool_key_account_quota_exhausted(
                            latest_key,
                            provider.provider_type.as_str(),
                        )
                    })
                    .await?
            } else {
                false
            };
        if auto_removed_quota_exhausted {
            auto_removed_count += 1;
            status = "quota_exhausted".to_string();
        }
        let auto_removed = auto_removed_hard_banned || auto_removed_quota_exhausted;
        let refresh_fixed =
            status == "success" && had_oauth_refresh_issue && oauth_invalid_reason.is_none();
        if refresh_fixed {
            refresh_fixed_count += 1;
        }
        let refresh_failed_retained =
            status != "success" && oauth_invalid_reason.is_some() && !auto_removed;
        if refresh_failed_retained {
            refresh_failed_retained_count += 1;
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
        if let Some(status_code) = status_code.take() {
            if status_code != 200 {
                payload.insert("status_code".to_string(), json!(status_code));
            }
        }
        if let Some(metadata_update) = metadata_update
            .as_ref()
            .and_then(|value| value.get("codex"))
            .cloned()
        {
            payload.insert("metadata".to_string(), metadata_update);
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "codex",
            key.status_snapshot.as_ref(),
            metadata_update.as_ref(),
        ) {
            payload.insert("quota_snapshot".to_string(), quota_snapshot);
        }
        if auto_removed {
            payload.insert("auto_removed".to_string(), json!(true));
        }
        if auto_removed_hard_banned {
            payload.insert("auto_removed_hard_banned".to_string(), json!(true));
        }
        if auto_removed_quota_exhausted {
            payload.insert("auto_removed_quota_exhausted".to_string(), json!(true));
        }
        if refresh_fixed {
            payload.insert("refresh_fixed".to_string(), json!(true));
        }
        if refresh_failed_retained {
            payload.insert("refresh_failed_retained".to_string(), json!(true));
        }
        results.push(serde_json::Value::Object(payload));
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": results.len(),
        "results": results,
        "auto_removed": auto_removed_count,
        "refresh_fixed": refresh_fixed_count,
        "refresh_failed_retained": refresh_failed_retained_count,
        "auto_removed_hard_banned": auto_removed_hard_banned_count,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_reset_credit_detail_count_overrides_usage_count() {
        let mut metadata = json!({
            "reset_credits": {
                "available_count": 0,
                "detail_source": "wham_usage"
            }
        })
        .as_object()
        .cloned()
        .expect("metadata object");
        let detail = json!({
            "reset_credits": {
                "available_count": 2,
                "detail_source": "wham_readonly"
            }
        });

        merge_codex_reset_credit_detail_metadata(&mut metadata, &detail);

        assert_eq!(
            metadata
                .get("reset_credits")
                .and_then(Value::as_object)
                .and_then(|credits| credits.get("available_count")),
            Some(&json!(2u64))
        );
    }

    #[test]
    fn codex_reset_credit_detail_failure_records_attempt_time() {
        let mut metadata = Map::new();

        mark_codex_reset_credit_detail_failed(&mut metadata, 1_777_000_000, "request failed");

        assert_eq!(
            metadata
                .get("reset_credits")
                .and_then(Value::as_object)
                .and_then(|credits| credits.get("updated_at")),
            Some(&json!(1_777_000_000u64))
        );
    }
}
