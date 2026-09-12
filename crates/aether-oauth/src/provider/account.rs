use crate::core::OAuthTokenSet;
use crate::network::OAuthNetworkContext;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOAuthCapabilities {
    pub supports_authorization_code: bool,
    pub supports_cookie_authorization: bool,
    pub supports_refresh_token_import: bool,
    pub supports_batch_import: bool,
    pub supports_device_flow: bool,
    pub supports_account_probe: bool,
    pub rotates_refresh_token: bool,
}

impl ProviderOAuthCapabilities {
    pub const GENERIC_AUTH_CODE: Self = Self {
        supports_authorization_code: true,
        supports_cookie_authorization: false,
        supports_refresh_token_import: true,
        supports_batch_import: true,
        supports_device_flow: false,
        supports_account_probe: false,
        rotates_refresh_token: true,
    };
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderOAuthCookieAuthorizationInput {
    pub session_key: String,
}

impl std::fmt::Debug for ProviderOAuthCookieAuthorizationInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderOAuthCookieAuthorizationInput")
            .field("session_key", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ProviderOAuthTransportContext {
    pub provider_id: String,
    pub provider_type: String,
    pub endpoint_id: Option<String>,
    pub key_id: Option<String>,
    pub auth_type: Option<String>,
    pub decrypted_api_key: Option<String>,
    pub decrypted_auth_config: Option<String>,
    pub provider_config: Option<Value>,
    pub endpoint_config: Option<Value>,
    pub key_config: Option<Value>,
    pub network: OAuthNetworkContext,
}

impl std::fmt::Debug for ProviderOAuthTransportContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderOAuthTransportContext")
            .field("provider_id", &self.provider_id)
            .field("provider_type", &self.provider_type)
            .field("endpoint_id", &self.endpoint_id)
            .field("key_id", &self.key_id)
            .field("auth_type", &self.auth_type)
            .field(
                "decrypted_api_key",
                &self.decrypted_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "decrypted_auth_config",
                &self.decrypted_auth_config.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "provider_config",
                &self.provider_config.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "endpoint_config",
                &self.endpoint_config.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "key_config",
                &self.key_config.as_ref().map(|_| "<redacted>"),
            )
            .field("network", &self.network)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ProviderOAuthTokenSet {
    pub token_set: OAuthTokenSet,
    pub auth_config: Value,
}

impl std::fmt::Debug for ProviderOAuthTokenSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderOAuthTokenSet")
            .field("token_set", &self.token_set)
            .field("auth_config", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ProviderOAuthAccount {
    pub provider_type: String,
    pub access_token: String,
    pub auth_config: Value,
    pub expires_at_unix_secs: Option<u64>,
    pub identity: BTreeMap<String, Value>,
}

impl std::fmt::Debug for ProviderOAuthAccount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderOAuthAccount")
            .field("provider_type", &self.provider_type)
            .field("access_token", &"<redacted>")
            .field("auth_config", &"<redacted>")
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .field("identity_keys", &self.identity.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ProviderOAuthAccount {
    pub fn request_bearer_auth(&self) -> ProviderOAuthRequestAuth {
        ProviderOAuthRequestAuth::Header {
            name: "authorization".to_string(),
            value: format!("Bearer {}", self.access_token.trim()),
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum ProviderOAuthRequestAuth {
    Header {
        name: String,
        value: String,
    },
    Kiro {
        name: String,
        value: String,
        auth_config: Value,
        machine_id: String,
    },
}

impl std::fmt::Debug for ProviderOAuthRequestAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header { name, .. } => formatter
                .debug_struct("Header")
                .field("name", name)
                .field("value", &"<redacted>")
                .finish(),
            Self::Kiro {
                name, machine_id, ..
            } => formatter
                .debug_struct("Kiro")
                .field("name", name)
                .field("value", &"<redacted>")
                .field("auth_config", &"<redacted>")
                .field("machine_id", machine_id)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct ProviderOAuthImportInput {
    pub provider_type: String,
    pub name: Option<String>,
    pub refresh_token: Option<String>,
    pub raw_credentials: Option<Value>,
    pub network: OAuthNetworkContext,
}

impl std::fmt::Debug for ProviderOAuthImportInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderOAuthImportInput")
            .field("provider_type", &self.provider_type)
            .field("name", &self.name)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "raw_credentials",
                &self.raw_credentials.as_ref().map(|_| "<redacted>"),
            )
            .field("network", &self.network)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ProviderOAuthAccountState {
    pub is_valid: bool,
    pub email: Option<String>,
    pub quota: Option<Value>,
    pub invalid_reason: Option<String>,
    pub raw: Option<Value>,
}

impl std::fmt::Debug for ProviderOAuthAccountState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderOAuthAccountState")
            .field("is_valid", &self.is_valid)
            .field("email", &self.email)
            .field("quota", &self.quota)
            .field("invalid_reason", &self.invalid_reason)
            .field("raw", &self.raw.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderOAuthAccount, ProviderOAuthAccountState, ProviderOAuthImportInput,
        ProviderOAuthTokenSet, ProviderOAuthTransportContext,
    };
    use crate::core::OAuthTokenSet;
    use crate::network::OAuthNetworkContext;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn debug_output_redacts_provider_oauth_credentials() {
        let context = ProviderOAuthTransportContext {
            provider_id: "provider-1".to_string(),
            provider_type: "generic".to_string(),
            endpoint_id: None,
            key_id: None,
            auth_type: None,
            decrypted_api_key: Some("api-key-secret-sentinel".to_string()),
            decrypted_auth_config: Some("auth-config-secret-sentinel".to_string()),
            provider_config: Some(json!({"client_secret": "provider-secret-sentinel"})),
            endpoint_config: None,
            key_config: None,
            network: OAuthNetworkContext::direct_identity(),
        };
        let token_set = ProviderOAuthTokenSet {
            token_set: OAuthTokenSet {
                access_token: "access-secret-sentinel".to_string(),
                refresh_token: Some("refresh-secret-sentinel".to_string()),
                token_type: None,
                scope: None,
                expires_at_unix_secs: None,
                raw_payload: None,
            },
            auth_config: json!({"password": "password-secret-sentinel"}),
        };
        let account = ProviderOAuthAccount {
            provider_type: "generic".to_string(),
            access_token: "account-secret-sentinel".to_string(),
            auth_config: json!({"client_secret": "account-config-secret-sentinel"}),
            expires_at_unix_secs: None,
            identity: BTreeMap::new(),
        };
        let import = ProviderOAuthImportInput {
            provider_type: "generic".to_string(),
            name: None,
            refresh_token: Some("import-refresh-secret-sentinel".to_string()),
            raw_credentials: Some(json!({"api_key": "import-raw-secret-sentinel"})),
            network: OAuthNetworkContext::direct_identity(),
        };
        let state = ProviderOAuthAccountState {
            is_valid: false,
            email: None,
            quota: None,
            invalid_reason: None,
            raw: Some(json!({"access_token": "probe-raw-secret-sentinel"})),
        };

        let debug = format!("{context:?} {token_set:?} {account:?} {import:?} {state:?}");
        for secret in [
            "api-key-secret-sentinel",
            "auth-config-secret-sentinel",
            "provider-secret-sentinel",
            "access-secret-sentinel",
            "refresh-secret-sentinel",
            "password-secret-sentinel",
            "account-secret-sentinel",
            "account-config-secret-sentinel",
            "import-refresh-secret-sentinel",
            "import-raw-secret-sentinel",
            "probe-raw-secret-sentinel",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
        assert!(debug.contains("<redacted>"));
    }
}
