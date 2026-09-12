use super::generic::{
    provider_account_state_from_metadata, template_for_provider_type, GenericProviderOAuthAdapter,
};
use crate::core::OAuthError;
use crate::network::{OAuthHttpExecutor, OAuthHttpRequest};
use crate::provider::{ProviderOAuthAdapter, ProviderOAuthTokenSet, ProviderOAuthTransportContext};
use serde_json::Value;
use std::collections::BTreeMap;

pub const ANTIGRAVITY_USER_INFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";

#[derive(Debug, Clone)]
pub struct AntigravityProviderOAuthAdapter {
    inner: GenericProviderOAuthAdapter,
    user_info_url: String,
}

impl Default for AntigravityProviderOAuthAdapter {
    fn default() -> Self {
        Self {
            inner: GenericProviderOAuthAdapter::new(
                template_for_provider_type("antigravity")
                    .expect("antigravity template should exist"),
            ),
            user_info_url: ANTIGRAVITY_USER_INFO_URL.to_string(),
        }
    }
}

impl AntigravityProviderOAuthAdapter {
    /// Supply deterministic OAuth client credentials for tests without
    /// requiring a process-wide environment variable. Production callers
    /// continue to resolve the secret from the configured environment.
    #[doc(hidden)]
    pub fn with_oauth_credentials_for_tests(
        mut self,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        self.inner = self
            .inner
            .with_oauth_credentials_for_tests(client_id, client_secret);
        self
    }

    pub fn with_token_url_override(mut self, token_url: impl Into<String>) -> Self {
        self.inner = self.inner.with_token_url_override(token_url);
        self
    }

    pub fn with_user_info_url_override(mut self, user_info_url: impl Into<String>) -> Self {
        self.user_info_url = user_info_url.into();
        self
    }

    async fn enrich_google_identity(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        mut result: ProviderOAuthTokenSet,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        if result
            .auth_config
            .get("email")
            .and_then(Value::as_str)
            .is_some_and(|email| !email.trim().is_empty())
        {
            return Ok(result);
        }

        let response = executor
            .execute(OAuthHttpRequest {
                request_id: "provider-oauth:antigravity-user-info".to_string(),
                method: reqwest::Method::GET,
                url: self.user_info_url.clone(),
                headers: BTreeMap::from([
                    ("accept".to_string(), "application/json".to_string()),
                    (
                        "authorization".to_string(),
                        result.token_set.bearer_header_value(),
                    ),
                ]),
                content_type: None,
                json_body: None,
                body_bytes: None,
                network: ctx.network.clone(),
                transport_profile: None,
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: response.body_text.trim().chars().take(500).collect(),
            });
        }

        let profile = response
            .json_body
            .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
            .ok_or_else(|| OAuthError::invalid_response("userinfo response is not json"))?;
        if profile.get("verified_email").and_then(Value::as_bool) == Some(false) {
            return Err(OAuthError::invalid_response(
                "userinfo response returned an unverified email",
            ));
        }
        let email = profile
            .get("email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .ok_or_else(|| OAuthError::invalid_response("userinfo response missing email"))?
            .to_string();

        if let Some(auth_config) = result.auth_config.as_object_mut() {
            auth_config.insert("email".to_string(), Value::String(email.clone()));
        }
        if let Some(token_payload) = result
            .token_set
            .raw_payload
            .as_mut()
            .and_then(Value::as_object_mut)
        {
            token_payload.insert("email".to_string(), Value::String(email));
        }
        Ok(result)
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
        let mut response = self.inner.build_authorize_url(ctx, state, code_challenge)?;
        let mut url = url::Url::parse(&response.authorize_url).map_err(|_| {
            crate::core::OAuthError::invalid_request("authorize_url must be absolute")
        })?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("access_type", "offline");
            query.append_pair("prompt", "consent");
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
        let result = self
            .inner
            .exchange_code(executor, ctx, code, state, pkce_verifier)
            .await?;
        self.enrich_google_identity(executor, ctx, result).await
    }

    async fn import_credentials(
        &self,
        executor: &dyn crate::network::OAuthHttpExecutor,
        ctx: &crate::provider::ProviderOAuthTransportContext,
        input: crate::provider::ProviderOAuthImportInput,
    ) -> Result<crate::provider::ProviderOAuthTokenSet, crate::core::OAuthError> {
        let result = self.inner.import_credentials(executor, ctx, input).await?;
        self.enrich_google_identity(executor, ctx, result).await
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
            "antigravity",
            account,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{AntigravityProviderOAuthAdapter, ANTIGRAVITY_USER_INFO_URL};
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::{
        ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthImportInput,
        ProviderOAuthTransportContext,
    };
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    struct UnusedExecutor;

    #[derive(Default)]
    struct GoogleOAuthExecutor {
        requests: Mutex<Vec<OAuthHttpRequest>>,
        token_payload: Option<Value>,
        user_info_response: Option<OAuthHttpResponse>,
    }

    fn transport_context() -> ProviderOAuthTransportContext {
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

    #[async_trait]
    impl OAuthHttpExecutor for UnusedExecutor {
        async fn execute(
            &self,
            _request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            unreachable!("metadata probe should not execute network requests")
        }
    }

    #[async_trait]
    impl OAuthHttpExecutor for GoogleOAuthExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            let request_id = request.request_id.clone();
            self.requests
                .lock()
                .expect("requests should lock")
                .push(request);
            match request_id.as_str() {
                "provider-oauth:exchange-code" | "provider-oauth:refresh-token" => {
                    Ok(OAuthHttpResponse {
                        status_code: 200,
                        body_text: self
                            .token_payload
                            .clone()
                            .unwrap_or_else(|| {
                                json!({
                                    "access_token": "google-access-token",
                                    "refresh_token": "google-refresh-token",
                                    "token_type": "Bearer",
                                    "expires_in": 3600
                                })
                            })
                            .to_string(),
                        json_body: None,
                    })
                }
                "provider-oauth:antigravity-user-info" => Ok(self
                    .user_info_response
                    .clone()
                    .unwrap_or_else(|| OAuthHttpResponse {
                        status_code: 200,
                        body_text: json!({
                            "email": "antigravity@example.com",
                            "verified_email": true
                        })
                        .to_string(),
                        json_body: None,
                    })),
                other => panic!("unexpected OAuth request: {other}"),
            }
        }
    }

    #[test]
    fn antigravity_authorize_requests_offline_refresh_token() {
        let adapter = AntigravityProviderOAuthAdapter::default()
            .with_oauth_credentials_for_tests("test-client-id", "test-client-secret");
        let response = adapter
            .build_authorize_url(&transport_context(), "state-1", Some("challenge-1"))
            .expect("authorize url should build");
        let url = url::Url::parse(&response.authorize_url).expect("authorize url should parse");
        let query = url.query_pairs().collect::<BTreeMap<_, _>>();

        assert_eq!(
            query.get("access_type").map(|value| value.as_ref()),
            Some("offline")
        );
        assert_eq!(
            query.get("prompt").map(|value| value.as_ref()),
            Some("consent")
        );
        assert_eq!(
            query.get("code_challenge").map(|value| value.as_ref()),
            Some("challenge-1")
        );
    }

    #[tokio::test]
    async fn antigravity_exchange_fetches_google_email_for_account_identity() {
        let adapter = AntigravityProviderOAuthAdapter::default()
            .with_oauth_credentials_for_tests("test-client-id", "test-client-secret");
        let ctx = transport_context();
        let executor = GoogleOAuthExecutor::default();

        let result = adapter
            .exchange_code(
                &executor,
                &ctx,
                "authorization-code",
                "state-1",
                Some("verifier-1"),
            )
            .await
            .expect("Antigravity OAuth exchange should succeed");

        assert_eq!(
            result.auth_config.get("email"),
            Some(&json!("antigravity@example.com"))
        );
        assert_eq!(
            result
                .token_set
                .raw_payload
                .as_ref()
                .and_then(|payload| payload.get("email")),
            Some(&json!("antigravity@example.com"))
        );
        let requests = executor.requests.lock().expect("requests should lock");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].url, ANTIGRAVITY_USER_INFO_URL);
        assert_eq!(requests[1].method, reqwest::Method::GET);
        assert_eq!(
            requests[1].headers.get("authorization").map(String::as_str),
            Some("Bearer google-access-token")
        );
        assert_eq!(requests[1].network, ctx.network);
    }

    #[tokio::test]
    async fn antigravity_import_fetches_google_email_for_account_identity() {
        let adapter = AntigravityProviderOAuthAdapter::default()
            .with_oauth_credentials_for_tests("test-client-id", "test-client-secret");
        let ctx = transport_context();
        let executor = GoogleOAuthExecutor::default();

        let result = adapter
            .import_credentials(
                &executor,
                &ctx,
                ProviderOAuthImportInput {
                    provider_type: "antigravity".to_string(),
                    name: None,
                    refresh_token: Some("google-refresh-token".to_string()),
                    raw_credentials: None,
                    network: ctx.network.clone(),
                },
            )
            .await
            .expect("Antigravity OAuth import should succeed");

        assert_eq!(result.auth_config["email"], "antigravity@example.com");
        assert_eq!(
            result
                .token_set
                .raw_payload
                .as_ref()
                .and_then(|payload| payload.get("email")),
            Some(&json!("antigravity@example.com"))
        );
        let requests = executor.requests.lock().expect("requests should lock");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].request_id, "provider-oauth:refresh-token");
        assert_eq!(requests[1].url, ANTIGRAVITY_USER_INFO_URL);
        assert_eq!(requests[1].method, reqwest::Method::GET);
        assert_eq!(
            requests[1].headers.get("authorization").map(String::as_str),
            Some("Bearer google-access-token")
        );
        assert_eq!(requests[1].network, ctx.network);
    }

    #[tokio::test]
    async fn antigravity_import_skips_userinfo_when_token_payload_has_email() {
        let adapter = AntigravityProviderOAuthAdapter::default()
            .with_oauth_credentials_for_tests("test-client-id", "test-client-secret");
        let ctx = transport_context();
        let executor = GoogleOAuthExecutor {
            token_payload: Some(json!({
                "access_token": "google-access-token",
                "email": "token-email@example.com"
            })),
            ..Default::default()
        };

        let result = adapter
            .import_credentials(
                &executor,
                &ctx,
                ProviderOAuthImportInput {
                    provider_type: "antigravity".to_string(),
                    name: None,
                    refresh_token: Some("google-refresh-token".to_string()),
                    raw_credentials: None,
                    network: ctx.network.clone(),
                },
            )
            .await
            .expect("Antigravity OAuth import should succeed");

        assert_eq!(result.auth_config["email"], "token-email@example.com");
        assert_eq!(
            executor
                .requests
                .lock()
                .expect("requests should lock")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn antigravity_import_rejects_unavailable_or_invalid_google_identity() {
        let adapter = AntigravityProviderOAuthAdapter::default()
            .with_oauth_credentials_for_tests("test-client-id", "test-client-secret");
        let ctx = transport_context();

        for (status_code, profile) in [
            (401, json!({"error": "unauthorized"})),
            (200, json!({})),
            (200, json!({"email": "  "})),
            (
                200,
                json!({"email": "unverified@example.com", "verified_email": false}),
            ),
        ] {
            let executor = GoogleOAuthExecutor {
                user_info_response: Some(OAuthHttpResponse {
                    status_code,
                    body_text: profile.to_string(),
                    json_body: None,
                }),
                ..Default::default()
            };

            let result = adapter
                .import_credentials(
                    &executor,
                    &ctx,
                    ProviderOAuthImportInput {
                        provider_type: "antigravity".to_string(),
                        name: None,
                        refresh_token: Some("google-refresh-token".to_string()),
                        raw_credentials: None,
                        network: ctx.network.clone(),
                    },
                )
                .await;

            assert!(
                result.is_err(),
                "invalid Google identity should reject import: {profile}"
            );
        }
    }

    #[tokio::test]
    async fn antigravity_probe_marks_forbidden_metadata_invalid() {
        let adapter = AntigravityProviderOAuthAdapter::default();
        let ctx = transport_context();
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
