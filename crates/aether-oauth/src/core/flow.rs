use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProviderMetadata {
    pub provider_type: String,
    pub display_name: String,
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
    pub use_pkce: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthAuthorizeRequest {
    pub state: String,
    pub code_challenge: Option<String>,
    pub prompt: Option<String>,
    pub login_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OAuthAuthorizeResponse {
    pub authorize_url: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_challenge: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OAuthDeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}
