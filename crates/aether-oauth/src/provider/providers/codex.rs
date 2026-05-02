use super::generic::{
    provider_account_state_from_metadata, template_for_provider_type, GenericProviderOAuthAdapter,
};
use crate::provider::ProviderOAuthAdapter;

#[derive(Debug, Clone)]
pub struct CodexProviderOAuthAdapter {
    inner: GenericProviderOAuthAdapter,
}

impl Default for CodexProviderOAuthAdapter {
    fn default() -> Self {
        Self {
            inner: GenericProviderOAuthAdapter::new(
                template_for_provider_type("codex").expect("codex template should exist"),
            ),
        }
    }
}

#[async_trait::async_trait]
impl ProviderOAuthAdapter for CodexProviderOAuthAdapter {
    fn provider_type(&self) -> &'static str {
        self.inner.provider_type()
    }

    fn capabilities(&self) -> crate::provider::ProviderOAuthCapabilities {
        crate::provider::ProviderOAuthCapabilities {
            supports_account_probe: true,
            ..self.inner.capabilities()
        }
    }

    fn build_authorize_url(
        &self,
        ctx: &crate::provider::ProviderOAuthTransportContext,
        state: &str,
        code_challenge: Option<&str>,
    ) -> Result<crate::core::OAuthAuthorizeResponse, crate::core::OAuthError> {
        let mut response = self.inner.build_authorize_url(ctx, state, code_challenge)?;
        let mut url = url::Url::parse(&response.authorize_url)
            .map_err(|_| crate::core::OAuthError::invalid_response("invalid authorize_url"))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("prompt", "login");
            query.append_pair("id_token_add_organizations", "true");
            query.append_pair("codex_cli_simplified_flow", "true");
        }
        response.authorize_url = url.to_string();
        Ok(response)
    }

    async fn exchange_code(
        &self,
        executor: &dyn crate::network::OAuthHttpExecutor,
        ctx: &crate::provider::ProviderOAuthTransportContext,
        code: &str,
        state: &str,
        pkce_verifier: Option<&str>,
    ) -> Result<crate::provider::ProviderOAuthTokenSet, crate::core::OAuthError> {
        self.inner
            .exchange_code(executor, ctx, code, state, pkce_verifier)
            .await
    }

    async fn import_credentials(
        &self,
        executor: &dyn crate::network::OAuthHttpExecutor,
        ctx: &crate::provider::ProviderOAuthTransportContext,
        input: crate::provider::ProviderOAuthImportInput,
    ) -> Result<crate::provider::ProviderOAuthTokenSet, crate::core::OAuthError> {
        self.inner.import_credentials(executor, ctx, input).await
    }

    async fn refresh(
        &self,
        executor: &dyn crate::network::OAuthHttpExecutor,
        ctx: &crate::provider::ProviderOAuthTransportContext,
        account: &crate::provider::ProviderOAuthAccount,
    ) -> Result<crate::provider::ProviderOAuthTokenSet, crate::core::OAuthError> {
        self.inner.refresh(executor, ctx, account).await
    }

    fn resolve_request_auth(
        &self,
        account: &crate::provider::ProviderOAuthAccount,
    ) -> Result<crate::provider::ProviderOAuthRequestAuth, crate::core::OAuthError> {
        self.inner.resolve_request_auth(account)
    }

    fn account_fingerprint(
        &self,
        account: &crate::provider::ProviderOAuthAccount,
    ) -> Option<String> {
        self.inner.account_fingerprint(account)
    }

    async fn probe_account_state(
        &self,
        _executor: &dyn crate::network::OAuthHttpExecutor,
        _ctx: &crate::provider::ProviderOAuthTransportContext,
        account: &crate::provider::ProviderOAuthAccount,
    ) -> Result<Option<crate::provider::ProviderOAuthProbeResult>, crate::core::OAuthError> {
        Ok(Some(provider_account_state_from_metadata("codex", account)))
    }
}

#[cfg(test)]
mod tests {
    use super::CodexProviderOAuthAdapter;
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::{
        ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthTransportContext,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;

    struct UnusedExecutor;

    #[async_trait]
    impl OAuthHttpExecutor for UnusedExecutor {
        async fn execute(
            &self,
            _request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            unreachable!("metadata probe should not execute network requests")
        }
    }

    #[test]
    fn codex_authorize_url_includes_cli_flow_hints() {
        let adapter = CodexProviderOAuthAdapter::default();
        let ctx = ProviderOAuthTransportContext {
            provider_id: String::new(),
            provider_type: "codex".to_string(),
            endpoint_id: None,
            key_id: None,
            auth_type: Some("oauth".to_string()),
            decrypted_api_key: None,
            decrypted_auth_config: None,
            provider_config: None,
            endpoint_config: None,
            key_config: None,
            network: crate::network::OAuthNetworkContext::provider_operation(None),
        };

        let response = adapter
            .build_authorize_url(&ctx, "state-1", Some("challenge-1"))
            .expect("authorize url should build");

        assert!(response.authorize_url.contains("prompt=login"));
        assert!(response
            .authorize_url
            .contains("id_token_add_organizations=true"));
        assert!(response
            .authorize_url
            .contains("codex_cli_simplified_flow=true"));
    }

    #[tokio::test]
    async fn codex_probe_reports_metadata_quota_and_email() {
        let adapter = CodexProviderOAuthAdapter::default();
        let ctx = ProviderOAuthTransportContext {
            provider_id: String::new(),
            provider_type: "codex".to_string(),
            endpoint_id: None,
            key_id: None,
            auth_type: Some("oauth".to_string()),
            decrypted_api_key: None,
            decrypted_auth_config: None,
            provider_config: None,
            endpoint_config: None,
            key_config: None,
            network: crate::network::OAuthNetworkContext::provider_operation(None),
        };
        let account = ProviderOAuthAccount {
            provider_type: "codex".to_string(),
            access_token: "access-token".to_string(),
            auth_config: json!({
                "email": "alice@example.com",
                "codex": {
                    "remaining_percent": 42,
                    "updated_at": 1000
                }
            }),
            expires_at_unix_secs: Some(2000),
            identity: BTreeMap::new(),
        };

        let probe = adapter
            .probe_account_state(&UnusedExecutor, &ctx, &account)
            .await
            .expect("probe should succeed")
            .expect("probe should return state");

        assert!(probe.state.is_valid);
        assert_eq!(probe.state.email.as_deref(), Some("alice@example.com"));
        assert_eq!(probe.state.quota.as_ref().unwrap()["remaining_percent"], 42);
    }
}
