use super::{
    ProviderOAuthAdapter, ProviderOAuthImportInput, ProviderOAuthProbeResult,
    ProviderOAuthRequestAuth, ProviderOAuthTokenSet, ProviderOAuthTransportContext,
};
use crate::core::{OAuthAdapterRegistry, OAuthAuthorizeResponse, OAuthError};
use crate::network::OAuthHttpExecutor;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct ProviderOAuthService {
    registry: OAuthAdapterRegistry<dyn ProviderOAuthAdapter>,
}

impl ProviderOAuthService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin_adapters() -> Self {
        use super::providers::{
            AntigravityProviderOAuthAdapter, CodexProviderOAuthAdapter,
            GenericProviderOAuthAdapter, KiroProviderOAuthAdapter,
        };

        let mut service = Self::new()
            .with_adapter(Arc::new(KiroProviderOAuthAdapter::default()))
            .with_adapter(Arc::new(CodexProviderOAuthAdapter::default()))
            .with_adapter(Arc::new(AntigravityProviderOAuthAdapter::default()));
        for provider_type in ["claude_code", "gemini_cli"] {
            if let Some(adapter) = GenericProviderOAuthAdapter::for_provider_type(provider_type) {
                service = service.with_adapter(Arc::new(adapter));
            }
        }
        service
    }

    pub fn with_adapter(mut self, adapter: Arc<dyn ProviderOAuthAdapter>) -> Self {
        self.registry.insert(adapter.provider_type(), adapter);
        self
    }

    pub fn adapter(
        &self,
        provider_type: &str,
    ) -> Result<Arc<dyn ProviderOAuthAdapter>, OAuthError> {
        self.registry
            .get(provider_type)
            .ok_or_else(|| OAuthError::UnsupportedProvider(provider_type.to_string()))
    }

    pub fn build_authorize_url(
        &self,
        ctx: &ProviderOAuthTransportContext,
        state: &str,
        code_challenge: Option<&str>,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        self.adapter(&ctx.provider_type)?
            .build_authorize_url(ctx, state, code_challenge)
    }

    pub async fn exchange_code(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        code: &str,
        state: &str,
        pkce_verifier: Option<&str>,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        self.adapter(&ctx.provider_type)?
            .exchange_code(executor, ctx, code, state, pkce_verifier)
            .await
    }

    pub async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        self.adapter(&ctx.provider_type)?
            .import_credentials(executor, ctx, input)
            .await
    }

    pub async fn refresh(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &super::ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        self.adapter(&ctx.provider_type)?
            .refresh(executor, ctx, account)
            .await
    }

    pub fn resolve_request_auth(
        &self,
        account: &super::ProviderOAuthAccount,
    ) -> Result<ProviderOAuthRequestAuth, OAuthError> {
        self.adapter(&account.provider_type)?
            .resolve_request_auth(account)
    }

    pub async fn probe_account_state(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &super::ProviderOAuthAccount,
    ) -> Result<Option<ProviderOAuthProbeResult>, OAuthError> {
        self.adapter(&account.provider_type)?
            .probe_account_state(executor, ctx, account)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderOAuthService;

    #[test]
    fn builtin_provider_service_registers_supported_provider_types() {
        let service = ProviderOAuthService::with_builtin_adapters();

        for provider_type in ["claude_code", "codex", "gemini_cli", "antigravity", "kiro"] {
            assert!(
                service.adapter(provider_type).is_ok(),
                "{provider_type} adapter should be registered"
            );
        }
        assert!(service.adapter("unknown").is_err());
    }
}
