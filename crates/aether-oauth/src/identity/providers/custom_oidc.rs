use super::super::adapter::{find_string, form_headers, mapped_string};
use crate::core::{OAuthAuthorizeResponse, OAuthError, OAuthTokenSet};
use crate::identity::{
    ExternalIdentity, IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthProvider,
    IdentityOAuthProviderConfig, IdentityOAuthStartContext,
};
use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthNetworkContext};
use async_trait::async_trait;
use url::form_urlencoded;

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
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: response.body_text.chars().take(500).collect(),
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
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: response.body_text.chars().take(500).collect(),
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
            email: identity.email,
            username: identity.username,
            display_name: identity.display_name,
            raw: identity.raw,
        })
    }
}
