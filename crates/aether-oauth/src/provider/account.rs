use crate::core::OAuthTokenSet;
use crate::network::OAuthNetworkContext;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOAuthCapabilities {
    pub supports_authorization_code: bool,
    pub supports_refresh_token_import: bool,
    pub supports_batch_import: bool,
    pub supports_device_flow: bool,
    pub supports_account_probe: bool,
    pub rotates_refresh_token: bool,
}

impl ProviderOAuthCapabilities {
    pub const GENERIC_AUTH_CODE: Self = Self {
        supports_authorization_code: true,
        supports_refresh_token_import: true,
        supports_batch_import: true,
        supports_device_flow: false,
        supports_account_probe: false,
        rotates_refresh_token: true,
    };
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderOAuthTokenSet {
    pub token_set: OAuthTokenSet,
    pub auth_config: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderOAuthAccount {
    pub provider_type: String,
    pub access_token: String,
    pub auth_config: Value,
    pub expires_at_unix_secs: Option<u64>,
    pub identity: BTreeMap<String, Value>,
}

impl ProviderOAuthAccount {
    pub fn request_bearer_auth(&self) -> ProviderOAuthRequestAuth {
        ProviderOAuthRequestAuth::Header {
            name: "authorization".to_string(),
            value: format!("Bearer {}", self.access_token.trim()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderOAuthImportInput {
    pub provider_type: String,
    pub name: Option<String>,
    pub refresh_token: Option<String>,
    pub raw_credentials: Option<Value>,
    pub network: OAuthNetworkContext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderOAuthAccountState {
    pub is_valid: bool,
    pub email: Option<String>,
    pub quota: Option<Value>,
    pub invalid_reason: Option<String>,
    pub raw: Option<Value>,
}
