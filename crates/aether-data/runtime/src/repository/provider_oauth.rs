use sha2::{Digest, Sha256};

const KIRO_DEVICE_AUTH_SESSION_PREFIX: &str = "device_auth_session:";
const PROVIDER_OAUTH_BATCH_TASK_PREFIX: &str = "provider_oauth_batch_task:";
const PROVIDER_OAUTH_STATE_PREFIX: &str = "provider_oauth_state:";

pub const KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS: u64 = 60;
pub const PROVIDER_OAUTH_BATCH_TASK_TTL_SECS: u64 = 24 * 60 * 60;
pub const PROVIDER_OAUTH_STATE_TTL_SECS: u64 = 600;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminProviderOAuthDeviceSession {
    pub session_id: String,
    pub provider_id: String,
    pub initiated_by_user_id: String,
    pub initiated_by_session_id: Option<String>,
    pub initiated_by_management_token_id: Option<String>,
    pub region: String,
    pub client_id: String,
    pub client_secret: String,
    pub device_code: String,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
    pub social_provider: Option<String>,
    #[serde(default)]
    pub code_verifier: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub machine_id: Option<String>,
    pub interval: u64,
    pub expires_at_unix_secs: u64,
    pub status: String,
    pub proxy_node_id: Option<String>,
    pub created_at_unix_ms: u64,
    pub key_id: Option<String>,
    pub email: Option<String>,
    pub replaced: bool,
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminProviderOAuthState {
    pub nonce: String,
    pub key_id: String,
    pub provider_id: String,
    pub provider_type: String,
    pub pkce_verifier: Option<String>,
    #[serde(default)]
    pub expected_encrypted_auth_config: Option<String>,
    pub initiated_by_user_id: String,
    #[serde(default)]
    pub initiated_by_session_id: Option<String>,
    #[serde(default)]
    pub initiated_by_management_token_id: Option<String>,
    pub created_at: u64,
}

pub fn provider_oauth_device_session_storage_key(session_id: &str) -> String {
    format!("{KIRO_DEVICE_AUTH_SESSION_PREFIX}{session_id}")
}

pub fn provider_oauth_device_session_secret_purpose(session_id: &str) -> String {
    let storage_key = provider_oauth_device_session_storage_key(session_id);
    format!(
        "provider-oauth-device-session:sha256:{:x}",
        Sha256::digest(storage_key.as_bytes())
    )
}

pub fn provider_oauth_state_storage_key(nonce: &str) -> String {
    format!(
        "{PROVIDER_OAUTH_STATE_PREFIX}sha256:{:x}",
        Sha256::digest(nonce.as_bytes())
    )
}

pub fn provider_oauth_batch_task_storage_key(task_id: &str) -> String {
    format!("{PROVIDER_OAUTH_BATCH_TASK_PREFIX}{task_id}")
}

pub fn provider_oauth_batch_task_secret_purpose(task_id: &str) -> String {
    let storage_key = provider_oauth_batch_task_storage_key(task_id);
    format!(
        "provider-oauth-batch-task:sha256:{:x}",
        Sha256::digest(storage_key.as_bytes())
    )
}

pub fn build_provider_oauth_batch_task_status_payload(
    provider_id: &str,
    state: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let now_unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let raw_status = state
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("failed");
    let normalized_status = match raw_status {
        "submitted" | "processing" | "completed" | "failed" => raw_status,
        _ => "failed",
    };
    let import_kind = match state.get("import_kind").and_then(serde_json::Value::as_str) {
        Some("oauth_batch" | "agent_identity" | "cookie_authorize") => state
            .get("import_kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        _ => "",
    };
    let error_samples = state
        .get("error_samples")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.is_object())
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    serde_json::json!({
        "task_id": state
            .get("task_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        "provider_id": provider_id,
        "provider_type": state
            .get("provider_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        "import_kind": import_kind,
        "status": normalized_status,
        "total": state.get("total").and_then(serde_json::Value::as_i64).unwrap_or(0),
        "processed": state.get("processed").and_then(serde_json::Value::as_i64).unwrap_or(0),
        "success": state.get("success").and_then(serde_json::Value::as_i64).unwrap_or(0),
        "failed": state.get("failed").and_then(serde_json::Value::as_i64).unwrap_or(0),
        "created_count": state
            .get("created_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        "replaced_count": state
            .get("replaced_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        "progress_percent": state
            .get("progress_percent")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .clamp(0, 100),
        "message": state.get("message").cloned().unwrap_or(serde_json::Value::Null),
        "error": state.get("error").cloned().unwrap_or(serde_json::Value::Null),
        "error_samples": error_samples,
        "created_at": state
            .get("created_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(now_unix_secs),
        "started_at": state.get("started_at").cloned().unwrap_or(serde_json::Value::Null),
        "finished_at": state
            .get("finished_at")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "updated_at": state
            .get("updated_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(now_unix_secs),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_provider_oauth_batch_task_status_payload, provider_oauth_batch_task_secret_purpose,
        provider_oauth_batch_task_storage_key, provider_oauth_device_session_secret_purpose,
        provider_oauth_device_session_storage_key, provider_oauth_state_storage_key,
        KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS, PROVIDER_OAUTH_BATCH_TASK_TTL_SECS,
        PROVIDER_OAUTH_STATE_TTL_SECS,
    };
    use serde_json::json;

    #[test]
    fn builds_provider_oauth_storage_keys_with_expected_prefixes() {
        assert_eq!(
            provider_oauth_device_session_storage_key("session-123"),
            "device_auth_session:session-123"
        );
        let first_purpose = provider_oauth_device_session_secret_purpose("session-123");
        let second_purpose = provider_oauth_device_session_secret_purpose("session-456");
        assert!(first_purpose.starts_with("provider-oauth-device-session:sha256:"));
        assert!(!first_purpose.contains("session-123"));
        assert_ne!(first_purpose, second_purpose);
        let state_key = provider_oauth_state_storage_key("nonce-123");
        assert!(state_key.starts_with("provider_oauth_state:sha256:"));
        assert!(!state_key.contains("nonce-123"));
        assert_eq!(
            provider_oauth_batch_task_storage_key("task-123"),
            "provider_oauth_batch_task:task-123"
        );
        let first_task_purpose = provider_oauth_batch_task_secret_purpose("task-123");
        let second_task_purpose = provider_oauth_batch_task_secret_purpose("task-456");
        assert!(first_task_purpose.starts_with("provider-oauth-batch-task:sha256:"));
        assert!(!first_task_purpose.contains("task-123"));
        assert_ne!(first_task_purpose, second_task_purpose);
    }

    #[test]
    fn provider_oauth_storage_ttls_match_gateway_expectations() {
        assert_eq!(KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS, 60);
        assert_eq!(PROVIDER_OAUTH_BATCH_TASK_TTL_SECS, 24 * 60 * 60);
        assert_eq!(PROVIDER_OAUTH_STATE_TTL_SECS, 600);
    }

    #[test]
    fn batch_task_status_payload_normalizes_status_and_clamps_progress() {
        let input = json!({
            "task_id": "task-123",
            "provider_type": "codex",
            "import_kind": "oauth_batch",
            "status": "weird",
            "total": 4,
            "processed": 2,
            "success": 1,
            "failed": 1,
            "created_count": 0,
            "replaced_count": 1,
            "progress_percent": 999,
            "error_samples": [
                {"detail": "x"},
                "skip-me"
            ],
            "created_at": 1u64,
            "updated_at": 2u64
        });

        let payload = build_provider_oauth_batch_task_status_payload(
            "provider-123",
            input.as_object().expect("input should be object"),
        );

        assert_eq!(
            payload.get("provider_id").and_then(|v| v.as_str()),
            Some("provider-123")
        );
        assert_eq!(
            payload.get("import_kind").and_then(|v| v.as_str()),
            Some("oauth_batch")
        );
        assert_eq!(
            payload.get("status").and_then(|v| v.as_str()),
            Some("failed")
        );
        assert_eq!(
            payload.get("progress_percent").and_then(|v| v.as_i64()),
            Some(100)
        );
        assert_eq!(
            payload.get("created_count").and_then(|v| v.as_i64()),
            Some(0)
        );
        assert_eq!(
            payload.get("replaced_count").and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            payload
                .get("error_samples")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(1)
        );
    }
}
