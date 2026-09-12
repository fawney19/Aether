use crate::core::{OAuthAuthorizeResponse, OAuthError, OAuthTokenSet};
use crate::network::{OAuthHttpExecutor, OAuthNetworkContext};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq)]
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

impl std::fmt::Debug for IdentityOAuthProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityOAuthProviderConfig")
            .field("provider_type", &self.provider_type)
            .field("display_name", &self.display_name)
            .field("authorization_url", &"[REDACTED]")
            .field("token_url", &"[REDACTED]")
            .field(
                "userinfo_url",
                &self.userinfo_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("scopes", &self.scopes)
            .field("redirect_uri", &self.redirect_uri)
            .field("frontend_callback_url", &self.frontend_callback_url)
            .field(
                "attribute_mapping",
                &self.attribute_mapping.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "extra_config",
                &self.extra_config.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct IdentityOAuthStartContext {
    pub state: String,
    pub code_challenge: Option<String>,
    pub network: OAuthNetworkContext,
}

impl std::fmt::Debug for IdentityOAuthStartContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityOAuthStartContext")
            .field("state", &"[REDACTED]")
            .field(
                "code_challenge",
                &self.code_challenge.as_ref().map(|_| "[REDACTED]"),
            )
            .field("network", &self.network)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct IdentityOAuthExchangeContext {
    pub code: String,
    pub state: String,
    pub pkce_verifier: Option<String>,
    pub network: OAuthNetworkContext,
}

impl std::fmt::Debug for IdentityOAuthExchangeContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityOAuthExchangeContext")
            .field("code", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field(
                "pkce_verifier",
                &self.pkce_verifier.as_ref().map(|_| "[REDACTED]"),
            )
            .field("network", &self.network)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct ExternalIdentity {
    pub provider_type: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub raw: Value,
}

impl std::fmt::Debug for ExternalIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalIdentity")
            .field("provider_type", &self.provider_type)
            .field("subject", &self.subject)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("avatar_url", &self.avatar_url)
            .field("raw", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct IdentityClaims {
    pub provider_type: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub raw: Value,
}

impl std::fmt::Debug for IdentityClaims {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityClaims")
            .field("provider_type", &self.provider_type)
            .field("subject", &self.subject)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("raw", &"[REDACTED]")
            .finish()
    }
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

pub(crate) fn mapped_bool(raw: &Value, mapping: Option<&Value>, logical_key: &str) -> Option<bool> {
    let mapped_key = match mapping
        .and_then(Value::as_object)
        .and_then(|object| object.get(logical_key))
    {
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())?,
        None => logical_key,
    };
    find_value(raw, mapped_key).and_then(Value::as_bool)
}

pub(crate) fn find_string(raw: &Value, key: &str) -> Option<String> {
    find_value(raw, key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn find_value<'a>(raw: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = raw;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
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

#[cfg(test)]
mod tests {
    use super::{
        mapped_bool, ExternalIdentity, IdentityClaims, IdentityOAuthExchangeContext,
        IdentityOAuthProviderConfig, IdentityOAuthStartContext,
    };
    use crate::network::OAuthNetworkContext;
    use serde_json::json;

    #[test]
    fn identity_oauth_debug_output_redacts_credentials_and_raw_claims() {
        let config = IdentityOAuthProviderConfig {
            provider_type: "custom".to_string(),
            display_name: "Custom".to_string(),
            authorization_url: "https://idp.example/authorize".to_string(),
            token_url: "https://idp.example/token".to_string(),
            userinfo_url: Some("https://idp.example/userinfo".to_string()),
            client_id: "public-client".to_string(),
            client_secret: Some("identity-client-secret-canary".to_string()),
            scopes: vec!["openid".to_string()],
            redirect_uri: "https://gateway.example/callback".to_string(),
            frontend_callback_url: "https://app.example/callback".to_string(),
            attribute_mapping: None,
            extra_config: Some(json!({"secret": "identity-extra-canary"})),
        };
        let start = IdentityOAuthStartContext {
            state: "identity-state-canary".to_string(),
            code_challenge: Some("identity-challenge-canary".to_string()),
            network: OAuthNetworkContext::direct_identity(),
        };
        let exchange = IdentityOAuthExchangeContext {
            code: "identity-code-canary".to_string(),
            state: "identity-exchange-state-canary".to_string(),
            pkce_verifier: Some("identity-verifier-canary".to_string()),
            network: OAuthNetworkContext::direct_identity(),
        };
        let external = ExternalIdentity {
            provider_type: "custom".to_string(),
            subject: "subject".to_string(),
            email: None,
            email_verified: false,
            username: None,
            display_name: None,
            avatar_url: None,
            raw: json!({"access_token": "identity-raw-canary"}),
        };
        let claims = IdentityClaims {
            provider_type: "custom".to_string(),
            subject: "subject".to_string(),
            email: None,
            email_verified: false,
            username: None,
            display_name: None,
            raw: json!({"id_token": "identity-claims-canary"}),
        };

        let debug = format!("{config:?} {start:?} {exchange:?} {external:?} {claims:?}");
        for secret in [
            "identity-client-secret-canary",
            "identity-extra-canary",
            "identity-state-canary",
            "identity-challenge-canary",
            "identity-code-canary",
            "identity-exchange-state-canary",
            "identity-verifier-canary",
            "identity-raw-canary",
            "identity-claims-canary",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn mapped_bool_accepts_only_an_explicit_json_boolean() {
        let raw = json!({
            "email_verified": true,
            "profile": {
                "verified": false,
                "string_verified": "true",
                "numeric_verified": 1
            }
        });

        assert_eq!(mapped_bool(&raw, None, "email_verified"), Some(true));
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": "profile.verified"})),
                "email_verified"
            ),
            Some(false)
        );
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": "profile.string_verified"})),
                "email_verified"
            ),
            None
        );
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": "profile.numeric_verified"})),
                "email_verified"
            ),
            None
        );
        assert_eq!(mapped_bool(&json!({}), None, "email_verified"), None);
        assert_eq!(
            mapped_bool(
                &raw,
                Some(&json!({"email_verified": true})),
                "email_verified"
            ),
            None
        );
        assert_eq!(
            mapped_bool(&raw, Some(&json!({"email_verified": ""})), "email_verified"),
            None
        );
    }
}
