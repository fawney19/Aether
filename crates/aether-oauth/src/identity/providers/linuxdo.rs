use super::custom_oidc::CustomOidcIdentityOAuthProvider;
use crate::core::{OAuthAuthorizeResponse, OAuthError, OAuthTokenSet};
use crate::identity::{
    ExternalIdentity, IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthProvider,
    IdentityOAuthProviderConfig, IdentityOAuthStartContext,
};
use crate::network::{OAuthHttpExecutor, OAuthNetworkContext};
use async_trait::async_trait;

#[derive(Debug, Clone, Default)]
pub struct LinuxDoIdentityOAuthProvider {
    inner: CustomOidcIdentityOAuthProvider,
}

#[async_trait]
impl IdentityOAuthProvider for LinuxDoIdentityOAuthProvider {
    fn provider_type(&self) -> &'static str {
        "linuxdo"
    }

    fn build_authorize_url(
        &self,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthStartContext,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        self.inner.build_authorize_url(config, ctx)
    }

    async fn exchange_code(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<OAuthTokenSet, OAuthError> {
        self.inner.exchange_code(executor, config, ctx).await
    }

    async fn fetch_identity(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        tokens: &OAuthTokenSet,
        network: OAuthNetworkContext,
    ) -> Result<ExternalIdentity, OAuthError> {
        self.inner
            .fetch_identity(executor, config, tokens, network)
            .await
    }

    fn map_identity(
        &self,
        config: &IdentityOAuthProviderConfig,
        identity: ExternalIdentity,
    ) -> Result<IdentityClaims, OAuthError> {
        let mut claims = self.inner.map_identity(config, identity)?;
        // Linux.do's OAuth user endpoint does not provide an OIDC-level guarantee
        // for the email verification claim, so it must not verify a local email.
        claims.email_verified = false;
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::LinuxDoIdentityOAuthProvider;
    use crate::identity::{ExternalIdentity, IdentityOAuthProvider, IdentityOAuthProviderConfig};
    use serde_json::json;

    #[test]
    fn linuxdo_does_not_promote_an_unverified_provider_assertion() {
        let provider = LinuxDoIdentityOAuthProvider::default();
        let config = IdentityOAuthProviderConfig {
            provider_type: "linuxdo".to_string(),
            display_name: "Linux.do".to_string(),
            authorization_url: "https://connect.linux.do/oauth2/authorize".to_string(),
            token_url: "https://connect.linux.do/oauth2/token".to_string(),
            userinfo_url: Some("https://connect.linux.do/api/user".to_string()),
            client_id: "client".to_string(),
            client_secret: None,
            scopes: vec![],
            redirect_uri: "https://gateway.example.test/callback".to_string(),
            frontend_callback_url: "https://app.example.test/callback".to_string(),
            attribute_mapping: None,
            extra_config: None,
        };
        let claims = provider
            .map_identity(
                &config,
                ExternalIdentity {
                    provider_type: "linuxdo".to_string(),
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

        assert!(!claims.email_verified);
    }
}
