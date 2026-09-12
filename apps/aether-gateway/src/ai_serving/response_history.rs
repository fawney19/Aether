use crate::ai_serving::{
    hydrate_response_history, normalize_api_format_alias, record_converted_response_history,
    response_history_is_loaded, response_history_storage_key, ResponseHistoryRecord,
};
use serde_json::Value;
use tracing::warn;

use crate::{AppState, GatewayError};

const RESPONSE_HISTORY_SECRET_PURPOSE: &str = "openai-response-history";

pub(crate) async fn hydrate_openai_response_history(
    state: &AppState,
    request: &Value,
    client_api_format: &str,
    provider_api_format: &str,
    history_scope: &str,
) -> Result<(), GatewayError> {
    if normalize_api_format_alias(client_api_format) != "openai:responses"
        || normalize_api_format_alias(provider_api_format) != "openai:chat"
    {
        return Ok(());
    }
    let Some(previous_response_id) = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if response_history_is_loaded(previous_response_id, Some(history_scope)) {
        return Ok(());
    }

    let storage_key = response_history_storage_key(previous_response_id, Some(history_scope));
    let runtime_state = state.runtime_state();
    let payload = runtime_state.kv_get(&storage_key).await.map_err(|error| {
        warn!(
            event_name = "openai_response_history_read_failed",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = ?error,
            "gateway failed to read shared OpenAI response history"
        );
        GatewayError::Internal("OpenAI response history lookup failed".to_string())
    })?;
    let Some(payload) = payload else {
        return Ok(());
    };
    let Some(payload) = crate::handlers::shared::open_runtime_secret_payload(
        state,
        RESPONSE_HISTORY_SECRET_PURPOSE,
        &payload,
    ) else {
        let _ = runtime_state.kv_delete(&storage_key).await;
        warn!(
            event_name = "openai_response_history_decryption_failed",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            "gateway rejected undecryptable shared OpenAI response history"
        );
        return Err(GatewayError::Internal(
            "OpenAI response history decryption failed".to_string(),
        ));
    };
    if let Err(error) =
        hydrate_response_history(previous_response_id, Some(history_scope), payload.as_str())
    {
        let _ = runtime_state.kv_delete(&storage_key).await;
        warn!(
            event_name = "openai_response_history_invalid",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = %error,
            "gateway rejected invalid shared OpenAI response history"
        );
        return Err(GatewayError::Internal(
            "OpenAI response history validation failed".to_string(),
        ));
    }
    Ok(())
}

pub(crate) async fn persist_response_history_record(
    state: &AppState,
    record: ResponseHistoryRecord,
) {
    let runtime_state = state.runtime_state();
    let Some(sealed_payload) = crate::handlers::shared::seal_runtime_secret_payload(
        state,
        RESPONSE_HISTORY_SECRET_PURPOSE,
        &record.payload,
    ) else {
        warn!(
            event_name = "openai_response_history_encryption_unavailable",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            "gateway refused to persist unencrypted OpenAI response history"
        );
        return;
    };
    if let Err(error) = runtime_state
        .kv_set(&record.storage_key, sealed_payload, Some(record.ttl))
        .await
    {
        warn!(
            event_name = "openai_response_history_write_failed",
            log_type = "ops",
            backend = runtime_state.backend_kind().as_str(),
            error = ?error,
            "gateway failed to persist shared OpenAI response history"
        );
    }
}

pub(crate) async fn persist_converted_response_history(
    state: &AppState,
    report_context: &Value,
    response: Option<&Value>,
) {
    let Some(response) = response else {
        return;
    };
    if let Some(record) = record_converted_response_history(report_context, response) {
        persist_response_history_record(state, record).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::{
        hydrate_openai_response_history, persist_response_history_record, ResponseHistoryRecord,
    };
    use crate::{ai_serving::response_history_storage_key, data::GatewayDataState, AppState};

    fn response_history_test_state() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_runtime_state(Arc::new(RuntimeState::memory(
                MemoryRuntimeStateConfig::default(),
            )))
    }

    fn response_history_payload(response_id: &str, scope: &str, marker: &str) -> String {
        let expires_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(3600);
        json!({
            "version": 1,
            "response_id": response_id,
            "scope_fingerprint": format!("{:x}", Sha256::digest(scope.trim().as_bytes())),
            "expires_at_unix_secs": expires_at_unix_secs,
            "transcript": [{"type": "message", "content": marker}],
        })
        .to_string()
    }

    #[tokio::test]
    async fn response_history_is_encrypted_at_rest_and_hydrates() {
        let state = response_history_test_state();
        let response_id = "resp_gateway_encrypted_history_v1";
        let scope = "response-history-encrypted-scope";
        let marker = "private-response-history-marker";
        let storage_key = response_history_storage_key(response_id, Some(scope));
        let payload = response_history_payload(response_id, scope, marker);

        persist_response_history_record(
            &state,
            ResponseHistoryRecord {
                storage_key: storage_key.clone(),
                payload,
                ttl: Duration::from_secs(6 * 60 * 60),
            },
        )
        .await;

        let stored = state
            .runtime_kv_get(&storage_key)
            .await
            .expect("history lookup should succeed")
            .expect("history should be persisted");
        assert!(crate::handlers::shared::runtime_secret_payload_is_sealed(
            &stored
        ));
        assert!(!stored.contains(marker));

        hydrate_openai_response_history(
            &state,
            &json!({"previous_response_id": response_id}),
            "openai:responses",
            "openai:chat",
            scope,
        )
        .await
        .expect("encrypted history should hydrate");
        assert!(crate::ai_serving::response_history_is_loaded(
            response_id,
            Some(scope)
        ));
    }

    #[tokio::test]
    async fn response_history_reader_rejects_and_deletes_legacy_plaintext() {
        let state = response_history_test_state();
        let response_id = "resp_gateway_legacy_history_v1";
        let scope = "response-history-legacy-scope";
        let storage_key = response_history_storage_key(response_id, Some(scope));
        let payload = response_history_payload(response_id, scope, "legacy-private-history");
        state
            .runtime_kv_setex(&storage_key, &payload, 6 * 60 * 60)
            .await
            .expect("legacy history should store");

        let result = hydrate_openai_response_history(
            &state,
            &json!({"previous_response_id": response_id}),
            "openai:responses",
            "openai:chat",
            scope,
        )
        .await;
        assert!(result.is_err());
        assert!(!crate::ai_serving::response_history_is_loaded(
            response_id,
            Some(scope)
        ));
        assert!(state
            .runtime_kv_get(&storage_key)
            .await
            .expect("history lookup should succeed")
            .is_none());
    }
}
