mod invalid;
mod parse;
mod plan;

use self::invalid::{
    codex_build_invalid_state, codex_looks_like_token_invalidated,
    codex_looks_like_workspace_deactivated, codex_soft_request_failure_reason,
    codex_structured_invalid_reason,
};
use self::parse::{
    build_codex_quota_exhausted_fallback_metadata, parse_codex_usage_headers,
    parse_codex_wham_usage_response,
};
use self::plan::{build_codex_refresh_headers, execute_codex_quota_plan};
use super::shared::{
    build_quota_snapshot_payload, extract_execution_error_message,
    persist_provider_quota_refresh_state, provider_auto_remove_banned_keys,
    public_refresh_quota_probe_timeout, quota_refresh_success_invalid_state,
    should_auto_remove_structured_reason, ProviderQuotaExecutionOutcome,
};
use crate::admin_api::{
    classify_oauth_key_probe_status, OauthKeyProbeClassification, OauthKeyProbeOutcome,
};
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

pub(crate) async fn probe_codex_provider_key_with_classification(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: StoredProviderCatalogKey,
    proxy_override: Option<&ProxySnapshot>,
    probe_timeout: Duration,
    allow_auto_remove: bool,
) -> Result<OauthKeyProbeOutcome, GatewayError> {
    let transport = match state
        .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
        .await?
    {
        Some(transport) => transport,
        None => {
            return Ok(OauthKeyProbeOutcome::from_payload(
                OauthKeyProbeClassification::TransportError,
                false,
                false,
                false,
                json!({
                    "key_id": key.id.clone(),
                    "key_name": key.name.clone(),
                    "status": "error",
                    "message": "Provider transport snapshot unavailable",
                }),
            ));
        }
    };

    let resolved_oauth_auth =
        if provider_key_is_oauth_managed(&key, provider.provider_type.as_str()) {
            state.resolve_local_oauth_header_auth(&transport).await?
        } else {
            None
        };

    let headers = match build_codex_refresh_headers(&transport, resolved_oauth_auth) {
        Ok(headers) => headers,
        Err(message) => {
            return Ok(OauthKeyProbeOutcome::from_payload(
                OauthKeyProbeClassification::OAuthInvalid,
                false,
                false,
                false,
                json!({
                    "key_id": key.id.clone(),
                    "key_name": key.name.clone(),
                    "status": "error",
                    "message": message,
                }),
            ));
        }
    };

    let result =
        match execute_codex_quota_plan(state, &transport, headers, proxy_override, probe_timeout)
            .await?
        {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(detail) => {
                return Ok(OauthKeyProbeOutcome::from_payload(
                    OauthKeyProbeClassification::TransportError,
                    false,
                    false,
                    false,
                    json!({
                        "key_id": key.id.clone(),
                        "key_name": key.name.clone(),
                        "status": "error",
                        "message": format!("wham/usage 请求执行失败: {detail}"),
                        "status_code": 502,
                    }),
                ));
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
    let status_code = Some(result.status_code);

    if result.status_code == 200 {
        if let Some(body_json) = result
            .body
            .as_ref()
            .and_then(|body| body.json_body.as_ref())
        {
            if let Some(parsed) = parse_codex_wham_usage_response(body_json, now_unix_secs) {
                metadata_update = Some(json!({
                    "codex": merge_codex_quota_metadata(header_metadata.as_ref(), &parsed)
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
                let candidate_reason = if codex_looks_like_token_invalidated(err_msg.as_deref()) {
                    codex_structured_invalid_reason(403, err_msg.as_deref())
                } else {
                    codex_soft_request_failure_reason(403, err_msg.as_deref())
                };
                let (at, reason) = codex_build_invalid_state(&key, candidate_reason, now_unix_secs);
                oauth_invalid_at_unix_secs = at;
                oauth_invalid_reason = reason;
                status = "forbidden".to_string();
            }
            _ => {}
        }
    }

    let auto_remove_abnormal_keys =
        allow_auto_remove && provider_auto_remove_banned_keys(provider.config.as_ref());
    let auto_removed = auto_remove_abnormal_keys
        && should_auto_remove_structured_reason(oauth_invalid_reason.as_deref());
    let mut auto_removed_counted = false;
    if auto_removed {
        if state.delete_provider_catalog_key(&key.id).await? {
            auto_removed_counted = true;
        }
    } else if !persist_provider_quota_refresh_state(
        state,
        &key.id,
        metadata_update.as_ref(),
        oauth_invalid_at_unix_secs,
        oauth_invalid_reason.clone(),
        None,
    )
    .await?
    {
        return Ok(OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::ServerError,
            false,
            false,
            false,
            json!({
                "key_id": key.id.clone(),
                "key_name": key.name.clone(),
                "status": "error",
                "message": "Key 状态写入失败",
            }),
        ));
    }

    let success = status == "success";
    let classification = classify_oauth_key_probe_status(
        provider.provider_type.as_str(),
        status.as_str(),
        status_code,
        message.as_deref(),
        oauth_invalid_reason.as_deref(),
        success,
    );
    let mut payload = serde_json::Map::new();
    payload.insert("key_id".to_string(), json!(key.id.clone()));
    payload.insert("key_name".to_string(), json!(key.name.clone()));
    payload.insert("status".to_string(), json!(status));
    if let Some(message) = message {
        payload.insert("message".to_string(), json!(message));
    }
    if let Some(status_code) = status_code {
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

    Ok(OauthKeyProbeOutcome::from_payload(
        classification,
        success,
        auto_removed,
        auto_removed_counted,
        serde_json::Value::Object(payload),
    ))
}

pub(crate) async fn refresh_codex_provider_quota_locally(
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
    let probe_timeout = public_refresh_quota_probe_timeout(provider, proxy_override.as_ref());

    for key in keys {
        let outcome = probe_codex_provider_key_with_classification(
            state,
            provider,
            endpoint,
            key,
            proxy_override.as_ref(),
            probe_timeout,
            true,
        )
        .await?;
        if outcome.success() {
            success_count += 1;
        } else {
            failed_count += 1;
        }
        if outcome.auto_removed_counted() {
            auto_removed_count += 1;
        }
        results.push(outcome.into_payload());
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": results.len(),
        "results": results,
        "auto_removed": auto_removed_count,
    })))
}
