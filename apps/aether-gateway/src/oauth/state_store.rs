use crate::{AppState, GatewayError};
use aether_oauth::core::{current_unix_secs, generate_oauth_nonce};
use serde::{Deserialize, Serialize};

const IDENTITY_OAUTH_STATE_TTL_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityOAuthStateMode {
    Login,
    Bind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredIdentityOAuthState {
    pub(crate) nonce: String,
    pub(crate) provider_type: String,
    pub(crate) mode: IdentityOAuthStateMode,
    pub(crate) client_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pkce_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bind_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) bind_session_id: Option<String>,
    pub(crate) created_at: u64,
}

impl StoredIdentityOAuthState {
    pub(crate) fn login(
        provider_type: impl Into<String>,
        client_device_id: impl Into<String>,
        pkce_verifier: Option<String>,
    ) -> Self {
        Self {
            nonce: generate_oauth_nonce(),
            provider_type: provider_type.into(),
            mode: IdentityOAuthStateMode::Login,
            client_device_id: client_device_id.into(),
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
        user_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            nonce: generate_oauth_nonce(),
            provider_type: provider_type.into(),
            mode: IdentityOAuthStateMode::Bind,
            client_device_id: client_device_id.into(),
            pkce_verifier,
            bind_user_id: Some(user_id.into()),
            bind_session_id: Some(session_id.into()),
            created_at: current_unix_secs(),
        }
    }
}

pub(crate) fn identity_oauth_state_storage_key(nonce: &str) -> String {
    format!("identity_oauth_state:{}", nonce.trim())
}

pub(crate) async fn save_identity_oauth_state(
    state: &AppState,
    record: &StoredIdentityOAuthState,
) -> Result<(), GatewayError> {
    let key = identity_oauth_state_storage_key(&record.nonce);
    let value =
        serde_json::to_string(record).map_err(|err| GatewayError::Internal(err.to_string()))?;
    if let Some(runner) = state.redis_kv_runner() {
        runner
            .setex(&key, &value, Some(IDENTITY_OAUTH_STATE_TTL_SECS))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(());
    }
    if state.save_provider_oauth_state_for_tests(&key, &value) {
        return Ok(());
    }
    Err(GatewayError::Internal(
        "identity oauth state store unavailable".to_string(),
    ))
}

pub(crate) async fn consume_identity_oauth_state(
    state: &AppState,
    nonce: &str,
) -> Result<Option<StoredIdentityOAuthState>, GatewayError> {
    let key = identity_oauth_state_storage_key(nonce);
    let raw = if let Some(runner) = state.redis_kv_runner() {
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let namespaced_key = runner.keyspace().key(&key);
        redis::cmd("GETDEL")
            .arg(&namespaced_key)
            .query_async::<Option<String>>(&mut connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
    } else {
        state.take_provider_oauth_state_for_tests(&key)
    };
    raw.map(|value| {
        serde_json::from_str::<StoredIdentityOAuthState>(&value)
            .map_err(|err| GatewayError::Internal(err.to_string()))
    })
    .transpose()
}
