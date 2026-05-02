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
        self.inner.map_identity(config, identity)
    }
}
