use super::{
    IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthProvider,
    IdentityOAuthProviderConfig, IdentityOAuthStartContext,
};
use crate::core::{OAuthAdapterRegistry, OAuthAuthorizeResponse, OAuthError};
use crate::network::OAuthHttpExecutor;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct IdentityOAuthService {
    registry: OAuthAdapterRegistry<dyn IdentityOAuthProvider>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthLoginOutcome {
    pub claims: IdentityClaims,
    pub is_new_external_identity: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoundOAuthIdentity {
    pub claims: IdentityClaims,
    pub replaced_existing_binding: bool,
}

impl IdentityOAuthService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin_providers() -> Self {
        use super::providers::{CustomOidcIdentityOAuthProvider, LinuxDoIdentityOAuthProvider};

        Self::new()
            .with_provider(Arc::new(LinuxDoIdentityOAuthProvider::default()))
            .with_provider(Arc::new(CustomOidcIdentityOAuthProvider))
    }

    pub fn with_provider(mut self, provider: Arc<dyn IdentityOAuthProvider>) -> Self {
        self.registry.insert(provider.provider_type(), provider);
        self
    }

    pub fn provider(
        &self,
        provider_type: &str,
    ) -> Result<Arc<dyn IdentityOAuthProvider>, OAuthError> {
        self.registry
            .get(provider_type)
            .ok_or_else(|| OAuthError::UnsupportedProvider(provider_type.to_string()))
    }

    pub fn start(
        &self,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthStartContext,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        self.provider(&config.provider_type)?
            .build_authorize_url(config, ctx)
    }

    pub async fn login(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<OAuthLoginOutcome, OAuthError> {
        let provider = self.provider(&config.provider_type)?;
        login_with_oauth(provider.as_ref(), executor, config, ctx).await
    }

    pub async fn bind(
        &self,
        executor: &dyn OAuthHttpExecutor,
        config: &IdentityOAuthProviderConfig,
        ctx: &IdentityOAuthExchangeContext,
    ) -> Result<BoundOAuthIdentity, OAuthError> {
        let provider = self.provider(&config.provider_type)?;
        bind_oauth_identity(provider.as_ref(), executor, config, ctx).await
    }
}

pub fn start_identity_oauth(
    provider: &dyn IdentityOAuthProvider,
    config: &IdentityOAuthProviderConfig,
    ctx: &IdentityOAuthStartContext,
) -> Result<OAuthAuthorizeResponse, OAuthError> {
    provider.build_authorize_url(config, ctx)
}

pub async fn login_with_oauth(
    provider: &dyn IdentityOAuthProvider,
    executor: &dyn OAuthHttpExecutor,
    config: &IdentityOAuthProviderConfig,
    ctx: &IdentityOAuthExchangeContext,
) -> Result<OAuthLoginOutcome, OAuthError> {
    let tokens = provider.exchange_code(executor, config, ctx).await?;
    let identity = provider
        .fetch_identity(executor, config, &tokens, ctx.network.clone())
        .await?;
    let claims = provider.map_identity(config, identity)?;
    Ok(OAuthLoginOutcome {
        claims,
        is_new_external_identity: false,
    })
}

pub async fn bind_oauth_identity(
    provider: &dyn IdentityOAuthProvider,
    executor: &dyn OAuthHttpExecutor,
    config: &IdentityOAuthProviderConfig,
    ctx: &IdentityOAuthExchangeContext,
) -> Result<BoundOAuthIdentity, OAuthError> {
    let tokens = provider.exchange_code(executor, config, ctx).await?;
    let identity = provider
        .fetch_identity(executor, config, &tokens, ctx.network.clone())
        .await?;
    let claims = provider.map_identity(config, identity)?;
    Ok(BoundOAuthIdentity {
        claims,
        replaced_existing_binding: false,
    })
}

#[cfg(test)]
mod tests {
    use super::IdentityOAuthService;

    #[test]
    fn builtin_identity_service_registers_login_and_custom_oidc_providers() {
        let service = IdentityOAuthService::with_builtin_providers();

        assert!(service.provider("linuxdo").is_ok());
        assert!(service.provider("custom_oidc").is_ok());
        assert!(service.provider("missing").is_err());
    }
}
