use super::{
    ProviderOAuthAccount, ProviderOAuthAccountState, ProviderOAuthCapabilities,
    ProviderOAuthImportInput, ProviderOAuthRequestAuth, ProviderOAuthTokenSet,
    ProviderOAuthTransportContext,
};
use crate::core::{OAuthAuthorizeResponse, OAuthError};
use crate::network::OAuthHttpExecutor;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderOAuthProbeResult {
    pub state: ProviderOAuthAccountState,
}

#[async_trait]
pub trait ProviderOAuthAdapter: Send + Sync {
    fn provider_type(&self) -> &'static str;

    fn capabilities(&self) -> ProviderOAuthCapabilities;

    fn build_authorize_url(
        &self,
        _ctx: &ProviderOAuthTransportContext,
        _state: &str,
        _code_challenge: Option<&str>,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        Err(OAuthError::UnsupportedProvider(
            self.provider_type().to_string(),
        ))
    }

    async fn exchange_code(
        &self,
        _executor: &dyn OAuthHttpExecutor,
        _ctx: &ProviderOAuthTransportContext,
        _code: &str,
        _state: &str,
        _pkce_verifier: Option<&str>,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        Err(OAuthError::UnsupportedProvider(
            self.provider_type().to_string(),
        ))
    }

    async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError>;

    async fn refresh(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError>;

    fn resolve_request_auth(
        &self,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthRequestAuth, OAuthError>;

    fn account_fingerprint(&self, account: &ProviderOAuthAccount) -> Option<String>;

    async fn probe_account_state(
        &self,
        _executor: &dyn OAuthHttpExecutor,
        _ctx: &ProviderOAuthTransportContext,
        _account: &ProviderOAuthAccount,
    ) -> Result<Option<ProviderOAuthProbeResult>, OAuthError> {
        Ok(None)
    }
}
