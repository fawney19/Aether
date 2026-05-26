use super::generic::{
    provider_account_state_from_metadata, template_for_provider_type, GenericProviderOAuthAdapter,
};
use crate::provider::ProviderOAuthAdapter;

pub const ANTIGRAVITY_PROVIDER_TYPE: &str = "antigravity";
pub const ANTIGRAVITY_CLI_PROVIDER_TYPE: &str = "antigravity_cli";

#[derive(Debug, Clone)]
pub struct AntigravityProviderOAuthAdapter {
    inner: GenericProviderOAuthAdapter,
}

impl Default for AntigravityProviderOAuthAdapter {
    fn default() -> Self {
        Self::for_provider_type(ANTIGRAVITY_PROVIDER_TYPE)
    }
}

impl AntigravityProviderOAuthAdapter {
    pub fn for_provider_type(provider_type: &'static str) -> Self {
        Self {
            inner: GenericProviderOAuthAdapter::new(
                template_for_provider_type(provider_type)
                    .expect("antigravity template should exist"),
            ),
        }
    }
}

#[async_trait::async_trait]
impl ProviderOAuthAdapter for AntigravityProviderOAuthAdapter {
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
        self.inner.build_authorize_url(ctx, state, code_challenge)
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
        Ok(Some(provider_account_state_from_metadata(
            self.provider_type(),
            account,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::AntigravityProviderOAuthAdapter;
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::{
        ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthTransportContext,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

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

    #[derive(Clone, Default)]
    struct CapturingExecutor {
        requests: Arc<Mutex<Vec<OAuthHttpRequest>>>,
    }

    #[async_trait]
    impl OAuthHttpExecutor for CapturingExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            self.requests
                .lock()
                .expect("requests should lock")
                .push(request);
            Ok(OAuthHttpResponse {
                status_code: 200,
                body_text: json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 3600
                })
                .to_string(),
                json_body: None,
            })
        }
    }

    fn sample_context() -> ProviderOAuthTransportContext {
        ProviderOAuthTransportContext {
            provider_id: String::new(),
            provider_type: "antigravity".to_string(),
            endpoint_id: None,
            key_id: None,
            auth_type: Some("oauth".to_string()),
            decrypted_api_key: None,
            decrypted_auth_config: None,
            provider_config: None,
            endpoint_config: None,
            key_config: None,
            network: crate::network::OAuthNetworkContext::provider_operation(None),
        }
    }

    #[test]
    fn antigravity_authorize_url_preserves_existing_antigravity_contract() {
        let adapter = AntigravityProviderOAuthAdapter::default();
        let response = adapter
            .build_authorize_url(&sample_context(), "state-1", Some("challenge-1"))
            .expect("authorize url should build");

        let parsed = url::Url::parse(&response.authorize_url).expect("authorize url should parse");
        let params = parsed
            .query_pairs()
            .into_owned()
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            parsed.as_str().split('?').next(),
            Some("https://accounts.google.com/o/oauth2/v2/auth")
        );
        assert_eq!(
            params.get("client_id").map(String::as_str),
            Some("1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com")
        );
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("http://localhost:51121/oauth2callback")
        );
        assert!(!params.contains_key("access_type"));
        assert!(!params.contains_key("prompt"));
        assert_eq!(params.get("state").map(String::as_str), Some("state-1"));
        assert_eq!(
            params.get("code_challenge").map(String::as_str),
            Some("challenge-1")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );

        let scope = params.get("scope").expect("scope should exist");
        assert!(!scope.split_whitespace().any(|item| item == "openid"));
        for expected in [
            "https://www.googleapis.com/auth/cloud-platform",
            "https://www.googleapis.com/auth/userinfo.email",
            "https://www.googleapis.com/auth/userinfo.profile",
            "https://www.googleapis.com/auth/cclog",
            "https://www.googleapis.com/auth/experimentsandconfigs",
        ] {
            assert!(
                scope.split_whitespace().any(|item| item == expected),
                "scope should include {expected}; got {scope}"
            );
        }
    }

    #[tokio::test]
    async fn antigravity_exchange_preserves_existing_redirect_and_scope_replay() {
        let adapter = AntigravityProviderOAuthAdapter::default();
        let executor = CapturingExecutor::default();

        adapter
            .exchange_code(
                &executor,
                &sample_context(),
                "code-1",
                "state-1",
                Some("verifier-1"),
            )
            .await
            .expect("exchange should succeed");

        let requests = executor.requests.lock().expect("requests should lock");
        assert_eq!(requests.len(), 1);
        let body = std::str::from_utf8(
            requests[0]
                .body_bytes
                .as_deref()
                .expect("form body should exist"),
        )
        .expect("form body should be utf8");
        let params = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("http://localhost:51121/oauth2callback")
        );
        assert_eq!(
            params.get("code_verifier").map(String::as_str),
            Some("verifier-1")
        );
        assert!(params.contains_key("scope"));
    }

    #[test]
    fn antigravity_cli_authorize_url_matches_cli_oauth_contract() {
        let adapter = AntigravityProviderOAuthAdapter::for_provider_type("antigravity_cli");
        let mut ctx = sample_context();
        ctx.provider_type = "antigravity_cli".to_string();
        let response = adapter
            .build_authorize_url(&ctx, "state-1", Some("challenge-1"))
            .expect("authorize url should build");

        let parsed = url::Url::parse(&response.authorize_url).expect("authorize url should parse");
        let params = parsed
            .query_pairs()
            .into_owned()
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            parsed.as_str().split('?').next(),
            Some("https://accounts.google.com/o/oauth2/auth")
        );
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("https://antigravity.google/oauth-callback")
        );
        assert_eq!(
            params.get("access_type").map(String::as_str),
            Some("offline")
        );
        assert_eq!(params.get("prompt").map(String::as_str), Some("consent"));
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert!(params
            .get("scope")
            .is_some_and(|scope| scope.split_whitespace().any(|item| item == "openid")));
    }

    #[tokio::test]
    async fn antigravity_cli_exchange_uses_cli_redirect_uri_without_scope_replay() {
        let adapter = AntigravityProviderOAuthAdapter::for_provider_type("antigravity_cli");
        let executor = CapturingExecutor::default();
        let mut ctx = sample_context();
        ctx.provider_type = "antigravity_cli".to_string();

        adapter
            .exchange_code(&executor, &ctx, "code-1", "state-1", Some("verifier-1"))
            .await
            .expect("exchange should succeed");

        let requests = executor.requests.lock().expect("requests should lock");
        assert_eq!(requests.len(), 1);
        let body = std::str::from_utf8(
            requests[0]
                .body_bytes
                .as_deref()
                .expect("form body should exist"),
        )
        .expect("form body should be utf8");
        let params = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some("https://antigravity.google/oauth-callback")
        );
        assert_eq!(
            params.get("code_verifier").map(String::as_str),
            Some("verifier-1")
        );
        assert!(!params.contains_key("scope"));
    }

    #[tokio::test]
    async fn antigravity_probe_marks_forbidden_metadata_invalid() {
        let adapter = AntigravityProviderOAuthAdapter::default();
        let ctx = sample_context();
        let account = ProviderOAuthAccount {
            provider_type: "antigravity".to_string(),
            access_token: "access-token".to_string(),
            auth_config: json!({
                "email": "ag@example.com",
                "antigravity": {
                    "is_forbidden": true,
                    "forbidden_reason": "project blocked"
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

        assert!(!probe.state.is_valid);
        assert_eq!(probe.state.email.as_deref(), Some("ag@example.com"));
        assert_eq!(
            probe.state.invalid_reason.as_deref(),
            Some("project blocked")
        );
    }
}
