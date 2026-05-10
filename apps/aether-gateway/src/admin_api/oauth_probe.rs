use super::{
    probe_antigravity_provider_key_with_classification,
    probe_chatgpt_web_provider_key_with_classification,
    probe_codex_provider_key_with_classification, probe_kiro_provider_key_with_classification,
    AdminAppState,
};
use crate::ai_serving::api::LocalCoreSyncErrorKind;
use crate::execution_runtime::oauth_retry::status_may_be_oauth_invalid;
use crate::execution_runtime::submission::classify_local_sync_error_kind;
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OauthKeyProbeClassification {
    Healthy,
    OAuthInvalid,
    AccountBlocked,
    RateLimited,
    ServerError,
    TransportError,
}

#[derive(Debug, Clone)]
pub(crate) struct OauthKeyProbeResult {
    success: bool,
    auto_removed: bool,
    auto_removed_counted: bool,
    payload: serde_json::Value,
}

impl OauthKeyProbeResult {
    pub(crate) fn new(
        success: bool,
        auto_removed: bool,
        auto_removed_counted: bool,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            success,
            auto_removed,
            auto_removed_counted,
            payload,
        }
    }

    pub(crate) fn success(&self) -> bool {
        self.success
    }

    pub(crate) fn auto_removed(&self) -> bool {
        self.auto_removed
    }

    pub(crate) fn auto_removed_counted(&self) -> bool {
        self.auto_removed_counted
    }

    pub(crate) fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    pub(crate) fn into_payload(self) -> serde_json::Value {
        self.payload
    }
}

#[derive(Debug, Clone)]
pub(crate) enum OauthKeyProbeOutcome {
    Healthy(OauthKeyProbeResult),
    OAuthInvalid(OauthKeyProbeResult),
    AccountBlocked(OauthKeyProbeResult),
    RateLimited(OauthKeyProbeResult),
    ServerError(OauthKeyProbeResult),
    TransportError(OauthKeyProbeResult),
}

impl OauthKeyProbeOutcome {
    pub(crate) fn from_payload(
        classification: OauthKeyProbeClassification,
        success: bool,
        auto_removed: bool,
        auto_removed_counted: bool,
        payload: serde_json::Value,
    ) -> Self {
        let result = OauthKeyProbeResult::new(success, auto_removed, auto_removed_counted, payload);
        match classification {
            OauthKeyProbeClassification::Healthy => Self::Healthy(result),
            OauthKeyProbeClassification::OAuthInvalid => Self::OAuthInvalid(result),
            OauthKeyProbeClassification::AccountBlocked => Self::AccountBlocked(result),
            OauthKeyProbeClassification::RateLimited => Self::RateLimited(result),
            OauthKeyProbeClassification::ServerError => Self::ServerError(result),
            OauthKeyProbeClassification::TransportError => Self::TransportError(result),
        }
    }

    pub(crate) fn classification(&self) -> OauthKeyProbeClassification {
        match self {
            Self::Healthy(_) => OauthKeyProbeClassification::Healthy,
            Self::OAuthInvalid(_) => OauthKeyProbeClassification::OAuthInvalid,
            Self::AccountBlocked(_) => OauthKeyProbeClassification::AccountBlocked,
            Self::RateLimited(_) => OauthKeyProbeClassification::RateLimited,
            Self::ServerError(_) => OauthKeyProbeClassification::ServerError,
            Self::TransportError(_) => OauthKeyProbeClassification::TransportError,
        }
    }

    pub(crate) fn result(&self) -> &OauthKeyProbeResult {
        match self {
            Self::Healthy(result)
            | Self::OAuthInvalid(result)
            | Self::AccountBlocked(result)
            | Self::RateLimited(result)
            | Self::ServerError(result)
            | Self::TransportError(result) => result,
        }
    }

    pub(crate) fn success(&self) -> bool {
        self.result().success()
    }

    pub(crate) fn auto_removed(&self) -> bool {
        self.result().auto_removed()
    }

    pub(crate) fn auto_removed_counted(&self) -> bool {
        self.result().auto_removed_counted()
    }

    pub(crate) fn payload(&self) -> &serde_json::Value {
        self.result().payload()
    }

    pub(crate) fn into_payload(self) -> serde_json::Value {
        match self {
            Self::Healthy(result)
            | Self::OAuthInvalid(result)
            | Self::AccountBlocked(result)
            | Self::RateLimited(result)
            | Self::ServerError(result)
            | Self::TransportError(result) => result.into_payload(),
        }
    }
}

pub(crate) fn classify_oauth_key_probe_status(
    provider_type: &str,
    status: &str,
    status_code: Option<u16>,
    message: Option<&str>,
    oauth_invalid_reason: Option<&str>,
    success: bool,
) -> OauthKeyProbeClassification {
    if success {
        return OauthKeyProbeClassification::Healthy;
    }

    let normalized_provider = provider_type.trim().to_ascii_lowercase();
    let normalized_status = status.trim().to_ascii_lowercase();
    let diagnostic_text = oauth_invalid_reason.or(message).unwrap_or_default().trim();
    let normalized_diagnostic = diagnostic_text.to_ascii_lowercase();

    if normalized_status == "quota_exhausted" || status_code == Some(429) {
        return OauthKeyProbeClassification::RateLimited;
    }

    if matches!(
        normalized_status.as_str(),
        "workspace_deactivated" | "banned"
    ) || normalized_diagnostic.contains("[account_block]")
        || (normalized_status == "forbidden"
            && matches!(normalized_provider.as_str(), "antigravity" | "chatgpt_web"))
    {
        return OauthKeyProbeClassification::AccountBlocked;
    }

    if normalized_status == "auth_invalid"
        || normalized_diagnostic.contains("[oauth_expired]")
        || normalized_diagnostic.contains("token 无效")
        || normalized_diagnostic.contains("无效或已过期")
        || status_code.is_some_and(|code| status_may_be_oauth_invalid(code, Some(diagnostic_text)))
    {
        return OauthKeyProbeClassification::OAuthInvalid;
    }

    if normalized_status == "no_metadata" {
        return OauthKeyProbeClassification::ServerError;
    }

    if let Some(status_code) = status_code {
        match classify_local_sync_error_kind(status_code, None, None, None, diagnostic_text) {
            LocalCoreSyncErrorKind::RateLimit => OauthKeyProbeClassification::RateLimited,
            LocalCoreSyncErrorKind::Authentication => OauthKeyProbeClassification::OAuthInvalid,
            LocalCoreSyncErrorKind::PermissionDenied => OauthKeyProbeClassification::TransportError,
            LocalCoreSyncErrorKind::Overloaded | LocalCoreSyncErrorKind::ServerError => {
                OauthKeyProbeClassification::ServerError
            }
            LocalCoreSyncErrorKind::InvalidRequest
            | LocalCoreSyncErrorKind::ContextLengthExceeded
            | LocalCoreSyncErrorKind::NotFound => OauthKeyProbeClassification::TransportError,
        }
    } else {
        OauthKeyProbeClassification::TransportError
    }
}

pub(crate) async fn probe_oauth_provider_key_with_classification(
    admin_state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: StoredProviderCatalogKey,
    proxy_override: Option<&ProxySnapshot>,
    probe_timeout: Duration,
    allow_auto_remove: bool,
) -> Result<OauthKeyProbeOutcome, GatewayError> {
    match provider.provider_type.trim().to_ascii_lowercase().as_str() {
        "codex" => {
            probe_codex_provider_key_with_classification(
                admin_state,
                provider,
                endpoint,
                key,
                proxy_override,
                probe_timeout,
                allow_auto_remove,
            )
            .await
        }
        "kiro" => {
            probe_kiro_provider_key_with_classification(
                admin_state,
                provider,
                endpoint,
                key,
                proxy_override,
                probe_timeout,
                allow_auto_remove,
            )
            .await
        }
        "antigravity" => {
            probe_antigravity_provider_key_with_classification(
                admin_state,
                provider,
                endpoint,
                key,
                proxy_override,
                probe_timeout,
                allow_auto_remove,
            )
            .await
        }
        "chatgpt_web" => {
            probe_chatgpt_web_provider_key_with_classification(
                admin_state,
                provider,
                endpoint,
                key,
                proxy_override,
                probe_timeout,
                allow_auto_remove,
            )
            .await
        }
        other => Err(GatewayError::Internal(format!(
            "unsupported OAuth quota probe provider type: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_oauth_key_probe_status, OauthKeyProbeClassification};

    #[test]
    fn probe_oauth_provider_key_classifies_common_statuses() {
        assert_eq!(
            classify_oauth_key_probe_status("codex", "success", Some(200), None, None, true),
            OauthKeyProbeClassification::Healthy
        );
        assert_eq!(
            classify_oauth_key_probe_status(
                "codex",
                "auth_invalid",
                Some(401),
                Some("expired token"),
                Some("[OAUTH_EXPIRED]expired token"),
                false,
            ),
            OauthKeyProbeClassification::OAuthInvalid
        );
        assert_eq!(
            classify_oauth_key_probe_status(
                "chatgpt_web",
                "forbidden",
                Some(403),
                Some("account blocked"),
                Some("[ACCOUNT_BLOCK]account blocked"),
                false,
            ),
            OauthKeyProbeClassification::AccountBlocked
        );
        assert_eq!(
            classify_oauth_key_probe_status(
                "codex",
                "quota_exhausted",
                Some(402),
                None,
                None,
                false
            ),
            OauthKeyProbeClassification::RateLimited
        );
        assert_eq!(
            classify_oauth_key_probe_status(
                "kiro",
                "error",
                Some(503),
                Some("unavailable"),
                None,
                false
            ),
            OauthKeyProbeClassification::ServerError
        );
        assert_eq!(
            classify_oauth_key_probe_status("kiro", "error", None, Some("network"), None, false),
            OauthKeyProbeClassification::TransportError
        );
    }

    fn assert_probe_oauth_provider_key_classification(
        provider_type: &str,
        status: &str,
        status_code: Option<u16>,
        message: Option<&str>,
        oauth_invalid_reason: Option<&str>,
        success: bool,
        expected: OauthKeyProbeClassification,
    ) {
        assert_eq!(
            classify_oauth_key_probe_status(
                provider_type,
                status,
                status_code,
                message,
                oauth_invalid_reason,
                success,
            ),
            expected
        );
    }

    #[test]
    fn probe_oauth_provider_key_codex_healthy() {
        assert_probe_oauth_provider_key_classification(
            "codex",
            "success",
            Some(200),
            None,
            None,
            true,
            OauthKeyProbeClassification::Healthy,
        );
    }

    #[test]
    fn probe_oauth_provider_key_codex_oauth_invalid() {
        assert_probe_oauth_provider_key_classification(
            "codex",
            "auth_invalid",
            Some(401),
            Some("expired token"),
            Some("[OAUTH_EXPIRED]expired token"),
            false,
            OauthKeyProbeClassification::OAuthInvalid,
        );
    }

    #[test]
    fn probe_oauth_provider_key_codex_account_blocked() {
        assert_probe_oauth_provider_key_classification(
            "codex",
            "workspace_deactivated",
            Some(402),
            Some("workspace deactivated"),
            Some("[ACCOUNT_BLOCK]工作区已停用 (deactivated_workspace)"),
            false,
            OauthKeyProbeClassification::AccountBlocked,
        );
    }

    #[test]
    fn probe_oauth_provider_key_codex_rate_limited() {
        assert_probe_oauth_provider_key_classification(
            "codex",
            "quota_exhausted",
            Some(402),
            Some("quota exhausted"),
            None,
            false,
            OauthKeyProbeClassification::RateLimited,
        );
    }

    #[test]
    fn probe_oauth_provider_key_kiro_healthy() {
        assert_probe_oauth_provider_key_classification(
            "kiro",
            "success",
            Some(200),
            None,
            None,
            true,
            OauthKeyProbeClassification::Healthy,
        );
    }

    #[test]
    fn probe_oauth_provider_key_kiro_oauth_invalid() {
        assert_probe_oauth_provider_key_classification(
            "kiro",
            "error",
            Some(401),
            Some("Bearer token invalid"),
            Some("Kiro Token 无效或已过期"),
            false,
            OauthKeyProbeClassification::OAuthInvalid,
        );
    }

    #[test]
    fn probe_oauth_provider_key_kiro_account_blocked() {
        assert_probe_oauth_provider_key_classification(
            "kiro",
            "banned",
            Some(423),
            Some("account suspended"),
            Some("账户已封禁: account suspended"),
            false,
            OauthKeyProbeClassification::AccountBlocked,
        );
    }

    #[test]
    fn probe_oauth_provider_key_kiro_server_error() {
        assert_probe_oauth_provider_key_classification(
            "kiro",
            "error",
            Some(503),
            Some("service unavailable"),
            None,
            false,
            OauthKeyProbeClassification::ServerError,
        );
    }

    #[test]
    fn probe_oauth_provider_key_antigravity_healthy() {
        assert_probe_oauth_provider_key_classification(
            "antigravity",
            "success",
            Some(200),
            None,
            None,
            true,
            OauthKeyProbeClassification::Healthy,
        );
    }

    #[test]
    fn probe_oauth_provider_key_antigravity_oauth_invalid() {
        assert_probe_oauth_provider_key_classification(
            "antigravity",
            "auth_invalid",
            Some(401),
            Some("token expired"),
            Some("[OAUTH_EXPIRED]token expired"),
            false,
            OauthKeyProbeClassification::OAuthInvalid,
        );
    }

    #[test]
    fn probe_oauth_provider_key_antigravity_account_blocked() {
        assert_probe_oauth_provider_key_classification(
            "antigravity",
            "forbidden",
            Some(403),
            Some("account forbidden"),
            Some("账户访问被禁止: account forbidden"),
            false,
            OauthKeyProbeClassification::AccountBlocked,
        );
    }

    #[test]
    fn probe_oauth_provider_key_antigravity_server_error() {
        assert_probe_oauth_provider_key_classification(
            "antigravity",
            "no_metadata",
            Some(200),
            Some("响应中未包含配额信息"),
            None,
            false,
            OauthKeyProbeClassification::ServerError,
        );
    }

    #[test]
    fn probe_oauth_provider_key_chatgpt_web_healthy() {
        assert_probe_oauth_provider_key_classification(
            "chatgpt_web",
            "success",
            Some(200),
            None,
            None,
            true,
            OauthKeyProbeClassification::Healthy,
        );
    }

    #[test]
    fn probe_oauth_provider_key_chatgpt_web_oauth_invalid() {
        assert_probe_oauth_provider_key_classification(
            "chatgpt_web",
            "auth_invalid",
            Some(401),
            Some("session expired"),
            Some("[OAUTH_EXPIRED]session expired"),
            false,
            OauthKeyProbeClassification::OAuthInvalid,
        );
    }

    #[test]
    fn probe_oauth_provider_key_chatgpt_web_account_blocked() {
        assert_probe_oauth_provider_key_classification(
            "chatgpt_web",
            "forbidden",
            Some(403),
            Some("account restricted"),
            Some("[ACCOUNT_BLOCK]account restricted"),
            false,
            OauthKeyProbeClassification::AccountBlocked,
        );
    }

    #[test]
    fn probe_oauth_provider_key_chatgpt_web_server_error() {
        assert_probe_oauth_provider_key_classification(
            "chatgpt_web",
            "error",
            Some(500),
            Some("upstream failed"),
            None,
            false,
            OauthKeyProbeClassification::ServerError,
        );
    }

    #[test]
    fn probe_oauth_provider_key_transport_error_without_status() {
        assert_probe_oauth_provider_key_classification(
            "codex",
            "error",
            None,
            Some("network unavailable"),
            None,
            false,
            OauthKeyProbeClassification::TransportError,
        );
    }
}
