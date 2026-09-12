use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
};
use crate::handlers::admin::request::{
    AdminAppState, AdminGatewayProviderTransportSnapshot, AdminLocalOAuthRefreshError,
};
use crate::handlers::shared::{
    sync_provider_key_oauth_status_snapshot, sync_provider_key_quota_status_snapshot,
};
use crate::state::ProviderTransportCredentialFence;
use crate::GatewayError;
use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_admin::provider::redaction::admin_provider_upstream_metadata_safe_json;
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTimeouts, ProxySnapshot, RequestBody,
    ResolvedTransportProfile, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyOAuthCredentialFence, ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ProviderCatalogKeyRuntimeMetadataUpdate, ProviderCatalogKeyStatusSnapshotUpdate,
    ProviderCatalogUpstreamMetadataNamespaceExpectation, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey,
};
use aether_provider_pool::{ProviderPoolQuotaRequestSpec, ProviderPoolService};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

const PROVIDER_QUOTA_DEFAULT_TIMEOUT_MS: u64 = 30_000;
const PROVIDER_QUOTA_PROXY_TIMEOUT_MS: u64 = 60_000;
const CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS: usize = 16;
const CODEX_RESET_HISTORY_LIMIT: usize = 64;
const CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY: &str =
    admin_provider_quota_pure::CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY;
const CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY: &str =
    admin_provider_quota_pure::CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAccountResetFence {
    pub unix_ms: u64,
    pub id: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexAccountResetFenceInstall {
    Owned(CodexAccountResetFence),
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAccountResetReservation {
    pub idempotency_key: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAccountResetTerminal {
    pub idempotency_key: String,
    pub generation: u64,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexAccountResetReserveResult {
    Reserved(CodexAccountResetReservation),
    Replay(CodexAccountResetTerminal),
    LegacyReplay,
    Busy(CodexAccountResetReservation),
    CredentialGenerationMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexAccountResetCompleteResult {
    Activated(CodexAccountResetFence),
    Noop(CodexAccountResetTerminal),
    Replay(CodexAccountResetTerminal),
}

pub(super) enum ProviderQuotaExecutionOutcome {
    Response(ExecutionResult),
    Failure(String),
}

pub(super) fn default_provider_quota_execution_timeouts(
    proxy: Option<&ProxySnapshot>,
) -> ExecutionTimeouts {
    let timeout_ms = if proxy.is_some() {
        PROVIDER_QUOTA_PROXY_TIMEOUT_MS
    } else {
        PROVIDER_QUOTA_DEFAULT_TIMEOUT_MS
    };
    ExecutionTimeouts {
        connect_ms: Some(timeout_ms),
        read_ms: Some(timeout_ms),
        write_ms: Some(timeout_ms),
        pool_ms: Some(timeout_ms),
        total_ms: Some(timeout_ms),
        ..ExecutionTimeouts::default()
    }
}

pub(super) fn resolve_provider_quota_execution_timeouts(
    configured: Option<ExecutionTimeouts>,
    proxy: Option<&ProxySnapshot>,
) -> ExecutionTimeouts {
    let defaults = default_provider_quota_execution_timeouts(proxy);
    let Some(mut timeouts) = configured else {
        return defaults;
    };
    timeouts.connect_ms = timeouts.connect_ms.or(defaults.connect_ms);
    timeouts.read_ms = timeouts.read_ms.or(defaults.read_ms);
    timeouts.write_ms = timeouts.write_ms.or(defaults.write_ms);
    timeouts.pool_ms = timeouts.pool_ms.or(defaults.pool_ms);
    timeouts.total_ms = timeouts.total_ms.or(defaults.total_ms);
    timeouts.first_byte_ms = timeouts.first_byte_ms.or(defaults.first_byte_ms);
    timeouts
}

pub(crate) fn provider_auto_remove_banned_keys(config: Option<&serde_json::Value>) -> bool {
    admin_provider_quota_pure::provider_auto_remove_banned_keys(config)
}

pub(crate) fn provider_auto_remove_quota_exhausted_keys(
    config: Option<&serde_json::Value>,
) -> bool {
    admin_provider_quota_pure::provider_auto_remove_quota_exhausted_keys(config)
}

pub(super) fn should_auto_remove_structured_reason(reason: Option<&str>) -> bool {
    admin_provider_quota_pure::should_auto_remove_structured_reason(reason)
}

pub(crate) fn should_auto_remove_oauth_invalid_key(
    key: &StoredProviderCatalogKey,
    candidate_reason: Option<&str>,
    access_token_invalid_proven: bool,
    now_unix_secs: u64,
) -> bool {
    admin_provider_quota_pure::should_auto_remove_oauth_invalid_key(
        key,
        candidate_reason,
        access_token_invalid_proven,
        now_unix_secs,
    )
}

pub(crate) async fn persist_quota_oauth_refresh_failure_state(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    err: &AdminLocalOAuthRefreshError,
) -> Result<bool, GatewayError> {
    let AdminLocalOAuthRefreshError::HttpStatus {
        status_code,
        body_excerpt,
        ..
    } = err
    else {
        return Ok(false);
    };
    if !matches!(*status_code, 400 | 401 | 403) {
        return Ok(false);
    }
    state
        .app()
        .persist_local_oauth_refresh_failure_state(transport, *status_code, body_excerpt, false)
        .await
}

pub(crate) async fn quota_key_auto_removed(
    state: &AdminAppState<'_>,
    key_id: &str,
) -> Result<bool, GatewayError> {
    if key_id.trim().is_empty() {
        return Ok(false);
    }
    Ok(state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .is_empty())
}

pub(crate) fn oauth_refresh_auto_removed_result(
    key: &StoredProviderCatalogKey,
) -> serde_json::Value {
    serde_json::json!({
        "key_id": key.id,
        "key_name": key.name,
        "status": "auto_removed",
        "message": "OAuth refresh 失败且凭证已不可用，已自动删除",
        "auto_removed": true,
    })
}

pub(crate) fn normalize_string_id_list(values: Option<Vec<String>>) -> Option<Vec<String>> {
    admin_provider_quota_pure::normalize_string_id_list(values)
}

pub(crate) fn provider_type_supports_quota_refresh(provider_type: &str) -> bool {
    ProviderPoolService::with_builtin_adapters().supports_quota_refresh(provider_type)
}

pub(crate) fn unsupported_provider_quota_refresh_message(provider_type: &str) -> String {
    ProviderPoolService::with_builtin_adapters().quota_refresh_unsupported_message(provider_type)
}

pub(crate) fn provider_quota_refresh_endpoint_for_provider(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
    include_inactive: bool,
) -> Option<StoredProviderCatalogEndpoint> {
    ProviderPoolService::with_builtin_adapters().quota_refresh_endpoint_for_provider(
        provider_type,
        endpoints,
        include_inactive,
    )
}

pub(crate) fn provider_quota_refresh_missing_endpoint_message(provider_type: &str) -> String {
    ProviderPoolService::with_builtin_adapters()
        .quota_refresh_missing_endpoint_message(provider_type)
}

pub(super) fn coerce_json_u64(value: &serde_json::Value) -> Option<u64> {
    admin_provider_quota_pure::coerce_json_u64(value)
}

pub(super) fn coerce_json_f64(value: &serde_json::Value) -> Option<f64> {
    admin_provider_quota_pure::coerce_json_f64(value)
}

pub(super) fn coerce_json_bool(value: &serde_json::Value) -> Option<bool> {
    admin_provider_quota_pure::coerce_json_bool(value)
}

fn merge_upstream_metadata(
    current: Option<&serde_json::Value>,
    updates: &serde_json::Value,
) -> serde_json::Value {
    let mut merged = current
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(update_object) = updates.as_object() {
        for (key, value) in update_object {
            let mut next = value.clone();
            if let (Some(current_namespace), Some(next_namespace)) = (
                merged.get(key).and_then(serde_json::Value::as_object),
                next.as_object_mut(),
            ) {
                let mut combined = current_namespace.clone();
                combined.extend(next_namespace.clone());
                next = serde_json::Value::Object(combined);
            }
            merged.insert(key.clone(), next);
        }
    }
    serde_json::Value::Object(merged)
}

pub(super) fn extract_execution_error_message(result: &ExecutionResult) -> Option<String> {
    admin_provider_quota_pure::extract_execution_error_message(result)
}

pub(super) fn extract_execution_error_message_ref(result: &ExecutionResult) -> Option<&str> {
    if let Some(body_json) = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .and_then(serde_json::Value::as_object)
    {
        if let Some(message) = body_json
            .get("error")
            .and_then(serde_json::Value::as_object)
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            return Some(message);
        }
        if let Some(message) = body_json
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            return Some(message);
        }
    }

    result
        .error
        .as_ref()
        .map(|error| error.message.trim())
        .filter(|message| !message.is_empty())
}

fn extract_execution_error_detail(result: &ExecutionResult) -> Option<String> {
    admin_provider_quota_pure::extract_execution_error_detail(result)
}

pub(super) fn quota_refresh_success_invalid_state(
    key: &StoredProviderCatalogKey,
) -> (Option<u64>, Option<String>) {
    admin_provider_quota_pure::quota_refresh_success_invalid_state(key)
}

fn merge_codex_oauth_response_state(
    latest_key: &StoredProviderCatalogKey,
    incoming_invalid_at_unix_secs: Option<u64>,
    incoming_invalid_reason: Option<&str>,
    observed_at_unix_secs: u64,
) -> (Option<u64>, Option<String>) {
    match incoming_invalid_reason {
        Some(reason) => admin_provider_quota_pure::codex_build_invalid_state(
            latest_key,
            reason.to_string(),
            incoming_invalid_at_unix_secs.unwrap_or(observed_at_unix_secs),
        ),
        None => admin_provider_quota_pure::quota_refresh_success_invalid_state(latest_key),
    }
}

fn codex_reset_credential_matches(
    key: &StoredProviderCatalogKey,
    expected_encrypted_auth_config: &str,
    expected_credential: &ProviderCatalogKeyOAuthCredentialFence,
) -> bool {
    key.encrypted_auth_config.as_deref() == Some(expected_encrypted_auth_config)
        && key.encrypted_api_key == expected_credential.encrypted_api_key
        && key.auth_type == expected_credential.auth_type
        && key.provider_id == expected_credential.provider_id
}

fn codex_reset_reservation_from_object(
    codex: &serde_json::Map<String, serde_json::Value>,
) -> Option<CodexAccountResetReservation> {
    let reservation = codex
        .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_RESERVATION_KEY)?
        .as_object()?;
    let idempotency_key = reservation.get("idempotency_key")?.as_str()?.trim();
    let generation = reservation
        .get("generation")
        .and_then(admin_provider_quota_pure::coerce_json_u64)
        .filter(|generation| *generation > 0)?;
    (!idempotency_key.is_empty()).then(|| CodexAccountResetReservation {
        idempotency_key: idempotency_key.to_string(),
        generation,
    })
}

fn codex_reset_history_from_object(
    codex: &serde_json::Map<String, serde_json::Value>,
) -> Vec<CodexAccountResetTerminal> {
    codex
        .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_HISTORY_KEY)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let object = value.as_object()?;
            let idempotency_key = object.get("idempotency_key")?.as_str()?.trim();
            let generation = object
                .get("generation")
                .and_then(admin_provider_quota_pure::coerce_json_u64)?;
            let outcome = object.get("outcome")?.as_str()?.trim();
            (!idempotency_key.is_empty() && !outcome.is_empty()).then(|| {
                CodexAccountResetTerminal {
                    idempotency_key: idempotency_key.to_string(),
                    generation,
                    outcome: outcome.to_string(),
                }
            })
        })
        .collect()
}

fn codex_reset_write_bounded_history(
    codex: &mut serde_json::Map<String, serde_json::Value>,
    terminal: &CodexAccountResetTerminal,
) {
    let mut history = codex_reset_history_from_object(codex);
    history.retain(|entry| entry.idempotency_key != terminal.idempotency_key);
    history.push(terminal.clone());
    if history.len() > CODEX_RESET_HISTORY_LIMIT {
        history.drain(..history.len() - CODEX_RESET_HISTORY_LIMIT);
    }
    codex.insert(
        admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_HISTORY_KEY.to_string(),
        serde_json::Value::Array(
            history
                .into_iter()
                .map(|entry| {
                    serde_json::json!({
                        "idempotency_key": entry.idempotency_key,
                        "generation": entry.generation,
                        "outcome": entry.outcome,
                    })
                })
                .collect(),
        ),
    );

    codex_reset_write_processed_id(codex, &terminal.idempotency_key);
}

fn codex_reset_write_processed_id(
    codex: &mut serde_json::Map<String, serde_json::Value>,
    idempotency_key: &str,
) -> bool {
    let mut processed_ids = codex
        .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_PROCESSED_IDS_KEY)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let already_processed = processed_ids.iter().any(|value| value == idempotency_key);
    processed_ids.retain(|value| value != idempotency_key);
    processed_ids.push(idempotency_key.to_string());
    codex.insert(
        admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_PROCESSED_IDS_KEY.to_string(),
        serde_json::Value::Array(
            processed_ids
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    already_processed
}

async fn persist_codex_reset_namespace(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    expected_codex: Option<serde_json::Value>,
    next_codex: serde_json::Value,
    expected_encrypted_auth_config: &str,
    expected_credential: &ProviderCatalogKeyOAuthCredentialFence,
    updated_at_unix_secs: u64,
) -> Result<bool, GatewayError> {
    state
        .app()
        .compare_and_update_provider_catalog_key_oauth_runtime_state(
            &ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
                key_id: key.id.clone(),
                expected_encrypted_auth_config: Some(expected_encrypted_auth_config.to_string()),
                expected_credential: Some(expected_credential.clone()),
                expected_upstream_metadata_namespace: Some(
                    ProviderCatalogUpstreamMetadataNamespaceExpectation {
                        namespace: "codex".to_string(),
                        expected_value: expected_codex,
                    },
                ),
                encrypted_auth_config: expected_encrypted_auth_config.to_string(),
                encrypted_api_key_update: None,
                expires_at_unix_secs_update: None,
                oauth_invalid_at_unix_secs: key.oauth_invalid_at_unix_secs,
                oauth_invalid_reason: key.oauth_invalid_reason.clone(),
                upstream_metadata_patch: Some(serde_json::json!({"codex": next_codex})),
                upstream_metadata_namespace_to_remove: None,
                status_snapshot_patch: serde_json::json!({}),
                reset_error_count: false,
                updated_at_unix_secs: Some(updated_at_unix_secs),
            },
        )
        .await
}

pub(crate) async fn reserve_codex_account_reset(
    state: &AdminAppState<'_>,
    key_id: &str,
    expected_encrypted_auth_config: &str,
    expected_credential: &ProviderCatalogKeyOAuthCredentialFence,
    expected_credential_generation: Option<&str>,
    idempotency_key: &str,
) -> Result<Option<CodexAccountResetReserveResult>, GatewayError> {
    let idempotency_key = idempotency_key.trim();
    if idempotency_key.is_empty() {
        return Ok(None);
    }
    for attempt in 0..CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
        let Some(key) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        if !codex_reset_credential_matches(
            &key,
            expected_encrypted_auth_config,
            expected_credential,
        ) {
            return Ok(None);
        }
        let expected_codex = key
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .cloned();
        if !admin_provider_quota_pure::codex_credential_generation_matches(
            expected_codex.as_ref(),
            expected_credential_generation,
        ) {
            return Ok(Some(
                CodexAccountResetReserveResult::CredentialGenerationMismatch,
            ));
        }
        let mut codex = expected_codex
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(terminal) = codex_reset_history_from_object(&codex)
            .into_iter()
            .find(|entry| entry.idempotency_key == idempotency_key)
        {
            return Ok(Some(CodexAccountResetReserveResult::Replay(terminal)));
        }
        if codex
            .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_PROCESSED_IDS_KEY)
            .and_then(serde_json::Value::as_array)
            .is_some_and(|ids| {
                ids.iter()
                    .any(|value| value.as_str() == Some(idempotency_key))
            })
        {
            let active_generation = codex
                .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY)
                .and_then(admin_provider_quota_pure::coerce_json_u64)
                .unwrap_or(0);
            let active_fence_matches = codex
                .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY)
                .and_then(serde_json::Value::as_str)
                == Some(format!("reset:{idempotency_key}").as_str());
            if active_generation > 0 && active_fence_matches {
                return Ok(Some(CodexAccountResetReserveResult::Replay(
                    CodexAccountResetTerminal {
                        idempotency_key: idempotency_key.to_string(),
                        generation: active_generation,
                        outcome: "already_redeemed".to_string(),
                    },
                )));
            }
            return Ok(Some(CodexAccountResetReserveResult::LegacyReplay));
        }
        if let Some(reservation) = codex_reset_reservation_from_object(&codex) {
            return Ok(Some(if reservation.idempotency_key == idempotency_key {
                CodexAccountResetReserveResult::Reserved(reservation)
            } else {
                CodexAccountResetReserveResult::Busy(reservation)
            }));
        }
        let active_generation = codex
            .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY)
            .and_then(admin_provider_quota_pure::coerce_json_u64)
            .unwrap_or(0);
        let sequence = codex
            .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_SEQUENCE_KEY)
            .and_then(admin_provider_quota_pure::coerce_json_u64)
            .unwrap_or(active_generation)
            .max(active_generation);
        let Some(generation) = sequence.checked_add(1) else {
            return Err(GatewayError::Internal(
                "Codex reset generation exhausted".to_string(),
            ));
        };
        let reservation = CodexAccountResetReservation {
            idempotency_key: idempotency_key.to_string(),
            generation,
        };
        codex.insert(
            admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_SEQUENCE_KEY.to_string(),
            serde_json::json!(generation),
        );
        codex.insert(
            admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_RESERVATION_KEY.to_string(),
            serde_json::json!({
                "idempotency_key": reservation.idempotency_key,
                "generation": reservation.generation,
            }),
        );
        if persist_codex_reset_namespace(
            state,
            &key,
            expected_codex,
            serde_json::Value::Object(codex),
            expected_encrypted_auth_config,
            expected_credential,
            crate::clock::current_unix_secs(),
        )
        .await?
        {
            return Ok(Some(CodexAccountResetReserveResult::Reserved(reservation)));
        }
        if attempt + 1 < CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
            tokio::task::yield_now().await;
        }
    }
    Ok(None)
}

fn record_locally_consumed_codex_reset_credit(
    codex: &mut serde_json::Map<String, serde_json::Value>,
    observed_at_unix_secs: u64,
) {
    let Some(reset_credits) = codex
        .get_mut("reset_credits")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let Some(available_count) = reset_credits
        .get("available_count")
        .and_then(admin_provider_quota_pure::coerce_json_u64)
    else {
        return;
    };

    reset_credits.insert(
        "available_count".to_string(),
        serde_json::json!(available_count.saturating_sub(1)),
    );
    reset_credits.insert(
        "updated_at".to_string(),
        serde_json::json!(observed_at_unix_secs),
    );
    reset_credits.insert(
        "detail_source".to_string(),
        serde_json::json!("local_consume"),
    );
    reset_credits.insert(
        "detail_status".to_string(),
        serde_json::json!("pending_refresh"),
    );
    reset_credits.remove("detail_error");
    if let Some(credits) = reset_credits
        .get_mut("credits")
        .and_then(serde_json::Value::as_array_mut)
    {
        if !credits.is_empty() {
            credits.remove(0);
        }
    }
}

pub(crate) async fn complete_codex_account_reset(
    state: &AdminAppState<'_>,
    key_id: &str,
    expected_encrypted_auth_config: &str,
    expected_credential: &ProviderCatalogKeyOAuthCredentialFence,
    reservation: &CodexAccountResetReservation,
    outcome: &str,
    fence_unix_ms: u64,
) -> Result<Option<CodexAccountResetCompleteResult>, GatewayError> {
    let outcome = outcome.trim();
    let activates = matches!(outcome, "reset" | "already_redeemed");
    let noop = matches!(outcome, "nothing_to_reset" | "no_credit");
    if (!activates && !noop) || fence_unix_ms == 0 {
        return Ok(None);
    }
    for attempt in 0..CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
        let Some(key) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        if !codex_reset_credential_matches(
            &key,
            expected_encrypted_auth_config,
            expected_credential,
        ) {
            return Ok(None);
        }
        let expected_codex = key
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .cloned();
        let mut codex = expected_codex
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        let existing_terminal = codex_reset_history_from_object(&codex)
            .into_iter()
            .find(|entry| entry.idempotency_key == reservation.idempotency_key);
        let upgrades_noop = existing_terminal.as_ref().is_some_and(|terminal| {
            terminal.generation == reservation.generation
                && !matches!(terminal.outcome.as_str(), "reset" | "already_redeemed")
                && activates
        });
        if let Some(terminal) = existing_terminal.as_ref().filter(|_| !upgrades_noop) {
            return Ok(Some(CodexAccountResetCompleteResult::Replay(
                terminal.clone(),
            )));
        }
        if !upgrades_noop
            && codex_reset_reservation_from_object(&codex).as_ref() != Some(reservation)
        {
            return Ok(None);
        }
        let terminal = CodexAccountResetTerminal {
            idempotency_key: reservation.idempotency_key.clone(),
            generation: reservation.generation,
            outcome: outcome.to_string(),
        };
        if outcome == "reset" {
            record_locally_consumed_codex_reset_credit(&mut codex, fence_unix_ms / 1_000);
        }
        codex_reset_write_bounded_history(&mut codex, &terminal);
        if codex_reset_reservation_from_object(&codex).as_ref() == Some(reservation) {
            codex.remove(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_RESERVATION_KEY);
        }
        let active_generation = codex
            .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY)
            .and_then(admin_provider_quota_pure::coerce_json_u64)
            .unwrap_or(0);
        let completed = if activates && active_generation <= reservation.generation {
            let fence = CodexAccountResetFence {
                unix_ms: fence_unix_ms,
                id: format!("reset:{}", reservation.idempotency_key),
                generation: reservation.generation,
            };
            codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY.to_string(),
                serde_json::json!(reservation.generation),
            );
            codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_PENDING_GENERATION_KEY
                    .to_string(),
                serde_json::json!(reservation.generation),
            );
            codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_PENDING_KEY.to_string(),
                serde_json::json!(true),
            );
            codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_UNIX_MS_KEY.to_string(),
                serde_json::json!(fence_unix_ms),
            );
            codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY.to_string(),
                serde_json::json!(fence.id),
            );
            CodexAccountResetCompleteResult::Activated(fence)
        } else if activates {
            CodexAccountResetCompleteResult::Replay(terminal)
        } else {
            CodexAccountResetCompleteResult::Noop(terminal)
        };
        if persist_codex_reset_namespace(
            state,
            &key,
            expected_codex,
            serde_json::Value::Object(codex),
            expected_encrypted_auth_config,
            expected_credential,
            fence_unix_ms / 1_000,
        )
        .await?
        {
            return Ok(Some(completed));
        }
        if attempt + 1 < CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
            tokio::task::yield_now().await;
        }
    }
    Ok(None)
}

pub(crate) async fn persist_codex_account_reset_fence(
    state: &AdminAppState<'_>,
    key_id: &str,
    expected_encrypted_auth_config: Option<&str>,
    expected_credential: Option<&ProviderCatalogKeyOAuthCredentialFence>,
    fence_unix_ms: u64,
    fence_id: &str,
    idempotency_key: &str,
) -> Result<Option<CodexAccountResetFenceInstall>, GatewayError> {
    if expected_encrypted_auth_config.is_some() != expected_credential.is_some() {
        return Err(GatewayError::Internal(
            "Codex reset credential fence must include auth_config and credential identity"
                .to_string(),
        ));
    }
    let fence_id = fence_id.trim();
    let idempotency_key = idempotency_key.trim();
    if fence_unix_ms == 0 || fence_id.is_empty() || idempotency_key.is_empty() {
        return Ok(None);
    }

    for attempt in 0..CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
        let Some(latest_key) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let expected_codex = latest_key
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .cloned();
        let mut next_codex = expected_codex
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        let already_processed = codex_reset_write_processed_id(&mut next_codex, idempotency_key);
        let stored_fence = next_codex
            .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_UNIX_MS_KEY)
            .and_then(admin_provider_quota_pure::coerce_json_u64)
            .zip(
                next_codex
                    .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
            );
        let owns_stored_fence = already_processed
            && stored_fence
                .as_ref()
                .is_some_and(|(_, stored_id)| stored_id == fence_id);
        let installs_fence = !already_processed
            && stored_fence
                .as_ref()
                .is_none_or(|(stored_unix_ms, stored_id)| {
                    (fence_unix_ms, fence_id) > (*stored_unix_ms, stored_id.as_str())
                });
        if installs_fence {
            next_codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_UNIX_MS_KEY.to_string(),
                serde_json::json!(fence_unix_ms),
            );
            next_codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY.to_string(),
                serde_json::json!(fence_id),
            );
            next_codex.insert(
                admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_PENDING_KEY.to_string(),
                serde_json::json!(true),
            );
        }
        let effective_fence = CodexAccountResetFence {
            unix_ms: next_codex
                .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_UNIX_MS_KEY)
                .and_then(admin_provider_quota_pure::coerce_json_u64)
                .unwrap_or(fence_unix_ms),
            id: next_codex
                .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY)
                .and_then(serde_json::Value::as_str)
                .unwrap_or(fence_id)
                .to_string(),
            generation: next_codex
                .get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_GENERATION_KEY)
                .and_then(admin_provider_quota_pure::coerce_json_u64)
                .unwrap_or(0),
        };
        let install = if installs_fence || owns_stored_fence {
            CodexAccountResetFenceInstall::Owned(effective_fence)
        } else {
            CodexAccountResetFenceInstall::Superseded
        };
        if already_processed {
            let credential_matches = match (expected_encrypted_auth_config, expected_credential) {
                (Some(expected_auth), Some(expected_credential)) => {
                    latest_key.encrypted_auth_config.as_deref() == Some(expected_auth)
                        && latest_key.encrypted_api_key == expected_credential.encrypted_api_key
                        && latest_key.auth_type == expected_credential.auth_type
                        && latest_key.provider_id == expected_credential.provider_id
                }
                (Some(expected_auth), None) => {
                    latest_key.encrypted_auth_config.as_deref() == Some(expected_auth)
                }
                (None, _) => true,
            };
            return Ok(credential_matches.then_some(install));
        }
        let next_codex = serde_json::Value::Object(next_codex);

        let persisted = if let Some(expected_encrypted_auth_config) = expected_encrypted_auth_config
        {
            if latest_key.encrypted_auth_config.as_deref() != Some(expected_encrypted_auth_config) {
                return Ok(None);
            }
            state
                .app()
                .compare_and_update_provider_catalog_key_oauth_runtime_state(
                    &ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
                        key_id: key_id.to_string(),
                        expected_encrypted_auth_config: Some(
                            expected_encrypted_auth_config.to_string(),
                        ),
                        expected_credential: expected_credential.cloned(),
                        expected_upstream_metadata_namespace: Some(
                            ProviderCatalogUpstreamMetadataNamespaceExpectation {
                                namespace: "codex".to_string(),
                                expected_value: expected_codex,
                            },
                        ),
                        encrypted_auth_config: expected_encrypted_auth_config.to_string(),
                        encrypted_api_key_update: None,
                        expires_at_unix_secs_update: None,
                        oauth_invalid_at_unix_secs: latest_key.oauth_invalid_at_unix_secs,
                        oauth_invalid_reason: latest_key.oauth_invalid_reason.clone(),
                        upstream_metadata_patch: Some(serde_json::json!({
                            "codex": next_codex
                        })),
                        upstream_metadata_namespace_to_remove: None,
                        status_snapshot_patch: serde_json::json!({}),
                        reset_error_count: false,
                        updated_at_unix_secs: Some(fence_unix_ms / 1_000),
                    },
                )
                .await?
        } else {
            state
                .app()
                .update_provider_catalog_key_runtime_metadata(
                    &ProviderCatalogKeyRuntimeMetadataUpdate {
                        key_id: key_id.to_string(),
                        namespace: "codex".to_string(),
                        expected_upstream_metadata_value: expected_codex,
                        upstream_metadata_value: next_codex,
                        status_snapshot_patch: serde_json::json!({}),
                        updated_at_unix_secs: Some(fence_unix_ms / 1_000),
                    },
                )
                .await?
        };
        if persisted {
            return Ok(Some(install));
        }
        if attempt + 1 < CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
            let backoff_us = 50_u64.saturating_mul((attempt + 1) as u64).min(1_000);
            tokio::time::sleep(std::time::Duration::from_micros(backoff_us)).await;
        }
    }
    Ok(None)
}

pub(super) fn coerce_json_string(value: Option<&serde_json::Value>) -> Option<String> {
    admin_provider_quota_pure::coerce_json_string(value)
}

pub(super) fn build_quota_snapshot_payload(
    provider_type: &str,
    current_status_snapshot: Option<&serde_json::Value>,
    metadata_update: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let updated_snapshot = sync_provider_key_quota_status_snapshot(
        current_status_snapshot,
        provider_type,
        metadata_update,
        "refresh_api",
    )?;
    updated_snapshot.get("quota").cloned()
}

pub(super) fn build_provider_quota_execution_plan(
    transport: &AdminGatewayProviderTransportSnapshot,
    spec: ProviderPoolQuotaRequestSpec,
    proxy: Option<ProxySnapshot>,
    transport_profile: Option<ResolvedTransportProfile>,
    timeouts: Option<ExecutionTimeouts>,
) -> ExecutionPlan {
    let ProviderPoolQuotaRequestSpec {
        request_id,
        provider_name,
        quota_kind: _,
        method,
        url,
        mut headers,
        content_type,
        json_body,
        client_api_format,
        provider_api_format,
        model_name,
    } = spec;
    force_provider_quota_redirects_disabled(&mut headers);
    let body = json_body
        .map(RequestBody::from_json)
        .unwrap_or(RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        });
    ExecutionPlan {
        request_id,
        candidate_id: None,
        provider_name: Some(provider_name),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method,
        url,
        headers,
        content_type,
        content_encoding: None,
        body,
        stream: false,
        client_api_format,
        provider_api_format,
        model_name,
        proxy,
        transport_profile,
        timeouts,
    }
}

fn force_provider_quota_redirects_disabled(
    headers: &mut std::collections::BTreeMap<String, String>,
) {
    headers.retain(|name, _| !name.eq_ignore_ascii_case(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER));
    headers.insert(
        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string(),
        "false".to_string(),
    );
}

fn codex_reset_refresh_is_superseded(
    current: Option<&serde_json::Value>,
    context: admin_provider_quota_pure::CodexQuotaMergeContext<'_>,
) -> bool {
    let Some(incoming_fence_id) = context
        .account_reset_fence_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    current
        .and_then(serde_json::Value::as_object)
        .and_then(|codex| {
            codex.get(admin_provider_quota_pure::CODEX_QUOTA_ACCOUNT_RESET_FENCE_ID_KEY)
        })
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|stored_fence_id| stored_fence_id != incoming_fence_id)
}

pub(crate) async fn persist_provider_quota_refresh_state(
    state: &AdminAppState<'_>,
    key_id: &str,
    metadata_update: Option<&serde_json::Value>,
    oauth_invalid_at_unix_secs: Option<u64>,
    oauth_invalid_reason: Option<String>,
    encrypted_auth_config: Option<String>,
) -> Result<bool, GatewayError> {
    persist_provider_quota_refresh_state_after_read(
        state,
        key_id,
        metadata_update,
        oauth_invalid_at_unix_secs,
        oauth_invalid_reason,
        encrypted_auth_config,
        None,
        std::future::ready(()),
    )
    .await
}

pub(crate) async fn persist_credential_fenced_provider_quota_refresh_state(
    state: &AdminAppState<'_>,
    key_id: &str,
    metadata_update: Option<&serde_json::Value>,
    oauth_invalid_at_unix_secs: Option<u64>,
    oauth_invalid_reason: Option<String>,
    encrypted_auth_config: Option<String>,
    expected_credential_fence: &ProviderTransportCredentialFence,
) -> Result<bool, GatewayError> {
    persist_provider_quota_refresh_state_after_read(
        state,
        key_id,
        metadata_update,
        oauth_invalid_at_unix_secs,
        oauth_invalid_reason,
        encrypted_auth_config,
        Some(expected_credential_fence),
        std::future::ready(()),
    )
    .await
}

pub(crate) async fn persist_codex_provider_quota_refresh_state(
    state: &AdminAppState<'_>,
    key_id: &str,
    metadata_update: Option<&serde_json::Value>,
    merge_context: admin_provider_quota_pure::CodexQuotaMergeContext<'_>,
) -> Result<bool, GatewayError> {
    let Some(incoming_codex) = metadata_update.and_then(|value| value.get("codex")) else {
        return persist_provider_quota_refresh_state(
            state,
            key_id,
            metadata_update,
            None,
            None,
            None,
        )
        .await;
    };

    for attempt in 0..CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
        let Some(mut latest_key) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let expected_codex = latest_key
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .cloned();
        if !admin_provider_quota_pure::codex_credential_generation_matches(
            expected_codex.as_ref(),
            merge_context.observed_credential_generation,
        ) {
            return Ok(true);
        }
        if codex_reset_refresh_is_superseded(expected_codex.as_ref(), merge_context) {
            return Ok(true);
        }
        let Some(outcome) = admin_provider_quota_pure::merge_codex_quota_metadata_snapshot(
            expected_codex.as_ref(),
            incoming_codex,
            merge_context,
        ) else {
            return Ok(false);
        };
        let merged_update = serde_json::json!({"codex": outcome.metadata.clone()});
        latest_key.upstream_metadata = Some(merge_upstream_metadata(
            latest_key.upstream_metadata.as_ref(),
            &merged_update,
        ));
        latest_key.status_snapshot = sync_provider_key_quota_status_snapshot(
            latest_key.status_snapshot.as_ref(),
            "codex",
            latest_key.upstream_metadata.as_ref(),
            "refresh_api",
        );
        latest_key.updated_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());

        let persisted = state
            .app()
            .update_provider_catalog_key_runtime_metadata(
                &ProviderCatalogKeyRuntimeMetadataUpdate {
                    key_id: key_id.to_string(),
                    namespace: "codex".to_string(),
                    expected_upstream_metadata_value: expected_codex,
                    upstream_metadata_value: outcome.metadata,
                    status_snapshot_patch: provider_quota_refresh_status_patch(
                        latest_key.status_snapshot.as_ref(),
                    ),
                    updated_at_unix_secs: latest_key.updated_at_unix_secs,
                },
            )
            .await?;
        if persisted {
            return Ok(true);
        }
        if attempt + 1 < CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
            let backoff_us = 50_u64.saturating_mul((attempt + 1) as u64).min(1_000);
            tokio::time::sleep(std::time::Duration::from_micros(backoff_us)).await;
        }
    }
    Ok(false)
}

/// Persist a Codex Agent Identity quota response only when the exact encrypted
/// auth_config used for the request is still installed. Metadata, OAuth state,
/// and their status projection share one repository CAS so a replacement cannot
/// receive any portion of an older response.
pub(crate) async fn persist_fenced_provider_quota_refresh_state(
    state: &AdminAppState<'_>,
    key_id: &str,
    expected_encrypted_auth_config: &str,
    metadata_update: Option<&serde_json::Value>,
    oauth_invalid_at_unix_secs: Option<u64>,
    oauth_invalid_reason: Option<String>,
    merge_context: admin_provider_quota_pure::CodexQuotaMergeContext<'_>,
    expected_credential: &ProviderCatalogKeyOAuthCredentialFence,
) -> Result<bool, GatewayError> {
    let expected_encrypted_auth_config = expected_encrypted_auth_config.trim();
    if expected_encrypted_auth_config.is_empty() {
        return Ok(false);
    }
    if metadata_update.is_some_and(|value| !value.is_object()) {
        return Err(GatewayError::Internal(
            "fenced quota metadata update must be an object".to_string(),
        ));
    }
    for attempt in 0..CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
        let Some(mut latest_key) = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        if latest_key.encrypted_auth_config.as_deref() != Some(expected_encrypted_auth_config) {
            return Ok(false);
        }

        let expected_codex = latest_key
            .upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .cloned();
        if !admin_provider_quota_pure::codex_credential_generation_matches(
            expected_codex.as_ref(),
            merge_context.observed_credential_generation,
        ) {
            return Ok(true);
        }
        if codex_reset_refresh_is_superseded(expected_codex.as_ref(), merge_context) {
            return Ok(true);
        }
        let merged_codex = match metadata_update.and_then(|value| value.get("codex")) {
            Some(incoming_codex) => {
                let Some(outcome) = admin_provider_quota_pure::merge_codex_quota_metadata_snapshot(
                    expected_codex.as_ref(),
                    incoming_codex,
                    merge_context,
                ) else {
                    return Ok(false);
                };
                outcome.metadata
            }
            None => expected_codex
                .clone()
                .unwrap_or_else(|| serde_json::json!({})),
        };
        let expected_codex_object = expected_codex
            .as_ref()
            .and_then(serde_json::Value::as_object);
        let stale_oauth_state = admin_provider_quota_pure::codex_oauth_state_request_order_is_stale(
            expected_codex_object,
            merge_context.request_started_at_unix_ms,
            merge_context.request_order_id,
        );
        let mut merged_codex = merged_codex.as_object().cloned().unwrap_or_default();
        if !stale_oauth_state {
            if let Some(request_started_at_unix_ms) = merge_context.request_started_at_unix_ms {
                merged_codex.insert(
                    CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY.to_string(),
                    serde_json::json!(request_started_at_unix_ms),
                );
                if let Some(request_order_id) = merge_context
                    .request_order_id
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    merged_codex.insert(
                        CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY.to_string(),
                        serde_json::json!(request_order_id),
                    );
                } else {
                    merged_codex.remove(CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY);
                }
            }
            (
                latest_key.oauth_invalid_at_unix_secs,
                latest_key.oauth_invalid_reason,
            ) = merge_codex_oauth_response_state(
                &latest_key,
                oauth_invalid_at_unix_secs,
                oauth_invalid_reason.as_deref(),
                merge_context.observed_at_unix_secs,
            );
        }
        let merged_metadata_update = serde_json::json!({
            "codex": serde_json::Value::Object(merged_codex)
        });
        latest_key.upstream_metadata = Some(merge_upstream_metadata(
            latest_key.upstream_metadata.as_ref(),
            &merged_metadata_update,
        ));
        if metadata_update
            .and_then(|value| value.get("codex"))
            .is_some()
        {
            latest_key.status_snapshot = sync_provider_key_quota_status_snapshot(
                latest_key.status_snapshot.as_ref(),
                "codex",
                latest_key.upstream_metadata.as_ref(),
                "refresh_api",
            );
        }
        latest_key.status_snapshot = sync_provider_key_oauth_status_snapshot(
            latest_key.status_snapshot.as_ref(),
            &latest_key,
        );
        latest_key.updated_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());

        let updated = state
            .app()
            .compare_and_update_provider_catalog_key_oauth_runtime_state(
                &ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
                    key_id: key_id.to_string(),
                    expected_encrypted_auth_config: Some(
                        expected_encrypted_auth_config.to_string(),
                    ),
                    expected_credential: Some(expected_credential.clone()),
                    expected_upstream_metadata_namespace: Some(
                        ProviderCatalogUpstreamMetadataNamespaceExpectation {
                            namespace: "codex".to_string(),
                            expected_value: expected_codex,
                        },
                    ),
                    encrypted_auth_config: expected_encrypted_auth_config.to_string(),
                    encrypted_api_key_update: None,
                    expires_at_unix_secs_update: None,
                    oauth_invalid_at_unix_secs: latest_key.oauth_invalid_at_unix_secs,
                    oauth_invalid_reason: latest_key.oauth_invalid_reason.clone(),
                    upstream_metadata_patch: Some(merged_metadata_update),
                    upstream_metadata_namespace_to_remove: None,
                    status_snapshot_patch: provider_quota_refresh_status_patch(
                        latest_key.status_snapshot.as_ref(),
                    ),
                    reset_error_count: false,
                    updated_at_unix_secs: latest_key.updated_at_unix_secs,
                },
            )
            .await?;
        if updated {
            return Ok(true);
        }
        if attempt + 1 < CODEX_QUOTA_PERSIST_CAS_MAX_ATTEMPTS {
            let backoff_us = 50_u64.saturating_mul((attempt + 1) as u64).min(1_000);
            tokio::time::sleep(std::time::Duration::from_micros(backoff_us)).await;
        }
    }
    Ok(false)
}

async fn persist_provider_quota_refresh_state_after_read<F>(
    state: &AdminAppState<'_>,
    key_id: &str,
    metadata_update: Option<&serde_json::Value>,
    oauth_invalid_at_unix_secs: Option<u64>,
    oauth_invalid_reason: Option<String>,
    encrypted_auth_config: Option<String>,
    expected_credential_fence: Option<&ProviderTransportCredentialFence>,
    after_read: F,
) -> Result<bool, GatewayError>
where
    F: std::future::Future<Output = ()>,
{
    let safe_metadata_update =
        metadata_update.map(|value| admin_provider_upstream_metadata_safe_json(Some(value)));
    let metadata_update = safe_metadata_update.as_ref();
    let Some(mut latest_key) = state
        .app()
        .list_provider_catalog_keys_by_ids_strong(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    after_read.await;

    // Keep the namespace values observed before applying the refresh response;
    // each runtime metadata write uses them as its CAS expectation.
    let observed_encrypted_auth_config = latest_key.encrypted_auth_config.clone();
    let observed_upstream_metadata = latest_key.upstream_metadata.clone();
    let mut quota_snapshot_provider_type = None::<String>;
    if let Some(metadata_update) = metadata_update {
        latest_key.upstream_metadata = Some(merge_upstream_metadata(
            latest_key.upstream_metadata.as_ref(),
            metadata_update,
        ));
        quota_snapshot_provider_type =
            aether_provider_pool::provider_pool_quota_metadata_provider_type(metadata_update);
    }
    if let Some(encrypted_auth_config) = encrypted_auth_config.as_ref() {
        latest_key.encrypted_auth_config = Some(encrypted_auth_config.clone());
    }
    latest_key.oauth_invalid_at_unix_secs = oauth_invalid_at_unix_secs;
    latest_key.oauth_invalid_reason = oauth_invalid_reason;
    if let Some(provider_type) = quota_snapshot_provider_type.as_deref() {
        latest_key.status_snapshot = sync_provider_key_quota_status_snapshot(
            latest_key.status_snapshot.as_ref(),
            provider_type,
            latest_key.upstream_metadata.as_ref(),
            "refresh_api",
        );
    }
    latest_key.status_snapshot =
        sync_provider_key_oauth_status_snapshot(latest_key.status_snapshot.as_ref(), &latest_key);
    latest_key.updated_at_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    let status_patch = provider_quota_refresh_status_patch(latest_key.status_snapshot.as_ref());
    let metadata_updates = metadata_update
        .and_then(serde_json::Value::as_object)
        .map(|updates| {
            let merged = latest_key
                .upstream_metadata
                .as_ref()
                .and_then(serde_json::Value::as_object);
            updates
                .keys()
                .filter_map(|namespace| {
                    merged
                        .and_then(|metadata| metadata.get(namespace))
                        .cloned()
                        .map(|value| (namespace.clone(), value))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(expected_credential_fence) = expected_credential_fence {
        if observed_encrypted_auth_config.as_deref()
            != Some(expected_credential_fence.encrypted_auth_config.as_str())
            || latest_key.encrypted_api_key
                != expected_credential_fence.credential.encrypted_api_key
            || latest_key.auth_type != expected_credential_fence.credential.auth_type
            || latest_key.provider_id != expected_credential_fence.credential.provider_id
        {
            return Ok(false);
        }
        if metadata_updates.len() > 1 {
            return Err(GatewayError::Internal(
                "credential-fenced quota refresh may update at most one metadata namespace"
                    .to_string(),
            ));
        }
        let expected_upstream_metadata_namespace =
            metadata_updates.first().map(|(namespace, _)| {
                ProviderCatalogUpstreamMetadataNamespaceExpectation {
                    namespace: namespace.clone(),
                    expected_value: observed_upstream_metadata
                        .as_ref()
                        .and_then(serde_json::Value::as_object)
                        .and_then(|metadata| metadata.get(namespace))
                        .cloned(),
                }
            });
        return state
            .app()
            .compare_and_update_provider_catalog_key_oauth_runtime_state(
                &ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
                    key_id: key_id.to_string(),
                    expected_encrypted_auth_config: Some(
                        expected_credential_fence.encrypted_auth_config.clone(),
                    ),
                    expected_credential: Some(expected_credential_fence.credential.clone()),
                    expected_upstream_metadata_namespace,
                    encrypted_auth_config: encrypted_auth_config
                        .clone()
                        .unwrap_or_else(|| expected_credential_fence.encrypted_auth_config.clone()),
                    encrypted_api_key_update: None,
                    expires_at_unix_secs_update: None,
                    oauth_invalid_at_unix_secs: latest_key.oauth_invalid_at_unix_secs,
                    oauth_invalid_reason: latest_key.oauth_invalid_reason.clone(),
                    upstream_metadata_patch: metadata_update.cloned(),
                    upstream_metadata_namespace_to_remove: None,
                    status_snapshot_patch: status_patch,
                    reset_error_count: false,
                    updated_at_unix_secs: latest_key.updated_at_unix_secs,
                },
            )
            .await;
    }
    if encrypted_auth_config.is_some() {
        return Err(GatewayError::Internal(
            "provider quota credential update requires a pre-request credential fence".to_string(),
        ));
    }
    if metadata_updates.is_empty() {
        if !state
            .update_provider_catalog_key_oauth_runtime_state(
                key_id,
                latest_key.oauth_invalid_at_unix_secs,
                latest_key.oauth_invalid_reason.as_deref(),
                latest_key.updated_at_unix_secs,
            )
            .await?
        {
            return Ok(false);
        }
        return state
            .update_provider_catalog_key_status_snapshot(&ProviderCatalogKeyStatusSnapshotUpdate {
                key_id: key_id.to_string(),
                status_snapshot_patch: status_patch,
                updated_at_unix_secs: latest_key.updated_at_unix_secs,
            })
            .await;
    }

    for (index, (namespace, value)) in metadata_updates.iter().enumerate() {
        let patch = if index + 1 == metadata_updates.len() {
            status_patch.clone()
        } else {
            serde_json::json!({})
        };
        let expected = observed_upstream_metadata
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get(namespace))
            .cloned();
        let persisted = state
            .app()
            .update_provider_catalog_key_runtime_metadata(
                &ProviderCatalogKeyRuntimeMetadataUpdate {
                    key_id: key_id.to_string(),
                    namespace: namespace.clone(),
                    expected_upstream_metadata_value: expected.clone(),
                    upstream_metadata_value: value.clone(),
                    status_snapshot_patch: patch.clone(),
                    updated_at_unix_secs: latest_key.updated_at_unix_secs,
                },
            )
            .await?;
        if !persisted {
            // The refresh response is an authoritative snapshot.  Do not
            // replay it over a newer namespace after a CAS conflict.
            return Ok(false);
        }
    }
    state
        .update_provider_catalog_key_oauth_runtime_state(
            key_id,
            latest_key.oauth_invalid_at_unix_secs,
            latest_key.oauth_invalid_reason.as_deref(),
            latest_key.updated_at_unix_secs,
        )
        .await
}

fn provider_quota_refresh_status_patch(
    status_snapshot: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut patch = serde_json::Map::new();
    if let Some(snapshot) = status_snapshot.and_then(serde_json::Value::as_object) {
        for field in ["quota", "oauth"] {
            if let Some(value) = snapshot.get(field) {
                patch.insert(field.to_string(), value.clone());
            }
        }
    }
    serde_json::Value::Object(patch)
}

pub(super) async fn execute_provider_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    plan: ExecutionPlan,
    quota_kind: &str,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let provider_name = plan.provider_name.as_deref().unwrap_or_default();
    if !provider_quota_url_has_allowed_origin(provider_name, &plan.url) {
        warn!(
            key_id = %transport.key.id,
            endpoint_id = %transport.endpoint.id,
            provider_name,
            quota_kind,
            upstream_origin = %crate::handlers::shared::security_log_url_origin(&plan.url),
            "gateway provider quota request blocked by origin policy"
        );
        return Ok(ProviderQuotaExecutionOutcome::Failure(
            "Provider quota request origin is not allowed".to_string(),
        ));
    }

    match state.execute_execution_runtime_sync_plan(None, &plan).await {
        Ok(result) => {
            if !crate::provider_transport::is_codex_agent_identity_transport(transport)
                || !crate::provider_transport::is_codex_agent_identity_invalid_task_response(
                    result.status_code,
                    extract_execution_error_detail(&result).as_deref(),
                )
            {
                return Ok(ProviderQuotaExecutionOutcome::Response(result));
            }

            let refreshed_entry = match state.force_local_oauth_refresh_entry(transport).await {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    return Ok(ProviderQuotaExecutionOutcome::Failure(
                        "Agent Identity 任务重注册未返回认证信息".to_string(),
                    ));
                }
                Err(_) => {
                    warn!(
                        key_id = %transport.key.id,
                        endpoint_id = %transport.endpoint.id,
                        quota_kind = %quota_kind,
                        "gateway Agent Identity quota task recovery failed"
                    );
                    return Ok(ProviderQuotaExecutionOutcome::Failure(
                        "Agent Identity 任务重注册失败".to_string(),
                    ));
                }
            };
            let header_name = refreshed_entry.auth_header_name.trim().to_ascii_lowercase();
            let header_value = refreshed_entry.auth_header_value.trim();
            if header_name.is_empty() || header_value.is_empty() {
                return Ok(ProviderQuotaExecutionOutcome::Failure(
                    "Agent Identity 任务重注册未返回有效认证信息".to_string(),
                ));
            }

            let mut retry_plan = plan.clone();
            retry_plan
                .headers
                .retain(|name, _| !name.eq_ignore_ascii_case(&header_name));
            retry_plan
                .headers
                .insert(header_name, header_value.to_string());
            match state
                .execute_execution_runtime_sync_plan(None, &retry_plan)
                .await
            {
                Ok(result) => Ok(ProviderQuotaExecutionOutcome::Response(result)),
                Err(_) => {
                    warn!(
                        key_id = %transport.key.id,
                        endpoint_id = %transport.endpoint.id,
                        quota_kind = %quota_kind,
                        "gateway Agent Identity quota task recovery retry failed"
                    );
                    Ok(ProviderQuotaExecutionOutcome::Failure(
                        "Provider quota request failed".to_string(),
                    ))
                }
            }
        }
        Err(_) => {
            let proxy_node_id = plan
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.node_id.as_deref())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            let proxy_source = state
                .resolve_transport_proxy_source_with_tunnel_affinity(transport)
                .await;
            let proxy_url_present = plan
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.url.as_deref())
                .map(str::trim)
                .is_some_and(|value| !value.is_empty());
            warn!(
                key_id = %transport.key.id,
                endpoint_id = %transport.endpoint.id,
                upstream_origin = %crate::handlers::shared::security_log_url_origin(&plan.url),
                proxy_source = ?proxy_source,
                proxy_node_id = ?proxy_node_id,
                proxy_url_present,
                quota_kind = %quota_kind,
                "gateway provider quota execution runtime request failed"
            );
            Ok(ProviderQuotaExecutionOutcome::Failure(
                "Provider quota request failed".to_string(),
            ))
        }
    }
}

fn provider_quota_url_has_allowed_origin(provider_name: &str, value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };

    match provider_name.trim().to_ascii_lowercase().as_str() {
        "antigravity" => matches!(
            host,
            "cloudcode-pa.googleapis.com"
                | "daily-cloudcode-pa.googleapis.com"
                | "daily-cloudcode-pa.sandbox.googleapis.com"
        ),
        "gemini_cli" => host == "cloudcode-pa.googleapis.com",
        "chatgpt_web" | "codex" => host == "chatgpt.com",
        "grok" => host == "grok.com",
        "windsurf" => host == "server.codeium.com",
        "kiro" => kiro_quota_host_is_allowed(host),
        _ => false,
    }
}

fn kiro_quota_host_is_allowed(host: &str) -> bool {
    let Some(region) = host
        .strip_prefix("q.")
        .and_then(|host| host.strip_suffix(".amazonaws.com"))
    else {
        return false;
    };
    !region.is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && region
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && region
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::GatewayDataState;
    use crate::AppState;
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, ProviderCatalogWriteRepository, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn quota_plans_cannot_enable_redirects_through_header_overrides() {
        let mut headers = std::collections::BTreeMap::from([
            (
                "X-Aether-Execution-Follow-Redirects".to_string(),
                "true".to_string(),
            ),
            ("authorization".to_string(), "Bearer secret".to_string()),
        ]);

        force_provider_quota_redirects_disabled(&mut headers);

        assert_eq!(
            headers
                .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                .map(String::as_str),
            Some("false")
        );
        assert_eq!(
            headers
                .keys()
                .filter(|name| {
                    name.eq_ignore_ascii_case(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                })
                .count(),
            1
        );
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer secret")
        );
    }

    #[test]
    fn quota_origin_policy_accepts_only_provider_owned_https_origins() {
        for (provider_name, url) in [
            (
                "antigravity",
                "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
            ),
            (
                "antigravity",
                "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
            ),
            (
                "antigravity",
                "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:fetchAvailableModels",
            ),
            (
                "gemini_cli",
                "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota",
            ),
            (
                "chatgpt_web",
                "https://chatgpt.com/backend-api/conversation/init",
            ),
            ("codex", "https://chatgpt.com/backend-api/wham/usage"),
            ("grok", "https://grok.com/rest/rate-limits"),
            (
                "windsurf",
                "https://server.codeium.com/exa.seat_management_pb.SeatManagementService/GetUserStatus",
            ),
            (
                "kiro",
                "https://q.us-east-1.amazonaws.com/getUsageLimits?origin=AI_EDITOR",
            ),
        ] {
            assert!(
                provider_quota_url_has_allowed_origin(provider_name, url),
                "expected {provider_name} quota URL to be allowed: {url}"
            );
        }
    }

    #[test]
    fn quota_origin_policy_rejects_ssrf_and_credential_redirect_origins() {
        for (provider_name, url) in [
            (
                "chatgpt_web",
                "http://chatgpt.com/backend-api/conversation/init",
            ),
            ("codex", "https://chatgpt.com:444/backend-api/wham/usage"),
            (
                "codex",
                "https://user:secret@chatgpt.com/backend-api/wham/usage",
            ),
            (
                "codex",
                "https://chatgpt.com.attacker.test/backend-api/wham/usage",
            ),
            ("grok", "https://grok.com.attacker.test/rest/rate-limits"),
            ("windsurf", "https://server.codeium.com.attacker.test/quota"),
            (
                "gemini_cli",
                "https://quota-proxy.internal/retrieveUserQuota",
            ),
            (
                "antigravity",
                "https://cloudcode-pa.googleapis.com.attacker.test/quota",
            ),
            ("kiro", "https://q.localhost:8443/getUsageLimits"),
            (
                "kiro",
                "https://q.us-east-1.evil.amazonaws.com/getUsageLimits",
            ),
            (
                "kiro",
                "https://q.us-east-1.amazonaws.com.attacker.test/getUsageLimits",
            ),
            ("unknown", "https://chatgpt.com/backend-api/wham/usage"),
        ] {
            assert!(
                !provider_quota_url_has_allowed_origin(provider_name, url),
                "expected {provider_name} quota URL to be rejected: {url}"
            );
        }
    }

    fn codex_merge_context(
        request_started_at_unix_ms: u64,
    ) -> admin_provider_quota_pure::CodexQuotaMergeContext<'static> {
        codex_merge_context_with_id(request_started_at_unix_ms, None)
    }

    fn codex_merge_context_with_id(
        request_started_at_unix_ms: u64,
        request_order_id: Option<&'static str>,
    ) -> admin_provider_quota_pure::CodexQuotaMergeContext<'static> {
        admin_provider_quota_pure::CodexQuotaMergeContext {
            observed_at_unix_secs: request_started_at_unix_ms / 1_000,
            request_started_at_unix_ms: Some(request_started_at_unix_ms),
            request_order_id,
            observed_reset_generation: Some(0),
            authoritative_reset_generation: None,
            observed_credential_generation: None,
            account_reset_fence_id: None,
            coverage: admin_provider_quota_pure::CodexQuotaWindowCoverage::AccountSnapshot,
        }
    }

    fn codex_reset_merge_context(
        request_started_at_unix_ms: u64,
        fence_id: &'static str,
    ) -> admin_provider_quota_pure::CodexQuotaMergeContext<'static> {
        admin_provider_quota_pure::CodexQuotaMergeContext {
            observed_at_unix_secs: request_started_at_unix_ms / 1_000,
            request_started_at_unix_ms: Some(request_started_at_unix_ms),
            request_order_id: Some("reset-refresh"),
            observed_reset_generation: Some(0),
            authoritative_reset_generation: None,
            observed_credential_generation: None,
            account_reset_fence_id: Some(fence_id),
            coverage: admin_provider_quota_pure::CodexQuotaWindowCoverage::AccountSnapshot,
        }
    }

    fn codex_refresh_test_state(
        key_id: &str,
        auth_config: Option<&str>,
    ) -> (
        AppState,
        Arc<InMemoryProviderCatalogReadRepository>,
        Option<ProviderTransportCredentialFence>,
    ) {
        let provider = StoredProviderCatalogProvider::new(
            "provider-codex-refresh".to_string(),
            "Codex Refresh".to_string(),
            None,
            "codex".to_string(),
        )
        .expect("provider should build");
        let bootstrap = AppState::new()
            .expect("bootstrap app should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let encrypted_auth_config = auth_config
            .map(|plaintext| {
                bootstrap.seal_provider_catalog_key_auth_config(&provider.id, key_id, plaintext)
            })
            .transpose()
            .expect("auth config should seal");
        let mut key = StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider.id.clone(),
            "Codex Refresh".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.encrypted_auth_config = encrypted_auth_config.clone();
        key.upstream_metadata = Some(json!({
            "codex": {
                "plan_type": "plus",
                "primary_used_percent": 60.0,
                "primary_reset_at": 2_000_000_000u64,
                "primary_window_minutes": 300u64,
                "account_quota_request_started_at_unix_ms": 200_000u64,
                "updated_at": 200u64
            }
        }));
        let credential_fence =
            encrypted_auth_config.map(|encrypted_auth_config| ProviderTransportCredentialFence {
                encrypted_auth_config,
                credential: ProviderCatalogKeyOAuthCredentialFence {
                    encrypted_api_key: key.encrypted_api_key.clone(),
                    auth_type: key.auth_type.clone(),
                    provider_id: key.provider_id.clone(),
                    provider_type: provider.provider_type.clone(),
                },
            });
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![],
            vec![key],
        ));
        let app = AppState::new()
            .expect("app should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        (app, repository, credential_fence)
    }

    fn codex_reset_state_machine_test_state(
        key_id: &str,
    ) -> (
        AppState,
        Arc<InMemoryProviderCatalogReadRepository>,
        ProviderTransportCredentialFence,
    ) {
        let provider = StoredProviderCatalogProvider::new(
            "provider-codex-reset-state".to_string(),
            "Codex Reset State".to_string(),
            None,
            "codex".to_string(),
        )
        .expect("provider should build");
        let bootstrap = AppState::new()
            .expect("bootstrap app should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let encrypted_auth_config = bootstrap
            .seal_provider_catalog_key_auth_config(&provider.id, key_id, "auth-v1")
            .expect("auth config should seal");
        let mut key = StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider.id.clone(),
            "Codex Reset".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.encrypted_auth_config = Some(encrypted_auth_config.clone());
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "credential-v1",
                "reset_credits": {
                    "available_count": 2,
                    "updated_at": 100u64,
                    "detail_source": "wham_readonly",
                    "detail_status": "available",
                    "credits": [
                        {"id": "credit-1", "expires_at": 20_000u64},
                        {"id": "credit-2", "expires_at": 30_000u64}
                    ]
                }
            }
        }));
        let credential_fence = ProviderTransportCredentialFence {
            encrypted_auth_config,
            credential: ProviderCatalogKeyOAuthCredentialFence {
                encrypted_api_key: None,
                auth_type: key.auth_type.clone(),
                provider_id: provider.id.clone(),
                provider_type: provider.provider_type.clone(),
            },
        };
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![],
            vec![key],
        ));
        let app = AppState::new()
            .expect("app should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        (app, repository, credential_fence)
    }

    #[tokio::test]
    async fn codex_reset_reservation_serializes_ids_and_reuses_same_generation() {
        let key_id = "key-codex-reset-reservation";
        let (app, repository, credential) = codex_reset_state_machine_test_state(key_id);
        let admin_state = AdminAppState::new(&app);

        let first = reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "reset-a",
        )
        .await
        .expect("reservation should complete")
        .expect("reservation should exist");
        let first = match first {
            CodexAccountResetReserveResult::Reserved(value) => value,
            other => panic!("unexpected first reservation: {other:?}"),
        };
        let same = reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "reset-a",
        )
        .await
        .expect("same reservation should complete")
        .expect("same reservation should exist");
        assert_eq!(
            same,
            CodexAccountResetReserveResult::Reserved(first.clone())
        );
        let other = reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "reset-b",
        )
        .await
        .expect("busy check should complete")
        .expect("busy reservation should exist");
        assert_eq!(other, CodexAccountResetReserveResult::Busy(first.clone()));

        // An ambiguous upstream result does not call complete; the durable
        // reservation continues to block another id while the same id resumes.
        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let codex = &stored.upstream_metadata.unwrap()["codex"];
        assert_eq!(codex["account_quota_reset_sequence"], json!(1u64));
        assert_eq!(
            codex["account_quota_reset_reservation"]["idempotency_key"],
            json!("reset-a")
        );
        assert!(codex.get("account_quota_reset_generation").is_none());
    }

    #[tokio::test]
    async fn codex_reset_reservation_rejects_replaced_credential_generation() {
        let key_id = "key-codex-reset-credential-generation";
        let (app, repository, credential) = codex_reset_state_machine_test_state(key_id);
        let admin_state = AdminAppState::new(&app);
        let original_metadata = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should load before reservation")
            .pop()
            .expect("key should exist before reservation")
            .upstream_metadata;

        let result = reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-before-rebind"),
            "reset-from-old-account",
        )
        .await
        .expect("generation fence should complete")
        .expect("generation mismatch should be explicit");
        assert_eq!(
            result,
            CodexAccountResetReserveResult::CredentialGenerationMismatch
        );

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.upstream_metadata, original_metadata);
    }

    #[tokio::test]
    async fn codex_reset_noop_does_not_activate_but_later_id_gets_new_generation() {
        let key_id = "key-codex-reset-noop";
        let (app, repository, credential) = codex_reset_state_machine_test_state(key_id);
        let admin_state = AdminAppState::new(&app);
        let reservation = match reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "reset-noop",
        )
        .await
        .unwrap()
        .unwrap()
        {
            CodexAccountResetReserveResult::Reserved(value) => value,
            other => panic!("unexpected reservation: {other:?}"),
        };
        assert!(matches!(
            complete_codex_account_reset(
                &admin_state,
                key_id,
                credential.encrypted_auth_config.as_str(),
                &credential.credential,
                &reservation,
                "nothing_to_reset",
                200_000,
            )
            .await
            .unwrap(),
            Some(CodexAccountResetCompleteResult::Noop(_))
        ));
        let next = reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "reset-next",
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            next,
            CodexAccountResetReserveResult::Reserved(CodexAccountResetReservation {
                generation: 2,
                ..
            })
        ));
        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .unwrap()
            .pop()
            .unwrap();
        let codex = &stored.upstream_metadata.unwrap()["codex"];
        assert!(codex.get("account_quota_reset_generation").is_none());
    }

    async fn complete_codex_reset_in_order(
        first_outcome: &str,
        second_outcome: &str,
    ) -> serde_json::Value {
        let key_id = format!("key-codex-reset-order-{first_outcome}");
        let (app, repository, credential) = codex_reset_state_machine_test_state(&key_id);
        let admin_state = AdminAppState::new(&app);
        let reservation = match reserve_codex_account_reset(
            &admin_state,
            &key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "same-id",
        )
        .await
        .unwrap()
        .unwrap()
        {
            CodexAccountResetReserveResult::Reserved(value) => value,
            other => panic!("unexpected reservation: {other:?}"),
        };
        complete_codex_account_reset(
            &admin_state,
            &key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            &reservation,
            first_outcome,
            200_000,
        )
        .await
        .unwrap()
        .expect("first completion should persist");
        complete_codex_account_reset(
            &admin_state,
            &key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            &reservation,
            second_outcome,
            210_000,
        )
        .await
        .unwrap()
        .expect("second completion should converge");
        repository
            .list_keys_by_ids(&[key_id])
            .await
            .unwrap()
            .pop()
            .unwrap()
            .upstream_metadata
            .unwrap()["codex"]
            .clone()
    }

    #[tokio::test]
    async fn codex_reset_activation_wins_over_noop_in_both_completion_orders() {
        for codex in [
            complete_codex_reset_in_order("nothing_to_reset", "reset").await,
            complete_codex_reset_in_order("reset", "nothing_to_reset").await,
        ] {
            assert_eq!(codex["account_quota_reset_generation"], json!(1u64));
            assert_eq!(codex["account_quota_reset_pending_generation"], json!(1u64));
            assert_eq!(codex["account_quota_reset_pending"], json!(true));
            assert_eq!(
                codex["account_quota_reset_history"][0]["outcome"],
                json!("reset")
            );
            assert_eq!(codex["reset_credits"]["available_count"], json!(1u64));
            assert_eq!(
                codex["reset_credits"]["credits"],
                json!([{"id": "credit-2", "expires_at": 30_000u64}])
            );
        }
    }

    #[tokio::test]
    async fn delayed_reset_upgrade_preserves_the_next_generation_reservation() {
        let key_id = "key-codex-reset-upgrade-next-generation";
        let (app, repository, credential) = codex_reset_state_machine_test_state(key_id);
        let admin_state = AdminAppState::new(&app);
        let first = match reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "reset-first",
        )
        .await
        .unwrap()
        .unwrap()
        {
            CodexAccountResetReserveResult::Reserved(value) => value,
            other => panic!("unexpected first reservation: {other:?}"),
        };
        complete_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            &first,
            "nothing_to_reset",
            200_000,
        )
        .await
        .unwrap()
        .expect("first noop should complete");
        let second = match reserve_codex_account_reset(
            &admin_state,
            key_id,
            credential.encrypted_auth_config.as_str(),
            &credential.credential,
            Some("credential-v1"),
            "reset-second",
        )
        .await
        .unwrap()
        .unwrap()
        {
            CodexAccountResetReserveResult::Reserved(value) => value,
            other => panic!("unexpected second reservation: {other:?}"),
        };
        assert_eq!(second.generation, 2);

        assert!(matches!(
            complete_codex_account_reset(
                &admin_state,
                key_id,
                credential.encrypted_auth_config.as_str(),
                &credential.credential,
                &first,
                "reset",
                210_000,
            )
            .await
            .unwrap(),
            Some(CodexAccountResetCompleteResult::Activated(
                CodexAccountResetFence { generation: 1, .. }
            ))
        ));
        let after_upgrade = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            after_upgrade.upstream_metadata.as_ref().unwrap()["codex"]
                ["account_quota_reset_reservation"],
            json!({
                "idempotency_key": "reset-second",
                "generation": 2,
            })
        );

        assert!(matches!(
            complete_codex_account_reset(
                &admin_state,
                key_id,
                credential.encrypted_auth_config.as_str(),
                &credential.credential,
                &second,
                "reset",
                220_000,
            )
            .await
            .unwrap(),
            Some(CodexAccountResetCompleteResult::Activated(
                CodexAccountResetFence { generation: 2, .. }
            ))
        ));
        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .unwrap()
            .pop()
            .unwrap();
        let codex = &stored.upstream_metadata.unwrap()["codex"];
        assert_eq!(codex["account_quota_reset_generation"], json!(2u64));
        assert!(codex.get("account_quota_reset_reservation").is_none());
    }

    #[tokio::test]
    async fn stale_codex_refresh_cannot_lower_realtime_usage() {
        let key_id = "key-codex-refresh-monotonic";
        let (app, repository, _) = codex_refresh_test_state(key_id, None);
        let admin_state = AdminAppState::new(&app);
        let stale_refresh = json!({"codex": {
            "plan_type": "plus",
            "primary_used_percent": 50.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64,
            "updated_at": 100u64
        }});

        assert!(persist_codex_provider_quota_refresh_state(
            &admin_state,
            key_id,
            Some(&stale_refresh),
            codex_merge_context(100_000),
        )
        .await
        .expect("refresh persistence should complete"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(codex["primary_used_percent"], json!(60.0));
        assert_eq!(
            codex["account_quota_request_started_at_unix_ms"],
            json!(200_000u64)
        );
    }

    #[tokio::test]
    async fn codex_reset_fence_is_idempotent_and_rejects_pre_reset_response() {
        let key_id = "key-codex-reset-fence";
        let (app, repository, _) = codex_refresh_test_state(key_id, None);
        let admin_state = AdminAppState::new(&app);

        let initial_fence = persist_codex_account_reset_fence(
            &admin_state,
            key_id,
            None,
            None,
            250_000,
            "fence-a",
            "redeem-once",
        )
        .await
        .expect("reset fence should persist")
        .expect("reset fence should be returned");
        let initial_fence = match initial_fence {
            CodexAccountResetFenceInstall::Owned(fence) => fence,
            CodexAccountResetFenceInstall::Superseded => {
                panic!("initial reset should own its fence")
            }
        };
        let duplicate_fence = persist_codex_account_reset_fence(
            &admin_state,
            key_id,
            None,
            None,
            300_000,
            "fence-a",
            "redeem-once",
        )
        .await
        .expect("duplicate reset fence should be idempotent")
        .expect("duplicate should return the installed fence");
        let duplicate_fence = match duplicate_fence {
            CodexAccountResetFenceInstall::Owned(fence) => fence,
            CodexAccountResetFenceInstall::Superseded => {
                panic!("duplicate active reset should retain ownership")
            }
        };
        assert_eq!(duplicate_fence, initial_fence);

        let stale = json!({"codex": {
            "primary_used_percent": 100.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});
        assert!(persist_codex_provider_quota_refresh_state(
            &admin_state,
            key_id,
            Some(&stale),
            codex_merge_context(200_000),
        )
        .await
        .expect("stale response should be harmlessly acknowledged"));

        let baseline = json!({"codex": {
            "primary_used_percent": 0.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});
        assert!(persist_codex_provider_quota_refresh_state(
            &admin_state,
            key_id,
            Some(&baseline),
            codex_reset_merge_context(260_000, "fence-a"),
        )
        .await
        .expect("reset baseline should persist"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(codex["primary_used_percent"], json!(0.0));
        assert_eq!(codex["account_quota_reset_fence_id"], json!("fence-a"));
        assert_eq!(codex["account_quota_reset_pending"], json!(false));
    }

    #[tokio::test]
    async fn codex_reset_fence_barrier_never_moves_backward_and_remembers_processed_ids() {
        let key_id = "key-codex-reset-fence-order";
        let (app, repository, _) = codex_refresh_test_state(key_id, None);
        let admin_state = AdminAppState::new(&app);

        let newer = persist_codex_account_reset_fence(
            &admin_state,
            key_id,
            None,
            None,
            300_000,
            "fence-newer",
            "redeem-newer",
        )
        .await
        .expect("newer reset fence should persist")
        .expect("newer reset fence should be returned");
        assert!(matches!(
            newer,
            CodexAccountResetFenceInstall::Owned(CodexAccountResetFence {
                unix_ms: 300_000,
                ref id,
                ..
            }) if id == "fence-newer"
        ));
        let delayed_older = persist_codex_account_reset_fence(
            &admin_state,
            key_id,
            None,
            None,
            250_000,
            "fence-older",
            "redeem-older",
        )
        .await
        .expect("older reset should be recorded")
        .expect("install result should be returned");
        assert_eq!(delayed_older, CodexAccountResetFenceInstall::Superseded);

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(
            codex["account_quota_reset_fence_unix_ms"],
            json!(300_000u64)
        );
        assert_eq!(codex["account_quota_reset_fence_id"], json!("fence-newer"));
        let processed_ids = codex["account_quota_reset_processed_ids"]
            .as_array()
            .expect("processed reset ids should be an array");
        assert_eq!(processed_ids.len(), 2);
        assert!(processed_ids.contains(&json!("redeem-older")));
        assert!(processed_ids.contains(&json!("redeem-newer")));
    }

    #[tokio::test]
    async fn concurrent_codex_reset_fences_converge_on_newest_barrier() {
        let key_id = "key-codex-reset-fence-concurrent";
        let (app, repository, _) = codex_refresh_test_state(key_id, None);
        let admin_state = AdminAppState::new(&app);

        let (older, newer) = tokio::join!(
            persist_codex_account_reset_fence(
                &admin_state,
                key_id,
                None,
                None,
                250_000,
                "fence-older",
                "redeem-older",
            ),
            persist_codex_account_reset_fence(
                &admin_state,
                key_id,
                None,
                None,
                300_000,
                "fence-newer",
                "redeem-newer",
            ),
        );
        let older = older
            .expect("older reset should complete")
            .expect("older reset should return an install result");
        let newer = newer
            .expect("newer reset should complete")
            .expect("newer reset should return an install result");
        assert!(matches!(
            (older, newer),
            (
                CodexAccountResetFenceInstall::Owned(_),
                CodexAccountResetFenceInstall::Owned(_)
            ) | (
                CodexAccountResetFenceInstall::Superseded,
                CodexAccountResetFenceInstall::Owned(_)
            )
        ));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(
            codex["account_quota_reset_fence_unix_ms"],
            json!(300_000u64)
        );
        assert_eq!(codex["account_quota_reset_fence_id"], json!("fence-newer"));
        assert_eq!(codex["account_quota_reset_pending"], json!(true));
        let processed_ids = codex["account_quota_reset_processed_ids"]
            .as_array()
            .expect("processed reset ids should be an array");
        assert_eq!(processed_ids.len(), 2);
        assert!(processed_ids.contains(&json!("redeem-older")));
        assert!(processed_ids.contains(&json!("redeem-newer")));
    }

    #[tokio::test]
    async fn superseded_codex_reset_refresh_cannot_confirm_newer_fence() {
        let key_id = "key-codex-reset-fence-stale-refresh";
        let (app, repository, _) = codex_refresh_test_state(key_id, None);
        let admin_state = AdminAppState::new(&app);

        for (fence_unix_ms, fence_id, redeem_id) in [
            (250_000, "fence-older", "redeem-older"),
            (300_000, "fence-newer", "redeem-newer"),
        ] {
            let install = persist_codex_account_reset_fence(
                &admin_state,
                key_id,
                None,
                None,
                fence_unix_ms,
                fence_id,
                redeem_id,
            )
            .await
            .expect("reset fence should persist")
            .expect("reset fence should return an install result");
            assert!(matches!(install, CodexAccountResetFenceInstall::Owned(_)));
        }

        let stale_baseline = json!({"codex": {
            "primary_used_percent": 0.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});
        assert!(persist_codex_provider_quota_refresh_state(
            &admin_state,
            key_id,
            Some(&stale_baseline),
            admin_provider_quota_pure::CodexQuotaMergeContext {
                observed_at_unix_secs: 310,
                request_started_at_unix_ms: Some(310_000),
                request_order_id: Some("older-reset-late-refresh"),
                observed_reset_generation: Some(0),
                authoritative_reset_generation: None,
                observed_credential_generation: None,
                account_reset_fence_id: Some("fence-older"),
                coverage: admin_provider_quota_pure::CodexQuotaWindowCoverage::AccountSnapshot,
            },
        )
        .await
        .expect("superseded reset refresh should be harmlessly acknowledged"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(codex["primary_used_percent"], json!(60.0));
        assert_eq!(codex["account_quota_reset_fence_id"], json!("fence-newer"));
        assert_eq!(codex["account_quota_reset_pending"], json!(true));
    }

    #[tokio::test]
    async fn replaying_historical_codex_reset_does_not_reopen_pending() {
        let key_id = "key-codex-reset-fence-replay";
        let (app, repository, _) = codex_refresh_test_state(key_id, None);
        let admin_state = AdminAppState::new(&app);

        for (fence_unix_ms, fence_id, redeem_id, request_started_at_unix_ms, usage) in [
            (250_000, "fence-a", "redeem-a", 260_000, 20.0),
            (300_000, "fence-b", "redeem-b", 310_000, 0.0),
        ] {
            let install = persist_codex_account_reset_fence(
                &admin_state,
                key_id,
                None,
                None,
                fence_unix_ms,
                fence_id,
                redeem_id,
            )
            .await
            .expect("reset fence should persist")
            .expect("reset fence should be returned");
            assert!(matches!(install, CodexAccountResetFenceInstall::Owned(_)));
            let baseline = json!({"codex": {
                "primary_used_percent": usage,
                "primary_reset_at": 2_000_000_000u64,
                "primary_window_minutes": 300u64
            }});
            assert!(persist_codex_provider_quota_refresh_state(
                &admin_state,
                key_id,
                Some(&baseline),
                admin_provider_quota_pure::CodexQuotaMergeContext {
                    observed_at_unix_secs: request_started_at_unix_ms / 1_000,
                    request_started_at_unix_ms: Some(request_started_at_unix_ms),
                    request_order_id: Some("reset-refresh"),
                    observed_reset_generation: Some(0),
                    authoritative_reset_generation: None,
                    observed_credential_generation: None,
                    account_reset_fence_id: Some(fence_id),
                    coverage: admin_provider_quota_pure::CodexQuotaWindowCoverage::AccountSnapshot,
                },
            )
            .await
            .expect("reset baseline should persist"));
        }

        let replay = persist_codex_account_reset_fence(
            &admin_state,
            key_id,
            None,
            None,
            350_000,
            "fence-a-replay",
            "redeem-a",
        )
        .await
        .expect("historical replay should be idempotent")
        .expect("historical replay should return an install result");
        assert_eq!(replay, CodexAccountResetFenceInstall::Superseded);

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(codex["primary_used_percent"], json!(0.0));
        assert_eq!(codex["account_quota_reset_fence_id"], json!("fence-b"));
        assert_eq!(codex["account_quota_reset_pending"], json!(false));
        assert_eq!(
            codex["account_quota_reset_processed_ids"],
            json!(["redeem-a", "redeem-b"])
        );
    }

    #[tokio::test]
    async fn fenced_stale_codex_refresh_keeps_usage_and_oauth_state() {
        let key_id = "key-codex-fenced-refresh-monotonic";
        let (app, repository, credential_fence) = codex_refresh_test_state(key_id, Some("auth-v1"));
        let credential_fence = credential_fence.expect("credential fence should exist");
        let admin_state = AdminAppState::new(&app);
        let stale_refresh = json!({"codex": {
            "primary_used_percent": 50.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});

        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&stale_refresh),
            Some(300),
            Some("refresh-state".to_string()),
            codex_merge_context(100_000),
            &credential_fence.credential,
        )
        .await
        .expect("fenced refresh persistence should complete"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["codex"]["primary_used_percent"],
            json!(60.0)
        );
        assert_eq!(stored.oauth_invalid_at_unix_secs, None);
        assert_eq!(stored.oauth_invalid_reason, None);
    }

    #[tokio::test]
    async fn fenced_older_refresh_cannot_overwrite_newer_oauth_state() {
        let key_id = "key-codex-fenced-oauth-watermark";
        let (app, repository, credential_fence) = codex_refresh_test_state(key_id, Some("auth-v1"));
        let credential_fence = credential_fence.expect("credential fence should exist");
        let admin_state = AdminAppState::new(&app);
        let quota = |used_percent| {
            json!({"codex": {
                "primary_used_percent": used_percent,
                "primary_reset_at": 2_000_000_000u64,
                "primary_window_minutes": 300u64
            }})
        };

        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota(70.0)),
            None,
            None,
            codex_merge_context(300_000),
            &credential_fence.credential,
        )
        .await
        .expect("newer refresh should persist"));
        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota(65.0)),
            Some(250),
            Some("stale-invalid".to_string()),
            codex_merge_context(250_000),
            &credential_fence.credential,
        )
        .await
        .expect("older refresh should merge without replacing OAuth state"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.oauth_invalid_at_unix_secs, None);
        assert_eq!(stored.oauth_invalid_reason, None);
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["codex"]
                [CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY],
            json!(300_000u64)
        );
    }

    #[tokio::test]
    async fn fenced_same_millisecond_refresh_uses_request_id_for_oauth_order() {
        let key_id = "key-codex-fenced-oauth-id-watermark";
        let (app, repository, credential_fence) = codex_refresh_test_state(key_id, Some("auth-v1"));
        let credential_fence = credential_fence.expect("credential fence should exist");
        let admin_state = AdminAppState::new(&app);
        let quota = json!({"codex": {
            "primary_used_percent": 70.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});

        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            None,
            None,
            codex_merge_context_with_id(300_000, Some("request-b")),
            &credential_fence.credential,
        )
        .await
        .expect("newer same-millisecond refresh should persist"));
        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            Some(300),
            Some("stale-invalid".to_string()),
            codex_merge_context_with_id(300_000, Some("request-a")),
            &credential_fence.credential,
        )
        .await
        .expect("older same-millisecond refresh should merge without replacing OAuth state"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.oauth_invalid_at_unix_secs, None);
        assert_eq!(stored.oauth_invalid_reason, None);
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(
            codex[CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY],
            json!(300_000u64)
        );
        assert_eq!(
            codex[CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY],
            json!("request-b")
        );
    }

    #[tokio::test]
    async fn fenced_same_millisecond_newer_request_id_can_replace_oauth_state() {
        let key_id = "key-codex-fenced-oauth-id-watermark-newer-invalid";
        let (app, repository, credential_fence) = codex_refresh_test_state(key_id, Some("auth-v1"));
        let credential_fence = credential_fence.expect("credential fence should exist");
        let admin_state = AdminAppState::new(&app);
        let quota = json!({"codex": {
            "primary_used_percent": 70.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});

        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            None,
            None,
            codex_merge_context_with_id(300_000, Some("request-a")),
            &credential_fence.credential,
        )
        .await
        .expect("older same-millisecond refresh should persist"));
        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            Some(300),
            Some("newer-invalid".to_string()),
            codex_merge_context_with_id(300_000, Some("request-b")),
            &credential_fence.credential,
        )
        .await
        .expect("newer same-millisecond refresh should replace OAuth state"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.oauth_invalid_at_unix_secs, Some(300));
        assert_eq!(
            stored.oauth_invalid_reason.as_deref(),
            Some("newer-invalid")
        );
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(
            codex[CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY],
            json!(300_000u64)
        );
        assert_eq!(
            codex[CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY],
            json!("request-b")
        );
    }

    #[tokio::test]
    async fn fenced_older_success_cannot_clear_newer_oauth_invalid_state() {
        let key_id = "key-codex-newer-invalid-older-success";
        let (app, repository, credential_fence) = codex_refresh_test_state(key_id, Some("auth-v1"));
        let credential_fence = credential_fence.expect("credential fence should exist");
        let admin_state = AdminAppState::new(&app);
        let quota = json!({"codex": {
            "primary_used_percent": 70.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});

        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            Some(300),
            Some("newer-invalid".to_string()),
            codex_merge_context_with_id(300_000, Some("request-newer")),
            &credential_fence.credential,
        )
        .await
        .expect("newer invalid response should persist"));
        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            None,
            None,
            codex_merge_context_with_id(250_000, Some("request-older")),
            &credential_fence.credential,
        )
        .await
        .expect("older success should be harmlessly acknowledged"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.oauth_invalid_at_unix_secs, Some(300));
        assert_eq!(
            stored.oauth_invalid_reason.as_deref(),
            Some("newer-invalid")
        );
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(
            codex[CODEX_OAUTH_STATE_REQUEST_WATERMARK_KEY],
            json!(300_000u64)
        );
        assert_eq!(
            codex[CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY],
            json!("request-newer")
        );
    }

    #[tokio::test]
    async fn fenced_same_millisecond_older_success_cannot_clear_newer_invalid_state() {
        let key_id = "key-codex-same-ms-newer-invalid-older-success";
        let (app, repository, credential_fence) = codex_refresh_test_state(key_id, Some("auth-v1"));
        let credential_fence = credential_fence.expect("credential fence should exist");
        let admin_state = AdminAppState::new(&app);
        let quota = json!({"codex": {
            "primary_used_percent": 70.0,
            "primary_reset_at": 2_000_000_000u64,
            "primary_window_minutes": 300u64
        }});

        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            Some(300),
            Some("newer-invalid".to_string()),
            codex_merge_context_with_id(300_000, Some("request-b")),
            &credential_fence.credential,
        )
        .await
        .expect("newer same-millisecond invalid response should persist"));
        assert!(persist_fenced_provider_quota_refresh_state(
            &admin_state,
            key_id,
            credential_fence.encrypted_auth_config.as_str(),
            Some(&quota),
            None,
            None,
            codex_merge_context_with_id(300_000, Some("request-a")),
            &credential_fence.credential,
        )
        .await
        .expect("older same-millisecond success should be acknowledged"));

        let stored = repository
            .list_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.oauth_invalid_at_unix_secs, Some(300));
        assert_eq!(
            stored.oauth_invalid_reason.as_deref(),
            Some("newer-invalid")
        );
        let codex = &stored.upstream_metadata.as_ref().unwrap()["codex"];
        assert_eq!(
            codex[CODEX_OAUTH_STATE_REQUEST_WATERMARK_ID_KEY],
            json!("request-b")
        );
    }

    #[tokio::test]
    async fn metadata_cas_conflict_does_not_persist_stale_oauth_runtime_state() {
        let provider_id = "provider-codex-cas";
        let key_id = "key-codex-cas";
        let bootstrap = AppState::new()
            .expect("bootstrap app should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let old_auth_config = bootstrap
            .seal_provider_catalog_key_auth_config(provider_id, key_id, "old-auth-config")
            .expect("old auth config should seal");
        let new_auth_config = bootstrap
            .seal_provider_catalog_key_auth_config(provider_id, key_id, "new-auth-config")
            .expect("new auth config should seal");
        let mut key = StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            "Codex CAS".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.encrypted_auth_config = Some(old_auth_config.clone());
        key.oauth_invalid_at_unix_secs = Some(100);
        key.oauth_invalid_reason = Some("old-invalid-reason".to_string());
        key.upstream_metadata = Some(json!({"codex":{"remaining":5}}));
        key.status_snapshot = Some(json!({"oauth":{"invalid":true}}));

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![StoredProviderCatalogProvider::new(
                provider_id.to_string(),
                "Codex CAS".to_string(),
                None,
                "codex".to_string(),
            )
            .expect("provider should build")],
            vec![],
            vec![key],
        ));
        let app = AppState::new()
            .expect("app should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let admin_state = AdminAppState::new(&app);
        let credential_fence = ProviderTransportCredentialFence {
            encrypted_auth_config: old_auth_config.clone(),
            credential: ProviderCatalogKeyOAuthCredentialFence {
                encrypted_api_key: None,
                auth_type: "oauth".to_string(),
                provider_id: provider_id.to_string(),
                provider_type: "codex".to_string(),
            },
        };
        let concurrent_repository = Arc::clone(&repository);
        let metadata_update = json!({"codex":{"remaining":3}});

        let persisted = persist_provider_quota_refresh_state_after_read(
            &admin_state,
            key_id,
            Some(&metadata_update),
            Some(200),
            Some("new-invalid-reason".to_string()),
            Some(new_auth_config),
            Some(&credential_fence),
            async move {
                assert!(concurrent_repository
                    .update_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
                        key_id: "key-codex-cas".to_string(),
                        namespace: "codex".to_string(),
                        expected_upstream_metadata_value: Some(json!({"remaining":5})),
                        upstream_metadata_value: json!({"remaining":4}),
                        status_snapshot_patch: json!({}),
                        updated_at_unix_secs: Some(150),
                    })
                    .await
                    .expect("concurrent metadata update should execute"));
            },
        )
        .await
        .expect("quota refresh persistence should not error");

        assert!(!persisted, "stale namespace should report a CAS conflict");
        let stored = repository
            .list_keys_by_ids(&["key-codex-cas".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should remain");
        assert_eq!(
            stored.encrypted_auth_config.as_deref(),
            Some(old_auth_config.as_str())
        );
        assert_eq!(stored.oauth_invalid_at_unix_secs, Some(100));
        assert_eq!(
            stored.oauth_invalid_reason.as_deref(),
            Some("old-invalid-reason")
        );
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["codex"],
            json!({"remaining":4})
        );
    }

    #[tokio::test]
    async fn quota_refresh_strong_read_bypasses_stale_provider_catalog_cache() {
        let mut key = StoredProviderCatalogKey::new(
            "key-antigravity-stale-cache".to_string(),
            "provider-antigravity-stale-cache".to_string(),
            "Antigravity stale cache".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.upstream_metadata = Some(json!({
            "antigravity": {
                "project_id": "project-1",
                "quota_by_model": {
                    "gemini-3.7-flash-tiered": {"remaining_fraction": 0.9}
                }
            }
        }));

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![],
            vec![],
            vec![key],
        ));
        let data =
            GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(&repository))
                .with_cached_provider_catalog_reader_for_tests(Arc::clone(&repository));
        let app = AppState::new()
            .expect("app should build")
            .with_data_state_for_tests(data);
        let admin_state = AdminAppState::new(&app);
        let key_ids = ["key-antigravity-stale-cache".to_string()];

        let cached = app
            .read_provider_catalog_keys_by_ids(&key_ids)
            .await
            .expect("initial cached read should succeed");
        assert_eq!(
            cached[0].upstream_metadata.as_ref().unwrap()["antigravity"]["quota_by_model"]
                ["gemini-3.7-flash-tiered"]["remaining_fraction"],
            json!(0.9)
        );

        let current_namespace = json!({
            "project_id": "project-1",
            "model_fetch_revision": 2,
            "quota_by_model": {
                "gemini-3.7-flash-tiered": {"remaining_fraction": 0.7}
            }
        });
        assert!(repository
            .upsert_key_upstream_metadata_namespace(
                "key-antigravity-stale-cache",
                "antigravity",
                &current_namespace,
                None,
            )
            .await
            .expect("out-of-band metadata update should succeed"));
        let still_cached = app
            .read_provider_catalog_keys_by_ids(&key_ids)
            .await
            .expect("stale cached read should succeed");
        assert_eq!(
            still_cached[0].upstream_metadata.as_ref().unwrap()["antigravity"]["quota_by_model"]
                ["gemini-3.7-flash-tiered"]["remaining_fraction"],
            json!(0.9),
            "regression setup must keep the ordinary read stale"
        );

        let metadata_update = json!({
            "antigravity": {
                "project_id": "project-1",
                "quota_by_model": {
                    "gemini-3.7-flash-tiered": {"remaining_fraction": 0.6}
                },
                "quota_groups": [{
                    "display_name": "Gemini models",
                    "buckets": [{"bucket_id": "gemini-weekly", "window": "weekly"}]
                }]
            }
        });
        assert!(persist_provider_quota_refresh_state(
            &admin_state,
            "key-antigravity-stale-cache",
            Some(&metadata_update),
            None,
            None,
            None,
        )
        .await
        .expect("quota refresh persistence should not error"));

        let stored = repository
            .list_keys_by_ids(&key_ids)
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["antigravity"]["quota_groups"][0]["buckets"]
                [0]["bucket_id"],
            json!("gemini-weekly")
        );
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["antigravity"]["model_fetch_revision"],
            json!(2),
            "quota refresh must preserve fields written by another Antigravity metadata producer"
        );
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["antigravity"]["quota_by_model"]
                ["gemini-3.7-flash-tiered"]["remaining_fraction"],
            json!(0.6)
        );
    }
}
