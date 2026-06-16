#[derive(Debug, Clone, Default)]
pub(crate) struct LocalProviderDeleteTaskState {
    pub task_id: String,
    pub provider_id: String,
    pub status: String,
    pub stage: String,
    pub total_keys: usize,
    pub deleted_keys: usize,
    pub total_endpoints: usize,
    pub deleted_endpoints: usize,
    pub message: String,
}

impl LocalProviderDeleteTaskState {
    pub(crate) fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "running")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LocalMutationOutcome<T> {
    Applied(T),
    NotFound,
    Invalid(String),
    Unavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LocalExecutionRuntimeMissDiagnostic {
    pub(crate) reason: String,
    pub(crate) route_family: Option<String>,
    pub(crate) route_kind: Option<String>,
    pub(crate) public_path: Option<String>,
    pub(crate) plan_kind: Option<String>,
    pub(crate) requested_model: Option<String>,
    pub(crate) candidate_count: Option<usize>,
    pub(crate) skipped_candidate_count: Option<usize>,
    pub(crate) skip_reasons: std::collections::BTreeMap<String, usize>,
}

impl LocalExecutionRuntimeMissDiagnostic {
    pub(crate) fn skip_reasons_summary(&self) -> Option<String> {
        if self.skip_reasons.is_empty() {
            return None;
        }
        Some(
            self.skip_reasons
                .iter()
                .map(|(reason, count)| format!("{reason}={count}"))
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AdminWalletMutationOutcome<T> {
    Applied(T),
    NotFound,
    Invalid(String),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GatewayUserSessionView {
    pub(crate) id: String,
    pub(crate) user_id: String,
    pub(crate) client_device_id: String,
    pub(crate) device_label: Option<String>,
    pub(crate) refresh_token_hash: String,
    pub(crate) prev_refresh_token_hash: Option<String>,
    pub(crate) rotated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) revoke_reason: Option<String>,
    pub(crate) ip_address: Option<String>,
    pub(crate) user_agent: Option<String>,
    pub(crate) created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(crate) updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl GatewayUserSessionView {
    pub(crate) const REFRESH_GRACE_SECONDS: i64 = 10;
    pub(crate) const TOUCH_INTERVAL_SECONDS: i64 = 300;

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: String,
        user_id: String,
        client_device_id: String,
        device_label: Option<String>,
        refresh_token_hash: String,
        prev_refresh_token_hash: Option<String>,
        rotated_at: Option<chrono::DateTime<chrono::Utc>>,
        last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        revoked_at: Option<chrono::DateTime<chrono::Utc>>,
        revoke_reason: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
        created_at: Option<chrono::DateTime<chrono::Utc>>,
        updated_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Self, String> {
        if id.trim().is_empty() {
            return Err("user_sessions.id is empty".to_string());
        }
        if user_id.trim().is_empty() {
            return Err("user_sessions.user_id is empty".to_string());
        }
        if client_device_id.trim().is_empty() {
            return Err("user_sessions.client_device_id is empty".to_string());
        }
        if refresh_token_hash.trim().is_empty() {
            return Err("user_sessions.refresh_token_hash is empty".to_string());
        }

        Ok(Self {
            id,
            user_id,
            client_device_id,
            device_label,
            refresh_token_hash,
            prev_refresh_token_hash,
            rotated_at,
            last_seen_at,
            expires_at,
            revoked_at,
            revoke_reason,
            ip_address,
            user_agent,
            created_at,
            updated_at,
        })
    }

    pub(crate) fn hash_refresh_token(token: &str) -> String {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub(crate) fn verify_refresh_token(
        &self,
        token: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> (bool, bool) {
        let token_hash = Self::hash_refresh_token(token);
        if self.refresh_token_hash == token_hash {
            return (true, false);
        }
        let Some(prev_hash) = self.prev_refresh_token_hash.as_ref() else {
            return (false, false);
        };
        let Some(rotated_at) = self.rotated_at else {
            return (false, false);
        };
        if prev_hash == &token_hash
            && now.signed_duration_since(rotated_at).num_seconds() <= Self::REFRESH_GRACE_SECONDS
        {
            return (true, true);
        }
        (false, false)
    }

    pub(crate) fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub(crate) fn is_expired(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.expires_at.is_none_or(|expires_at| expires_at <= now)
    }

    pub(crate) fn should_touch(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.last_seen_at
            .map(|last_seen_at| {
                now.signed_duration_since(last_seen_at).num_seconds()
                    >= Self::TOUCH_INTERVAL_SECONDS
            })
            .unwrap_or(true)
    }
}

impl From<crate::data::state::StoredUserSessionRecord> for GatewayUserSessionView {
    fn from(value: crate::data::state::StoredUserSessionRecord) -> Self {
        Self {
            id: value.id,
            user_id: value.user_id,
            client_device_id: value.client_device_id,
            device_label: value.device_label,
            refresh_token_hash: value.refresh_token_hash,
            prev_refresh_token_hash: value.prev_refresh_token_hash,
            rotated_at: value.rotated_at,
            last_seen_at: value.last_seen_at,
            expires_at: value.expires_at,
            revoked_at: value.revoked_at,
            revoke_reason: value.revoke_reason,
            ip_address: value.ip_address,
            user_agent: value.user_agent,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<&GatewayUserSessionView> for GatewayUserSessionView {
    fn from(value: &GatewayUserSessionView) -> Self {
        value.clone()
    }
}

impl From<GatewayUserSessionView> for crate::data::state::StoredUserSessionRecord {
    fn from(value: GatewayUserSessionView) -> Self {
        Self {
            id: value.id,
            user_id: value.user_id,
            client_device_id: value.client_device_id,
            device_label: value.device_label,
            refresh_token_hash: value.refresh_token_hash,
            prev_refresh_token_hash: value.prev_refresh_token_hash,
            rotated_at: value.rotated_at,
            last_seen_at: value.last_seen_at,
            expires_at: value.expires_at,
            revoked_at: value.revoked_at,
            revoke_reason: value.revoke_reason,
            ip_address: value.ip_address,
            user_agent: value.user_agent,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct GatewayUserPreferenceView {
    pub(crate) user_id: String,
    pub(crate) avatar_url: Option<String>,
    pub(crate) bio: Option<String>,
    pub(crate) default_provider_id: Option<String>,
    pub(crate) default_provider_name: Option<String>,
    pub(crate) theme: String,
    pub(crate) language: String,
    pub(crate) timezone: String,
    pub(crate) email_notifications: bool,
    pub(crate) usage_alerts: bool,
    pub(crate) announcement_notifications: bool,
}

impl GatewayUserPreferenceView {
    pub(crate) fn default_for_user(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            avatar_url: None,
            bio: None,
            default_provider_id: None,
            default_provider_name: None,
            theme: "light".to_string(),
            language: "zh-CN".to_string(),
            timezone: "Asia/Shanghai".to_string(),
            email_notifications: true,
            usage_alerts: true,
            announcement_notifications: true,
        }
    }
}

impl From<crate::data::state::StoredUserPreferenceRecord> for GatewayUserPreferenceView {
    fn from(value: crate::data::state::StoredUserPreferenceRecord) -> Self {
        Self {
            user_id: value.user_id,
            avatar_url: value.avatar_url,
            bio: value.bio,
            default_provider_id: value.default_provider_id,
            default_provider_name: value.default_provider_name,
            theme: value.theme,
            language: value.language,
            timezone: value.timezone,
            email_notifications: value.email_notifications,
            usage_alerts: value.usage_alerts,
            announcement_notifications: value.announcement_notifications,
        }
    }
}

impl From<&GatewayUserPreferenceView> for GatewayUserPreferenceView {
    fn from(value: &GatewayUserPreferenceView) -> Self {
        value.clone()
    }
}

impl From<GatewayUserPreferenceView> for crate::data::state::StoredUserPreferenceRecord {
    fn from(value: GatewayUserPreferenceView) -> Self {
        Self {
            user_id: value.user_id,
            avatar_url: value.avatar_url,
            bio: value.bio,
            default_provider_id: value.default_provider_id,
            default_provider_name: value.default_provider_name,
            theme: value.theme,
            language: value.language,
            timezone: value.timezone,
            email_notifications: value.email_notifications,
            usage_alerts: value.usage_alerts,
            announcement_notifications: value.announcement_notifications,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct GatewayAdminPaymentCallbackView {
    pub(crate) id: String,
    pub(crate) payment_order_id: Option<String>,
    pub(crate) payment_method: String,
    pub(crate) callback_key: String,
    pub(crate) order_no: Option<String>,
    pub(crate) gateway_order_id: Option<String>,
    pub(crate) payload_hash: Option<String>,
    pub(crate) signature_valid: bool,
    pub(crate) status: String,
    pub(crate) payload: Option<serde_json::Value>,
    pub(crate) error_message: Option<String>,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) processed_at_unix_secs: Option<u64>,
}

impl From<super::AdminPaymentCallbackRecord> for GatewayAdminPaymentCallbackView {
    fn from(value: super::AdminPaymentCallbackRecord) -> Self {
        Self {
            id: value.id,
            payment_order_id: value.payment_order_id,
            payment_method: value.payment_method,
            callback_key: value.callback_key,
            order_no: value.order_no,
            gateway_order_id: value.gateway_order_id,
            payload_hash: value.payload_hash,
            signature_valid: value.signature_valid,
            status: value.status,
            payload: value.payload,
            error_message: value.error_message,
            created_at_unix_ms: value.created_at_unix_ms,
            processed_at_unix_secs: value.processed_at_unix_secs,
        }
    }
}
