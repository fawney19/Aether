use crate::core::{OAuthAuthorizeResponse, OAuthError, OAuthTokenSet};
use crate::network::{OAuthHttpExecutor, OAuthNetworkContext};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct IdentityOAuthProviderConfig {
    pub provider_type: String,
    pub display_name: String,
    pub authorization_url: String,
    pub token_url: String,
    pub userinfo_url: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_uri: String,
    pub frontend_callback_url: String,
    pub attribute_mapping: Option<Value>,
    pub extra_config: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdentityOAuthStartContext {
    pub state: String,
    pub code_challenge: Option<String>,
    pub network: OAuthNetworkContext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdentityOAuthExchangeContext {
    pub code: String,
    pub state: String,
    pub pkce_verifier: Option<String>,
    pub network: OAuthNetworkContext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalIdentity {
    pub provider_type: String,
    pub subject: String,
    pub email: Option<String>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdentityClaims {
    pub provider_type: String,
    pub subject: String,
    pub email: Option<String>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub raw: Value,
}

#[async_trait]
pub trait IdentityOAuthProvider: Send + Sync {
    fn provider_type(&self) -> &'static str;

    fn build_authorize_url(
        &self,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthStartContext,
    ) -> Result<OAuthAuthorizeResponse, OAuthError>;

    async fn exchange_code(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<OAuthTokenSet, OAuthError>;

    async fn fetch_identity(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        tokens: &OAuthTokenSet,
        network: OAuthNetworkContext,
    ) -> Result<ExternalIdentity, OAuthError>;

    fn map_identity(
        &self,
        config: &IdentityOAuthProviderConfig,
        identity: ExternalIdentity,
    ) -> Result<IdentityClaims, OAuthError>;
}

pub(crate) fn mapped_string(
    raw: &Value,
    mapping: Option<&Value>,
    logical_key: &str,
) -> Option<String> {
    let mapped_key = mapping
        .and_then(Value::as_object)
        .and_then(|object| object.get(logical_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(logical_key);
    find_string(raw, mapped_key)
}

pub(crate) fn find_string(raw: &Value, key: &str) -> Option<String> {
    let mut current = raw;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn form_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ),
        ("accept".to_string(), "application/json".to_string()),
    ])
}
