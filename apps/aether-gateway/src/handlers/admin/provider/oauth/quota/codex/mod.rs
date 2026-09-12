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
    build_quota_snapshot_payload, complete_codex_account_reset, extract_execution_error_message,
    oauth_refresh_auto_removed_result, persist_codex_provider_quota_refresh_state,
    persist_fenced_provider_quota_refresh_state, provider_auto_remove_banned_keys,
    provider_auto_remove_quota_exhausted_keys, quota_key_auto_removed,
    quota_refresh_success_invalid_state, reserve_codex_account_reset,
    should_auto_remove_oauth_invalid_key, CodexAccountResetCompleteResult,
    CodexAccountResetReserveResult, CodexAccountResetTerminal, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::state::ProviderTransportCredentialFence;
use crate::GatewayError;
use aether_admin::provider::redaction::admin_provider_metadata_bucket_safe_json;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyOAuthCredentialCasDelete,
    ProviderCatalogUpstreamMetadataNamespaceExpectation, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::http::StatusCode;
use serde_json::{json, Map, Value};

const CODEX_OAUTH_CREDENTIAL_STABILIZATION_ATTEMPTS: usize = 3;
const CODEX_RESET_QUOTA_RECONCILIATION_DELAYS_MS: [u64; 4] = [1_000, 2_000, 4_000, 8_000];

enum CodexOAuthRequestPreparation {
    Ready {
        transport: AdminGatewayProviderTransportSnapshot,
        auth: (String, String),
        credential_fence: ProviderTransportCredentialFence,
    },
    MissingAuth,
    Conflict,
}

async fn prepare_codex_oauth_request(
    state: &AdminAppState<'_>,
    initial_transport: &AdminGatewayProviderTransportSnapshot,
) -> Result<CodexOAuthRequestPreparation, GatewayError> {
    for _ in 0..CODEX_OAUTH_CREDENTIAL_STABILIZATION_ATTEMPTS {
        let Some(transport) = state
            .read_provider_transport_snapshot_uncached(
                &initial_transport.provider.id,
                &initial_transport.endpoint.id,
                &initial_transport.key.id,
            )
            .await?
        else {
            return Ok(CodexOAuthRequestPreparation::Conflict);
        };
        if !crate::state::provider_transport_context_allows_credential_rotation(
            initial_transport,
            &transport,
        ) {
            return Ok(CodexOAuthRequestPreparation::Conflict);
        }
        let Some(before_fence) = state
            .app()
            .capture_provider_transport_credential_fence(&transport)
            .await?
        else {
            continue;
        };

        let resolved_auth = state.resolve_local_oauth_header_auth(&transport).await?;
        let Some(current_transport) = state
            .read_provider_transport_snapshot_uncached(
                &initial_transport.provider.id,
                &initial_transport.endpoint.id,
                &initial_transport.key.id,
            )
            .await?
        else {
            return Ok(CodexOAuthRequestPreparation::Conflict);
        };
        if !crate::state::provider_transport_context_allows_credential_rotation(
            initial_transport,
            &current_transport,
        ) {
            return Ok(CodexOAuthRequestPreparation::Conflict);
        }
        let Some(after_fence) = state
            .app()
            .capture_provider_transport_credential_fence(&current_transport)
            .await?
        else {
            continue;
        };
        if before_fence != after_fence {
            continue;
        }

        return Ok(match resolved_auth {
            Some(auth) => CodexOAuthRequestPreparation::Ready {
                transport: current_transport,
                auth,
                credential_fence: after_fence,
            },
            None => CodexOAuthRequestPreparation::MissingAuth,
        });
    }

    Ok(CodexOAuthRequestPreparation::Conflict)
}

fn codex_reset_refresh_succeeded(payload: Option<&Value>, key_id: &str) -> bool {
    payload
        .and_then(|payload| payload.get("results"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find(|item| item.get("key_id").and_then(Value::as_str) == Some(key_id))
        .and_then(|item| item.get("status"))
        .and_then(Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("success"))
}

async fn codex_reset_fence_is_still_pending(
    state: &AdminAppState<'_>,
    key_id: &str,
    expected_credential: &ProviderTransportCredentialFence,
    reset_fence: &super::shared::CodexAccountResetFence,
) -> Result<bool, GatewayError> {
    let Some(key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    if key.encrypted_auth_config.as_deref()
        != Some(expected_credential.encrypted_auth_config.as_str())
        || key.encrypted_api_key != expected_credential.credential.encrypted_api_key
        || key.auth_type != expected_credential.credential.auth_type
        || key.provider_id != expected_credential.credential.provider_id
    {
        return Ok(false);
    }
    let provider_type_matches = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await?
        .into_iter()
        .next()
        .is_some_and(|provider| {
            provider.provider_type == expected_credential.credential.provider_type
        });
    if !provider_type_matches {
        return Ok(false);
    }

    let codex = key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("codex"))
        .and_then(Value::as_object);
    Ok(codex.is_some_and(|codex| {
        codex
            .get(aether_admin::provider::quota::CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY)
            .and_then(Value::as_str)
            == Some(reset_fence.id.as_str())
            && codex
                .get(aether_admin::provider::quota::CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY)
                .and_then(aether_admin::provider::quota::coerce_json_u64)
                == Some(reset_fence.generation)
            && codex
                .get(
                    aether_admin::provider::quota::CODEX_QUOTA_ACCOUNT_RESET_PENDING_GENERATION_KEY,
                )
                .and_then(aether_admin::provider::quota::coerce_json_u64)
                == Some(reset_fence.generation)
            && codex
                .get(aether_admin::provider::quota::CODEX_QUOTA_ACCOUNT_RESET_PENDING_KEY)
                .and_then(Value::as_bool)
                == Some(true)
    }))
}

async fn refresh_codex_quota_after_reset_until_settled(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: &StoredProviderCatalogKey,
    reset_fence: &super::shared::CodexAccountResetFence,
    expected_credential: &ProviderTransportCredentialFence,
) -> Result<Option<Value>, GatewayError> {
    let mut latest_payload = None;
    for attempt in 0..=CODEX_RESET_QUOTA_RECONCILIATION_DELAYS_MS.len() {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(
                CODEX_RESET_QUOTA_RECONCILIATION_DELAYS_MS[attempt - 1],
            ))
            .await;
        }
        if !codex_reset_fence_is_still_pending(state, &key.id, expected_credential, reset_fence)
            .await?
        {
            break;
        }

        let payload = refresh_codex_provider_quota_locally_with_reset_fence(
            state,
            provider,
            endpoint,
            vec![key.clone()],
            None,
            Some(reset_fence.id.as_str()),
            Some(reset_fence.generation),
            Some(expected_credential),
        )
        .await?;
        let refresh_succeeded = codex_reset_refresh_succeeded(payload.as_ref(), &key.id);
        latest_payload = payload;
        if !refresh_succeeded
            || !codex_reset_fence_is_still_pending(state, &key.id, expected_credential, reset_fence)
                .await?
        {
            break;
        }
    }
    Ok(latest_payload)
}

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

fn codex_quota_window_coverage(
    body_json: Option<&Value>,
) -> aether_admin::provider::quota::CodexQuotaWindowCoverage {
    let body = body_json.and_then(Value::as_object);
    let has_account_snapshot = body
        .and_then(|body| body.get("rate_limit"))
        .and_then(Value::as_object)
        .is_some();
    let has_spark_snapshot = body
        .and_then(|body| body.get("additional_rate_limits"))
        .and_then(Value::as_array)
        .is_some();

    match (has_account_snapshot, has_spark_snapshot) {
        (true, true) => aether_admin::provider::quota::CodexQuotaWindowCoverage::FullSnapshot,
        (true, false) => aether_admin::provider::quota::CodexQuotaWindowCoverage::AccountSnapshot,
        _ => aether_admin::provider::quota::CodexQuotaWindowCoverage::Patch,
    }
}

fn truncate_codex_reset_credit_detail_error(message: impl Into<String>) -> String {
    let message = message.into();
    let mut sanitized = message.replace('\n', " ");
    if sanitized.len() > 240 {
        let mut truncate_at = 240;
        while !sanitized.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }
        sanitized.truncate(truncate_at);
        sanitized.push('…');
    }
    sanitized
}

fn safe_codex_quota_refresh_error(error: &GatewayError) -> &'static str {
    if matches!(
        error,
        GatewayError::LocalExecutionPlanningTimeout { .. } | GatewayError::AdmissionTimeout { .. }
    ) {
        return "Quota refresh timed out";
    }

    let message = match error {
        GatewayError::UpstreamUnavailable { message, .. }
        | GatewayError::ControlUnavailable { message, .. }
        | GatewayError::Client { message, .. }
        | GatewayError::Internal(message) => message.as_str(),
        GatewayError::PlanUsageLimited(_)
        | GatewayError::LastActiveAdminUpdateDenied
        | GatewayError::LastActiveAdminDeleteDenied => return "Quota refresh failed",
        GatewayError::LocalExecutionPlanningTimeout { .. }
        | GatewayError::AdmissionTimeout { .. } => unreachable!(),
    };
    let lower = message.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        "Quota refresh timed out"
    } else if [
        "connection",
        "connect",
        "dns",
        "network",
        "proxy",
        "socket",
        "tls",
        "certificate",
        "transport",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        "Quota refresh connection failed"
    } else {
        "Quota refresh failed"
    }
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
            ProviderQuotaExecutionOutcome::Failure(_) => {
                mark_codex_reset_credit_detail_failed(
                    codex_metadata,
                    now_unix_secs,
                    "reset credit detail 请求执行失败".to_string(),
                );
                return Ok(());
            }
        };

    if result.status_code != 200 {
        mark_codex_reset_credit_detail_failed(
            codex_metadata,
            now_unix_secs,
            format!("reset credit detail 返回状态码 {}", result.status_code),
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

fn codex_reset_credit_outcome_allows_usage_drop(outcome: &str) -> bool {
    matches!(outcome, "reset" | "already_redeemed")
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

async fn finish_codex_reset_replay(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: &StoredProviderCatalogKey,
    credential: &ProviderTransportCredentialFence,
    terminal: CodexAccountResetTerminal,
) -> Result<(StatusCode, Value), GatewayError> {
    let mut refresh_status = "skipped".to_string();
    let mut refresh_error = None;
    let mut metadata = None;
    let mut quota_snapshot = None;
    if codex_reset_credit_outcome_allows_usage_drop(&terminal.outcome) {
        let fence = super::shared::CodexAccountResetFence {
            unix_ms: crate::clock::current_unix_ms(),
            id: format!("reset:{}", terminal.idempotency_key),
            generation: terminal.generation,
        };
        if codex_reset_fence_is_still_pending(state, &key.id, credential, &fence).await? {
            match refresh_codex_quota_after_reset_until_settled(
                state, provider, endpoint, key, &fence, credential,
            )
            .await
            {
                Ok(payload) => {
                    (refresh_status, refresh_error, metadata, quota_snapshot) =
                        codex_extract_refresh_result_fields(payload.as_ref(), &key.id);
                }
                Err(err) => {
                    refresh_status = "failed".to_string();
                    refresh_error = Some(safe_codex_quota_refresh_error(&err).to_string());
                }
            }
        }
    }
    let mut payload = Map::new();
    payload.insert("key_id".to_string(), json!(key.id));
    payload.insert(
        "status".to_string(),
        json!(codex_consume_success_status(&terminal.outcome)),
    );
    payload.insert("outcome".to_string(), json!(terminal.outcome));
    payload.insert(
        "idempotency_key".to_string(),
        json!(terminal.idempotency_key),
    );
    payload.insert("replay".to_string(), json!(true));
    payload.insert("refresh_status".to_string(), json!(refresh_status));
    if let Some(refresh_error) = refresh_error {
        payload.insert("refresh_error".to_string(), json!(refresh_error));
    }
    if let Some(metadata) = metadata {
        payload.insert(
            "metadata".to_string(),
            admin_provider_metadata_bucket_safe_json("codex", Some(&metadata)),
        );
    }
    if let Some(quota_snapshot) = quota_snapshot {
        payload.insert("quota_snapshot".to_string(), quota_snapshot);
    }
    Ok((StatusCode::OK, Value::Object(payload)))
}

pub(crate) async fn consume_codex_reset_credit_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: StoredProviderCatalogKey,
    idempotency_key: &str,
    expected_credential_generation: Option<&str>,
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
    if !is_oauth_managed {
        return Ok((
            StatusCode::BAD_REQUEST,
            json!({
                "key_id": key.id,
                "status": "error",
                "outcome": "error",
                "message": "Codex reset credit 仅支持 OAuth 托管账号",
            }),
        ));
    }
    let (transport, resolved_oauth_auth, reset_credential_fence) =
        match prepare_codex_oauth_request(state, &transport).await? {
            CodexOAuthRequestPreparation::Ready {
                transport,
                auth,
                credential_fence,
            } => (transport, Some(auth), credential_fence),
            CodexOAuthRequestPreparation::MissingAuth => {
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
            CodexOAuthRequestPreparation::Conflict => {
                return Ok((
                    StatusCode::CONFLICT,
                    json!({
                        "key_id": key.id,
                        "status": "error",
                        "outcome": "error",
                        "idempotency_key": idempotency_key,
                        "message": "Codex credential changed before reset credit could be consumed",
                    }),
                ));
            }
        };

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

    let reservation = match reserve_codex_account_reset(
        state,
        &key.id,
        reset_credential_fence.encrypted_auth_config.as_str(),
        &reset_credential_fence.credential,
        expected_credential_generation,
        idempotency_key,
    )
    .await?
    {
        Some(CodexAccountResetReserveResult::Reserved(reservation)) => reservation,
        Some(CodexAccountResetReserveResult::Replay(terminal)) => {
            return finish_codex_reset_replay(
                state,
                provider,
                endpoint,
                &key,
                &reset_credential_fence,
                terminal,
            )
            .await;
        }
        Some(CodexAccountResetReserveResult::LegacyReplay) => {
            return Ok((
                StatusCode::OK,
                json!({
                    "key_id": key.id,
                    "status": "success",
                    "outcome": "historical_replay",
                    "idempotency_key": idempotency_key,
                    "refresh_status": "skipped",
                }),
            ));
        }
        Some(CodexAccountResetReserveResult::Busy(active)) => {
            return Ok((
                StatusCode::CONFLICT,
                json!({
                    "key_id": key.id,
                    "status": "error",
                    "outcome": "busy",
                    "idempotency_key": idempotency_key,
                    "active_idempotency_key": active.idempotency_key,
                    "message": "Another Codex reset credit operation is unresolved",
                }),
            ));
        }
        Some(CodexAccountResetReserveResult::CredentialGenerationMismatch) => {
            return Ok((
                StatusCode::CONFLICT,
                json!({
                    "key_id": key.id,
                    "status": "error",
                    "outcome": "credential_changed",
                    "idempotency_key": idempotency_key,
                    "message": "Codex credential changed since this reset request was prepared",
                }),
            ));
        }
        None => {
            return Ok((
                StatusCode::CONFLICT,
                json!({
                    "key_id": key.id,
                    "status": "error",
                    "outcome": "error",
                    "idempotency_key": idempotency_key,
                    "message": "Codex reset reservation could not be persisted",
                }),
            ));
        }
    };

    let result =
        match execute_codex_reset_credit_plan(state, &transport, request_spec, None).await? {
            ProviderQuotaExecutionOutcome::Response(result) => result,
            ProviderQuotaExecutionOutcome::Failure(_) => {
                return Ok((
                    StatusCode::BAD_GATEWAY,
                    json!({
                        "key_id": key.id,
                        "status": "error",
                        "outcome": "error",
                        "message": "reset credit consume 请求执行失败",
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
    let known_terminal_outcome = matches!(
        outcome.as_str(),
        "reset" | "already_redeemed" | "nothing_to_reset" | "no_credit"
    );
    if !known_terminal_outcome {
        return Ok((
            StatusCode::BAD_GATEWAY,
            json!({
                "key_id": key.id,
                "status": "error",
                "outcome": "error",
                "idempotency_key": idempotency_key,
                "message": "reset credit consume outcome is ambiguous",
                "status_code": result.status_code,
            }),
        ));
    }

    let fence_unix_ms = result
        .response_observation
        .as_ref()
        .map(|observation| observation.response_headers_observed_at_unix_ms)
        .unwrap_or_else(crate::clock::current_unix_ms);
    let Some(completed) = complete_codex_account_reset(
        state,
        &key.id,
        reset_credential_fence.encrypted_auth_config.as_str(),
        &reset_credential_fence.credential,
        &reservation,
        &outcome,
        fence_unix_ms,
    )
    .await?
    else {
        return Ok((
            StatusCode::CONFLICT,
            json!({
                "key_id": key.id,
                "status": "error",
                "outcome": "error",
                "idempotency_key": idempotency_key,
                "message": "Codex reset completion could not be persisted",
            }),
        ));
    };

    let (effective_outcome, reset_fence) = match completed {
        CodexAccountResetCompleteResult::Activated(fence) => (outcome.clone(), Some(fence)),
        CodexAccountResetCompleteResult::Noop(terminal)
        | CodexAccountResetCompleteResult::Replay(terminal) => {
            let fence =
                codex_reset_credit_outcome_allows_usage_drop(&terminal.outcome).then(|| {
                    super::shared::CodexAccountResetFence {
                        unix_ms: fence_unix_ms,
                        id: format!("reset:{}", terminal.idempotency_key),
                        generation: terminal.generation,
                    }
                });
            (terminal.outcome, fence)
        }
    };

    let (refresh_status, refresh_error, metadata, quota_snapshot) = match reset_fence.as_ref() {
        Some(reset_fence) => {
            match refresh_codex_quota_after_reset_until_settled(
                state,
                provider,
                endpoint,
                &key,
                reset_fence,
                &reset_credential_fence,
            )
            .await
            {
                Ok(refresh_payload) => {
                    codex_extract_refresh_result_fields(refresh_payload.as_ref(), &key.id)
                }
                Err(err) => (
                    "failed".to_string(),
                    Some(safe_codex_quota_refresh_error(&err).to_string()),
                    None,
                    None,
                ),
            }
        }
        None => match refresh_codex_provider_quota_locally_with_reset_fence(
            state,
            provider,
            endpoint,
            vec![key.clone()],
            None,
            None,
            None,
            Some(&reset_credential_fence),
        )
        .await
        {
            Ok(refresh_payload) => {
                codex_extract_refresh_result_fields(refresh_payload.as_ref(), &key.id)
            }
            Err(err) => (
                "failed".to_string(),
                Some(safe_codex_quota_refresh_error(&err).to_string()),
                None,
                None,
            ),
        },
    };

    let mut payload = Map::new();
    payload.insert("key_id".to_string(), json!(key.id));
    payload.insert(
        "status".to_string(),
        json!(codex_consume_success_status(&effective_outcome)),
    );
    payload.insert("outcome".to_string(), json!(effective_outcome));
    payload.insert("idempotency_key".to_string(), json!(idempotency_key));
    payload.insert("refresh_status".to_string(), json!(refresh_status));
    if let Some(refresh_error) = refresh_error {
        payload.insert("refresh_error".to_string(), json!(refresh_error));
    }
    if let Some(metadata) = metadata {
        payload.insert(
            "metadata".to_string(),
            admin_provider_metadata_bucket_safe_json("codex", Some(&metadata)),
        );
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
    refresh_codex_provider_quota_locally_with_reset_fence(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
        None,
        None,
        None,
    )
    .await
}

async fn refresh_codex_provider_quota_locally_with_reset_fence(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
    account_reset_fence_id: Option<&str>,
    authoritative_reset_generation: Option<u64>,
    expected_reset_credential: Option<&crate::state::ProviderTransportCredentialFence>,
) -> Result<Option<serde_json::Value>, GatewayError> {
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
        let initial_transport = match state
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
        let (transport, resolved_oauth_auth, quota_credential_fence) = if is_oauth_managed {
            match prepare_codex_oauth_request(state, &initial_transport).await? {
                CodexOAuthRequestPreparation::Ready {
                    transport,
                    auth,
                    credential_fence,
                } => (transport, Some(auth), Some(credential_fence)),
                CodexOAuthRequestPreparation::MissingAuth => {
                    failed_count += 1;
                    results.push(json!({
                        "key_id": key.id,
                        "key_name": key.name,
                        "status": "error",
                        "message": "缺少 Codex OAuth 认证信息，请先重新授权/刷新 Token",
                    }));
                    continue;
                }
                CodexOAuthRequestPreparation::Conflict => {
                    if quota_key_auto_removed(state, &key.id).await? {
                        auto_removed_count += 1;
                        results.push(oauth_refresh_auto_removed_result(&key));
                    } else {
                        failed_count += 1;
                        results.push(json!({
                            "key_id": key.id,
                            "key_name": key.name,
                            "status": "error",
                            "message": "OAuth credential changed before quota refresh",
                        }));
                    }
                    continue;
                }
            }
        } else {
            (initial_transport, None, None)
        };
        if let Some(expected_reset_credential) = expected_reset_credential {
            if quota_credential_fence.as_ref() != Some(expected_reset_credential) {
                failed_count += 1;
                results.push(json!({
                    "key_id": key.id,
                    "key_name": key.name,
                    "status": "error",
                    "message": "Codex credential changed after reset credit was consumed",
                }));
                continue;
            }
        }
        let transport_codex_metadata = transport
            .key
            .upstream_metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("codex"));
        let observed_reset_generation = authoritative_reset_generation.or_else(|| {
            Some(
                aether_admin::provider::quota::codex_quota_account_reset_generation(
                    transport_codex_metadata,
                ),
            )
        });
        let observed_credential_generation =
            aether_admin::provider::quota::codex_credential_generation(transport_codex_metadata)
                .map(ToOwned::to_owned);

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

        let quota_request_fallback_started_at_unix_ms = crate::clock::current_unix_ms();
        let quota_request_fallback_order_id = uuid::Uuid::now_v7().to_string();
        let result = match execute_codex_quota_plan(
            state,
            &transport,
            request_spec,
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
                    "message": "wham/usage 请求执行失败",
                    "status_code": 502,
                }));
                continue;
            }
        };
        let quota_response_fallback_observed_at_unix_ms = crate::clock::current_unix_ms();
        let quota_response_observation = result.response_observation.as_ref();
        let quota_request_started_at_unix_ms = quota_response_observation
            .map(|observation| observation.request_started_at_unix_ms)
            .unwrap_or(quota_request_fallback_started_at_unix_ms);
        let quota_response_observed_at_unix_ms = quota_response_observation
            .map(|observation| observation.response_headers_observed_at_unix_ms)
            .unwrap_or(quota_response_fallback_observed_at_unix_ms);
        let quota_request_order_id = quota_response_observation
            .map(|observation| observation.request_order_id.as_str())
            .unwrap_or(quota_request_fallback_order_id.as_str());
        let now_unix_secs = quota_response_observed_at_unix_ms / 1_000;

        let header_metadata = parse_codex_usage_headers(&result.headers, now_unix_secs);
        let mut metadata_update = header_metadata
            .as_ref()
            .map(|metadata| json!({ "codex": metadata }));
        let mut quota_window_coverage =
            aether_admin::provider::quota::CodexQuotaWindowCoverage::Patch;
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
                    quota_window_coverage = codex_quota_window_coverage(Some(body_json));
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
            message = Some(format!("wham/usage API 返回状态码 {}", result.status_code));

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
                        quota_window_coverage =
                            aether_admin::provider::quota::CodexQuotaWindowCoverage::Patch;
                        let mut codex_meta = metadata_update
                            .as_ref()
                            .and_then(|value| value.get("codex"))
                            .and_then(serde_json::Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        codex_meta.insert("updated_at".to_string(), json!(now_unix_secs));
                        codex_meta.insert("account_disabled".to_string(), json!(true));
                        codex_meta.insert("reason".to_string(), json!("deactivated_workspace"));
                        codex_meta.insert("message".to_string(), json!("deactivated_workspace"));
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
                        quota_window_coverage =
                            aether_admin::provider::quota::CodexQuotaWindowCoverage::Patch;
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

        let persisted = if let Some(expected_credential) = quota_credential_fence.as_ref() {
            persist_fenced_provider_quota_refresh_state(
                state,
                &key.id,
                expected_credential.encrypted_auth_config.as_str(),
                metadata_update.as_ref(),
                oauth_invalid_at_unix_secs,
                oauth_invalid_reason.clone(),
                aether_admin::provider::quota::CodexQuotaMergeContext {
                    observed_at_unix_secs: now_unix_secs,
                    request_started_at_unix_ms: Some(quota_request_started_at_unix_ms),
                    request_order_id: Some(quota_request_order_id),
                    observed_reset_generation,
                    authoritative_reset_generation,
                    observed_credential_generation: observed_credential_generation.as_deref(),
                    account_reset_fence_id,
                    coverage: quota_window_coverage,
                },
                &expected_credential.credential,
            )
            .await?
        } else {
            persist_codex_provider_quota_refresh_state(
                state,
                &key.id,
                metadata_update.as_ref(),
                aether_admin::provider::quota::CodexQuotaMergeContext {
                    observed_at_unix_secs: now_unix_secs,
                    request_started_at_unix_ms: Some(quota_request_started_at_unix_ms),
                    request_order_id: Some(quota_request_order_id),
                    observed_reset_generation,
                    authoritative_reset_generation,
                    observed_credential_generation: observed_credential_generation.as_deref(),
                    account_reset_fence_id,
                    coverage: quota_window_coverage,
                },
            )
            .await?
        };
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
        let persisted_key = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key.id))
            .await?
            .into_iter()
            .next();
        let persisted_codex_metadata = persisted_key
            .as_ref()
            .and_then(|key| key.upstream_metadata.as_ref())
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .cloned();
        if let Some(codex_metadata) = persisted_codex_metadata.as_ref() {
            metadata_update = Some(json!({"codex": codex_metadata}));
        }
        let persisted_codex_object = persisted_codex_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object);
        let request_owns_persisted_oauth_state = quota_credential_fence.is_none()
            || (persisted_codex_object
                .and_then(|codex| codex.get("oauth_state_request_started_at_unix_ms"))
                .and_then(aether_admin::provider::quota::coerce_json_u64)
                == Some(quota_request_started_at_unix_ms)
                && persisted_codex_object
                    .and_then(|codex| codex.get("oauth_state_request_id"))
                    .and_then(serde_json::Value::as_str)
                    == Some(quota_request_order_id));
        let credential_cas_delete = quota_credential_fence.as_ref().map(|credential_fence| {
            ProviderCatalogKeyOAuthCredentialCasDelete {
                key_id: key.id.clone(),
                expected_encrypted_auth_config: Some(
                    credential_fence.encrypted_auth_config.clone(),
                ),
                expected_credential: credential_fence.credential.clone(),
                expected_upstream_metadata_namespace: Some(
                    ProviderCatalogUpstreamMetadataNamespaceExpectation {
                        namespace: "codex".to_string(),
                        expected_value: persisted_codex_metadata.clone(),
                    },
                ),
            }
        });
        let should_auto_remove_hard_banned = request_owns_persisted_oauth_state
            && provider_auto_remove_banned_keys(provider.config.as_ref())
            && should_auto_remove_oauth_invalid_key(
                persisted_key.as_ref().unwrap_or(&key),
                persisted_key
                    .as_ref()
                    .and_then(|key| key.oauth_invalid_reason.as_deref()),
                matches!(status_code, Some(401 | 403)),
                now_unix_secs,
            );
        let auto_removed_hard_banned = if should_auto_remove_hard_banned {
            match credential_cas_delete.as_ref() {
                Some(delete) => {
                    state
                        .compare_and_delete_provider_catalog_key_oauth_credential(delete)
                        .await?
                }
                None => false,
            }
        } else {
            false
        };
        if auto_removed_hard_banned {
            auto_removed_count += 1;
            auto_removed_hard_banned_count += 1;
        }
        let auto_removed_quota_exhausted = if !auto_removed_hard_banned
            && request_owns_persisted_oauth_state
            && status == "quota_exhausted"
            && provider_auto_remove_quota_exhausted_keys(provider.config.as_ref())
        {
            match credential_cas_delete.as_ref() {
                Some(delete) => {
                    state
                        .compare_and_delete_provider_catalog_key_oauth_credential(delete)
                        .await?
                }
                None => false,
            }
        } else {
            false
        };
        if auto_removed_quota_exhausted {
            auto_removed_count += 1;
            status = "quota_exhausted".to_string();
        }
        let auto_removed = auto_removed_hard_banned || auto_removed_quota_exhausted;
        if auto_removed {
            let deleted_key_ids = [key.id.clone()];
            state
                .cleanup_deleted_provider_catalog_refs(&provider.id, false, &[], &deleted_key_ids)
                .await?;
        }
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
        {
            payload.insert(
                "metadata".to_string(),
                admin_provider_metadata_bucket_safe_json("codex", Some(metadata_update)),
            );
        }
        if let Some(quota_snapshot) = build_quota_snapshot_payload(
            "codex",
            persisted_key
                .as_ref()
                .and_then(|key| key.status_snapshot.as_ref()),
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
    fn codex_reset_credit_only_allows_usage_drop_after_confirmed_redemption() {
        assert!(codex_reset_credit_outcome_allows_usage_drop("reset"));
        assert!(codex_reset_credit_outcome_allows_usage_drop(
            "already_redeemed"
        ));
        assert!(!codex_reset_credit_outcome_allows_usage_drop(
            "nothing_to_reset"
        ));
        assert!(!codex_reset_credit_outcome_allows_usage_drop("no_credit"));
        assert!(!codex_reset_credit_outcome_allows_usage_drop("unknown"));
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

    #[test]
    fn codex_reset_credit_detail_truncation_preserves_utf8_boundaries() {
        let detail = format!("{}密钥", "a".repeat(239));

        let truncated = truncate_codex_reset_credit_detail_error(detail);

        assert_eq!(truncated, format!("{}…", "a".repeat(239)));
    }

    #[test]
    fn codex_quota_refresh_error_projection_discards_internal_details() {
        let secrets = [
            "https://user:password@internal.test/v1/quota?q=secret",
            "Authorization: Bearer upstream-secret",
            "user:password",
            "upstream-secret",
        ];
        let connection_error = GatewayError::Internal(format!(
            "connection failed for {}; {}",
            secrets[0], secrets[1]
        ));
        let generic_error = GatewayError::Internal(format!(
            "repository failure while processing {}; {}",
            secrets[0], secrets[1]
        ));
        let timeout_error = GatewayError::LocalExecutionPlanningTimeout {
            trace_id: secrets[1].to_string(),
            phase: "quota_refresh",
            timeout_ms: 5_000,
        };

        let projected = [
            safe_codex_quota_refresh_error(&connection_error),
            safe_codex_quota_refresh_error(&generic_error),
            safe_codex_quota_refresh_error(&timeout_error),
        ];

        assert_eq!(projected[0], "Quota refresh connection failed");
        assert_eq!(projected[1], "Quota refresh failed");
        assert_eq!(projected[2], "Quota refresh timed out");
        for safe_error in projected {
            for secret in secrets {
                assert!(!safe_error.contains(secret));
            }
        }
    }

    #[test]
    fn codex_quota_coverage_only_replaces_observed_window_families() {
        assert_eq!(
            codex_quota_window_coverage(Some(&json!({"credits":{"balance":5}}))),
            aether_admin::provider::quota::CodexQuotaWindowCoverage::Patch
        );
        assert_eq!(
            codex_quota_window_coverage(None),
            aether_admin::provider::quota::CodexQuotaWindowCoverage::Patch
        );
        assert_eq!(
            codex_quota_window_coverage(Some(&json!({
                "rate_limit":{"primary_window":{}}
            }))),
            aether_admin::provider::quota::CodexQuotaWindowCoverage::AccountSnapshot
        );
        assert_eq!(
            codex_quota_window_coverage(Some(&json!({
                "rate_limit":{"primary_window":{}},
                "additional_rate_limits":[]
            }))),
            aether_admin::provider::quota::CodexQuotaWindowCoverage::FullSnapshot
        );
    }
}
