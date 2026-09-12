use super::super::adapter::{find_string, form_headers, mapped_bool, mapped_string};
use crate::core::{
    redacted_oauth_error_body_excerpt, OAuthAuthorizeResponse, OAuthError, OAuthTokenSet,
};
use crate::identity::{
    ExternalIdentity, IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthProvider,
    IdentityOAuthProviderConfig, IdentityOAuthStartContext,
};
use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthNetworkContext};
use async_trait::async_trait;
use url::form_urlencoded;

const SERVER_MANAGED_AUTHORIZE_PARAMS: &[&str] = &[
    "response_type",
    "client_id",
    "redirect_uri",
    "state",
    "scope",
    "code_challenge",
    "code_challenge_method",
];

#[derive(Debug, Clone, Default)]
pub struct CustomOidcIdentityOAuthProvider;

#[async_trait]
impl IdentityOAuthProvider for CustomOidcIdentityOAuthProvider {
    fn provider_type(&self) -> &'static str {
        "custom_oidc"
    }

    fn build_authorize_url(
        &self,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthStartContext,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        let mut url = url::Url::parse(&config.authorization_url)
            .map_err(|_| OAuthError::invalid_request("authorization_url must be absolute"))?;
        if url.query_pairs().any(|(name, _)| {
            SERVER_MANAGED_AUTHORIZE_PARAMS
                .iter()
                .any(|reserved| name.eq_ignore_ascii_case(reserved))
        }) {
            return Err(OAuthError::invalid_request(
                "authorization_url must not predefine server-managed OAuth parameters",
            ));
        }
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", &config.client_id);
            query.append_pair("redirect_uri", &config.redirect_uri);
            query.append_pair("state", &ctx.state);
            if !config.scopes.is_empty() {
                query.append_pair("scope", &config.scopes.join(" "));
            }
            if let Some(challenge) = ctx.code_challenge.as_deref() {
                query.append_pair("code_challenge", challenge);
                query.append_pair("code_challenge_method", "S256");
            }
        }

        Ok(OAuthAuthorizeResponse {
            authorize_url: url.to_string(),
            state: ctx.state.clone(),
            code_challenge: ctx.code_challenge.clone(),
        })
    }

    async fn exchange_code(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<OAuthTokenSet, OAuthError> {
        let body_bytes = {
            let mut form = form_urlencoded::Serializer::new(String::new());
            form.append_pair("grant_type", "authorization_code");
            form.append_pair("client_id", &config.client_id);
            form.append_pair("redirect_uri", &config.redirect_uri);
            form.append_pair("code", &ctx.code);
            if let Some(secret) = config
                .client_secret
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                form.append_pair("client_secret", secret);
            }
            if let Some(verifier) = ctx.pkce_verifier.as_deref() {
                form.append_pair("code_verifier", verifier);
            }
            form.finish().into_bytes()
        };
        let response = executor
            .execute(OAuthHttpRequest {
                request_id: format!("identity-oauth:{}:exchange-code", config.provider_type),
                method: reqwest::Method::POST,
                url: config.token_url.clone(),
                headers: form_headers(),
                content_type: Some("application/x-www-form-urlencoded".to_string()),
                json_body: None,
                body_bytes: Some(body_bytes),
                network: ctx.network.clone(),
                transport_profile: None,
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: redacted_oauth_error_body_excerpt(&response.body_text),
            });
        }
        let payload = response
            .json_body
            .or_else(|| serde_json::from_str(&response.body_text).ok())
            .ok_or_else(|| OAuthError::invalid_response("token response is not json"))?;
        OAuthTokenSet::from_token_payload(payload)
            .ok_or_else(|| OAuthError::invalid_response("token response missing access_token"))
    }

    async fn fetch_identity(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        tokens: &OAuthTokenSet,
        network: OAuthNetworkContext,
    ) -> Result<ExternalIdentity, OAuthError> {
        let userinfo_url = config
            .userinfo_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| OAuthError::invalid_request("userinfo_url is required"))?;
        let response = executor
            .execute(OAuthHttpRequest {
                request_id: format!("identity-oauth:{}:userinfo", config.provider_type),
                method: reqwest::Method::GET,
                url: userinfo_url.to_string(),
                headers: std::collections::BTreeMap::from([
                    ("authorization".to_string(), tokens.bearer_header_value()),
                    ("accept".to_string(), "application/json".to_string()),
                ]),
                content_type: None,
                json_body: None,
                body_bytes: None,
                network,
                transport_profile: None,
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: redacted_oauth_error_body_excerpt(&response.body_text),
            });
        }
        let raw = response
            .json_body
            .or_else(|| serde_json::from_str(&response.body_text).ok())
            .ok_or_else(|| OAuthError::invalid_response("userinfo response is not json"))?;
        let subject = mapped_string(&raw, config.attribute_mapping.as_ref(), "sub")
            .or_else(|| find_string(&raw, "id"))
            .ok_or_else(|| OAuthError::invalid_response("userinfo response missing subject"))?;
        Ok(ExternalIdentity {
            provider_type: config.provider_type.clone(),
            subject,
            email: mapped_string(&raw, config.attribute_mapping.as_ref(), "email"),
            email_verified: mapped_bool(&raw, config.attribute_mapping.as_ref(), "email_verified")
                .unwrap_or(false),
            username: mapped_string(&raw, config.attribute_mapping.as_ref(), "username"),
            display_name: mapped_string(&raw, config.attribute_mapping.as_ref(), "display_name")
                .or_else(|| find_string(&raw, "name")),
            avatar_url: mapped_string(&raw, config.attribute_mapping.as_ref(), "avatar_url"),
            raw,
        })
    }

    fn map_identity(
        &self,
        config: &IdentityOAuthProviderConfig,
        identity: ExternalIdentity,
    ) -> Result<IdentityClaims, OAuthError> {
        Ok(IdentityClaims {
            provider_type: config.provider_type.clone(),
            subject: identity.subject,
            email_verified: identity.email.is_some() && identity.email_verified,
            email: identity.email,
            username: identity.username,
            display_name: identity.display_name,
            raw: identity.raw,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CustomOidcIdentityOAuthProvider;
    use crate::identity::{
        ExternalIdentity, IdentityOAuthProvider, IdentityOAuthProviderConfig,
        IdentityOAuthStartContext,
    };
    use crate::network::OAuthNetworkContext;
    use serde_json::json;

    fn config() -> IdentityOAuthProviderConfig {
        IdentityOAuthProviderConfig {
            provider_type: "custom_oidc_work".to_string(),
            display_name: "Work OIDC".to_string(),
            authorization_url: "https://idp.example.test/authorize".to_string(),
            token_url: "https://idp.example.test/token".to_string(),
            userinfo_url: Some("https://idp.example.test/userinfo".to_string()),
            client_id: "client".to_string(),
            client_secret: None,
            scopes: vec!["openid".to_string(), "email".to_string()],
            redirect_uri: "https://gateway.example.test/callback".to_string(),
            frontend_callback_url: "https://app.example.test/callback".to_string(),
            attribute_mapping: None,
            extra_config: None,
        }
    }

    fn start_context() -> IdentityOAuthStartContext {
        IdentityOAuthStartContext {
            state: "server-state".to_string(),
            code_challenge: Some("server-challenge".to_string()),
            network: OAuthNetworkContext::direct_identity(),
        }
    }

    #[test]
    fn custom_oidc_authorize_url_rejects_predefined_server_managed_parameters() {
        for name in [
            "response_type",
            "client_id",
            "redirect_uri",
            "state",
            "scope",
            "code_challenge",
            "code_challenge_method",
        ] {
            let mut config = config();
            config.authorization_url =
                format!("https://idp.example.test/authorize?{name}=attacker");

            assert!(CustomOidcIdentityOAuthProvider
                .build_authorize_url(&config, &start_context())
                .is_err());
        }
    }

    #[test]
    fn custom_oidc_authorize_url_preserves_non_oauth_tenant_parameters() {
        let mut config = config();
        config.authorization_url =
            "https://idp.example.test/authorize?tenant=workforce".to_string();

        let response = CustomOidcIdentityOAuthProvider
            .build_authorize_url(&config, &start_context())
            .expect("tenant parameter should be preserved");
        let parsed = url::Url::parse(&response.authorize_url).expect("authorize URL");
        let params = parsed.query_pairs().collect::<Vec<_>>();

        assert!(params
            .iter()
            .any(|(name, value)| name == "tenant" && value == "workforce"));
        assert_eq!(params.iter().filter(|(name, _)| name == "state").count(), 1);
        assert!(params
            .iter()
            .any(|(name, value)| name == "state" && value == "server-state"));
    }

    #[test]
    fn custom_oidc_propagates_an_explicit_verified_email_claim() {
        let claims = CustomOidcIdentityOAuthProvider
            .map_identity(
                &config(),
                ExternalIdentity {
                    provider_type: "custom_oidc_work".to_string(),
                    subject: "user-1".to_string(),
                    email: Some("user@example.test".to_string()),
                    email_verified: true,
                    username: Some("user".to_string()),
                    display_name: None,
                    avatar_url: None,
                    raw: json!({"email_verified": true}),
                },
            )
            .expect("identity should map");

        assert!(claims.email_verified);
    }

    #[test]
    fn custom_oidc_cannot_verify_a_missing_email() {
        let claims = CustomOidcIdentityOAuthProvider
            .map_identity(
                &config(),
                ExternalIdentity {
                    provider_type: "custom_oidc_work".to_string(),
                    subject: "user-1".to_string(),
                    email: None,
                    email_verified: true,
                    username: Some("user".to_string()),
                    display_name: None,
                    avatar_url: None,
                    raw: json!({"email_verified": true}),
                },
            )
            .expect("identity should map");

        assert!(!claims.email_verified);
    }
}
