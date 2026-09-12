use crate::{AppState, GatewayError};
use aether_oauth::core::{current_unix_secs, generate_oauth_nonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const IDENTITY_OAUTH_STATE_TTL_SECS: u64 = 10 * 60;
const IDENTITY_OAUTH_STATE_SECRET_PURPOSE: &str = "identity-oauth-state";
const IDENTITY_OAUTH_STATE_MAX_CLOCK_SKEW_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityOAuthStateMode {
    Login,
    Bind,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredIdentityOAuthState {
    pub(crate) nonce: String,
    pub(crate) provider_type: String,
    pub(crate) mode: IdentityOAuthStateMode,
    pub(crate) client_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) browser_binding_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pkce_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bind_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bind_session_id: Option<String>,
    pub(crate) created_at: u64,
}

impl std::fmt::Debug for StoredIdentityOAuthState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredIdentityOAuthState")
            .field("nonce", &"[REDACTED]")
            .field("provider_type", &self.provider_type)
            .field("mode", &self.mode)
            .field("client_device_id", &"[REDACTED]")
            .field("browser_binding_hash", &"[REDACTED]")
            .field("pkce_verifier", &"[REDACTED]")
            .field("bind_user_id", &"[REDACTED]")
            .field("bind_session_id", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl StoredIdentityOAuthState {
    pub(crate) fn login(
        provider_type: impl Into<String>,
        client_device_id: impl Into<String>,
        pkce_verifier: Option<String>,
        browser_binding_hash: Option<String>,
    ) -> Self {
        Self {
            nonce: generate_oauth_nonce(),
            provider_type: provider_type.into(),
            mode: IdentityOAuthStateMode::Login,
            client_device_id: client_device_id.into(),
            browser_binding_hash,
            pkce_verifier,
            bind_user_id: None,
            bind_session_id: None,
            created_at: current_unix_secs(),
        }
    }

    pub(crate) fn bind(
        provider_type: impl Into<String>,
        client_device_id: impl Into<String>,
        pkce_verifier: Option<String>,
        browser_binding_hash: String,
        user_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            nonce: generate_oauth_nonce(),
            provider_type: provider_type.into(),
            mode: IdentityOAuthStateMode::Bind,
            client_device_id: client_device_id.into(),
            browser_binding_hash: Some(browser_binding_hash),
            pkce_verifier,
            bind_user_id: Some(user_id.into()),
            bind_session_id: Some(session_id.into()),
            created_at: current_unix_secs(),
        }
    }
}

pub(crate) fn identity_oauth_state_storage_key(nonce: &str) -> String {
    format!(
        "identity_oauth_state:sha256:{:x}",
        Sha256::digest(nonce.trim().as_bytes())
    )
}

fn identity_oauth_state_secret_purpose(nonce: &str) -> String {
    format!(
        "{IDENTITY_OAUTH_STATE_SECRET_PURPOSE}:sha256:{:x}",
        Sha256::digest(nonce.trim().as_bytes())
    )
}

fn legacy_identity_oauth_state_storage_key(nonce: &str) -> String {
    format!("identity_oauth_state:{}", nonce.trim())
}

fn is_generated_oauth_nonce(nonce: &str) -> bool {
    let nonce = nonce.trim();
    nonce.len() == 64
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) async fn save_identity_oauth_state(
    state: &AppState,
    record: &StoredIdentityOAuthState,
) -> Result<(), GatewayError> {
    let key = identity_oauth_state_storage_key(&record.nonce);
    let plaintext =
        serde_json::to_string(record).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let purpose = identity_oauth_state_secret_purpose(&record.nonce);
    let value = crate::handlers::shared::seal_runtime_secret_payload(state, &purpose, &plaintext)
        .ok_or_else(|| {
        GatewayError::Internal("identity OAuth state encryption unavailable".to_string())
    })?;
    state
        .runtime_kv_setex(&key, &value, IDENTITY_OAUTH_STATE_TTL_SECS)
        .await
}

pub(crate) async fn consume_identity_oauth_state(
    state: &AppState,
    nonce: &str,
) -> Result<Option<StoredIdentityOAuthState>, GatewayError> {
    let key = identity_oauth_state_storage_key(nonce);
    let raw = match state.runtime_kv_getdel(&key).await? {
        Some(value) => Some(value),
        None if is_generated_oauth_nonce(nonce) => {
            state
                .runtime_kv_getdel(&legacy_identity_oauth_state_storage_key(nonce))
                .await?
        }
        None => None,
    };
    raw.map(|value| decode_identity_oauth_state(state, nonce, &value))
        .transpose()
}

pub(crate) async fn load_identity_oauth_state(
    state: &AppState,
    nonce: &str,
) -> Result<Option<StoredIdentityOAuthState>, GatewayError> {
    let key = identity_oauth_state_storage_key(nonce);
    let raw = match state.runtime_kv_get(&key).await? {
        Some(value) => Some(value),
        None if is_generated_oauth_nonce(nonce) => {
            state
                .runtime_kv_get(&legacy_identity_oauth_state_storage_key(nonce))
                .await?
        }
        None => None,
    };
    raw.map(|value| decode_identity_oauth_state(state, nonce, &value))
        .transpose()
}

fn decode_identity_oauth_state(
    state: &AppState,
    expected_nonce: &str,
    stored: &str,
) -> Result<StoredIdentityOAuthState, GatewayError> {
    let expected_nonce = expected_nonce.trim();
    let purpose = identity_oauth_state_secret_purpose(expected_nonce);
    let plaintext = crate::handlers::shared::open_runtime_secret_payload(state, &purpose, stored)
        // States created immediately before a rolling upgrade still live under the
        // legacy key and were sealed with the fixed purpose.  They remain safe to
        // accept for their short TTL because the decoded record is checked against
        // the callback nonce and all authority-bearing fields below.
        .or_else(|| {
            crate::handlers::shared::open_runtime_secret_payload(
                state,
                IDENTITY_OAUTH_STATE_SECRET_PURPOSE,
                stored,
            )
        })
        .ok_or_else(|| GatewayError::Internal("identity OAuth state is invalid".to_string()))?;
    let record = serde_json::from_str::<StoredIdentityOAuthState>(&plaintext)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    validate_identity_oauth_state(expected_nonce, &record)?;
    Ok(record)
}

fn validate_identity_oauth_state(
    expected_nonce: &str,
    record: &StoredIdentityOAuthState,
) -> Result<(), GatewayError> {
    let invalid = || GatewayError::Internal("identity OAuth state is invalid".to_string());
    if !is_generated_oauth_nonce(expected_nonce)
        || record.nonce != expected_nonce
        || !is_generated_oauth_nonce(&record.nonce)
        || record.provider_type.trim().is_empty()
        || record.provider_type != record.provider_type.trim().to_ascii_lowercase()
        || record.client_device_id.trim().is_empty()
        || !record
            .browser_binding_hash
            .as_deref()
            .is_some_and(is_lower_hex_sha256)
        || !record
            .pkce_verifier
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(invalid());
    }

    let mode_is_valid = match record.mode {
        IdentityOAuthStateMode::Login => {
            record.bind_user_id.is_none() && record.bind_session_id.is_none()
        }
        IdentityOAuthStateMode::Bind => {
            record
                .bind_user_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                && record
                    .bind_session_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        }
    };
    let now = current_unix_secs();
    let time_is_valid = record.created_at
        <= now.saturating_add(IDENTITY_OAUTH_STATE_MAX_CLOCK_SKEW_SECS)
        && now.saturating_sub(record.created_at) <= IDENTITY_OAUTH_STATE_TTL_SECS;
    if !mode_is_valid || !time_is_valid {
        return Err(invalid());
    }
    Ok(())
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{
        decode_identity_oauth_state, identity_oauth_state_secret_purpose, StoredIdentityOAuthState,
    };
    use crate::{data::GatewayDataState, AppState};
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;

    fn state_with_encryption_key() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    #[test]
    fn identity_oauth_state_ciphertext_is_bound_to_its_nonce() {
        let state = state_with_encryption_key();
        let record = StoredIdentityOAuthState::login(
            "linuxdo",
            "device-1",
            Some("pkce-verifier".to_string()),
            Some("a".repeat(64)),
        );
        let plaintext = serde_json::to_string(&record).expect("state should serialize");
        let sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &state,
            &identity_oauth_state_secret_purpose(&record.nonce),
            &plaintext,
        )
        .expect("state should seal");

        assert_eq!(
            decode_identity_oauth_state(&state, &record.nonce, &sealed)
                .expect("matching state should open"),
            record
        );
        assert!(decode_identity_oauth_state(&state, &"b".repeat(64), &sealed).is_err());
    }

    #[test]
    fn identity_oauth_state_debug_redacts_authorization_material() {
        let record = StoredIdentityOAuthState::login(
            "linuxdo",
            "debug-secret-device",
            Some("debug-secret-pkce".to_string()),
            Some("debug-secret-binding-hash".to_string()),
        );
        let nonce = record.nonce.clone();
        let rendered = format!("{record:?}");

        for secret in [
            nonce.as_str(),
            "debug-secret-device",
            "debug-secret-pkce",
            "debug-secret-binding-hash",
        ] {
            assert!(!rendered.contains(secret), "Debug output leaked {secret}");
        }
        assert!(rendered.contains("[REDACTED]"));
        assert!(rendered.contains("linuxdo"));
    }

    #[test]
    fn identity_oauth_state_accepts_valid_legacy_ciphertext_during_ttl_window() {
        let state = state_with_encryption_key();
        let record = StoredIdentityOAuthState::login(
            "linuxdo",
            "device-1",
            Some("pkce-verifier".to_string()),
            Some("a".repeat(64)),
        );
        let plaintext = serde_json::to_string(&record).expect("state should serialize");
        let sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &state,
            super::IDENTITY_OAUTH_STATE_SECRET_PURPOSE,
            &plaintext,
        )
        .expect("legacy state should seal");

        assert_eq!(
            decode_identity_oauth_state(&state, &record.nonce, &sealed)
                .expect("valid legacy state should open"),
            record
        );
        assert!(decode_identity_oauth_state(&state, &"b".repeat(64), &sealed).is_err());
    }

    #[test]
    fn identity_oauth_state_rejects_mode_field_confusion() {
        let state = state_with_encryption_key();
        let mut record = StoredIdentityOAuthState::login(
            "linuxdo",
            "device-1",
            Some("pkce-verifier".to_string()),
            Some("a".repeat(64)),
        );
        record.bind_user_id = Some("unexpected-user".to_string());
        let plaintext = serde_json::to_string(&record).expect("state should serialize");
        let sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &state,
            &identity_oauth_state_secret_purpose(&record.nonce),
            &plaintext,
        )
        .expect("state should seal");

        assert!(decode_identity_oauth_state(&state, &record.nonce, &sealed).is_err());
    }
}
