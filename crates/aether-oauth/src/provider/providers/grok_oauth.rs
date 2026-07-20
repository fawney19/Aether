use super::generic::{GenericProviderOAuthAdapter, GenericProviderOAuthTemplate};
use crate::core::{OAuthAuthorizeResponse, OAuthError};
use crate::network::OAuthHttpExecutor;
use crate::provider::{
    ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthCapabilities,
    ProviderOAuthImportInput, ProviderOAuthRequestAuth, ProviderOAuthTokenSet,
    ProviderOAuthTransportContext,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub const GROK_OAUTH_PROVIDER_TYPE: &str = "grok_oauth";
const GROK_OAUTH_CLI_VERSION: &str = "0.2.93";
const GROK_OAUTH_NONCE_DOMAIN: &[u8] = b"aether:grok-oauth:nonce:v1:";

const GROK_OAUTH_TEMPLATE: GenericProviderOAuthTemplate = GenericProviderOAuthTemplate {
    provider_type: GROK_OAUTH_PROVIDER_TYPE,
    display_name: "Grok OAuth",
    authorize_url: "https://auth.x.ai/oauth2/authorize",
    token_url: "https://auth.x.ai/oauth2/token",
    client_id: "b1a00492-073a-47ea-816f-4c329264a828",
    client_secret: "",
    scopes: &[
        "openid",
        "profile",
        "email",
        "offline_access",
        "grok-cli:access",
        "api:access",
    ],
    redirect_uri: "http://127.0.0.1:56121/callback",
    use_pkce: true,
    uses_json_payload: false,
};

#[derive(Debug, Clone)]
pub struct GrokOAuthProviderOAuthAdapter {
    inner: GenericProviderOAuthAdapter,
}

impl Default for GrokOAuthProviderOAuthAdapter {
    fn default() -> Self {
        Self {
            inner: GenericProviderOAuthAdapter::new(GROK_OAUTH_TEMPLATE),
        }
    }
}

impl GrokOAuthProviderOAuthAdapter {
    #[cfg(test)]
    fn with_token_url_for_tests(mut self, token_url: impl Into<String>) -> Self {
        self.inner = self.inner.with_token_url_for_tests(token_url);
        self
    }

    fn enrich_token_set(mut token_set: ProviderOAuthTokenSet) -> ProviderOAuthTokenSet {
        let token_payload = token_set
            .token_set
            .raw_payload
            .clone()
            .unwrap_or(Value::Null);
        if let Some(auth_config) = token_set.auth_config.as_object_mut() {
            enrich_grok_oauth_auth_config(auth_config, &token_payload);
        }
        token_set
    }
}

#[async_trait::async_trait]
impl ProviderOAuthAdapter for GrokOAuthProviderOAuthAdapter {
    fn provider_type(&self) -> &'static str {
        GROK_OAUTH_PROVIDER_TYPE
    }

    fn capabilities(&self) -> ProviderOAuthCapabilities {
        self.inner.capabilities()
    }

    fn build_authorize_url(
        &self,
        ctx: &ProviderOAuthTransportContext,
        state: &str,
        code_challenge: Option<&str>,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        let mut response = self.inner.build_authorize_url(ctx, state, code_challenge)?;
        let mut url = url::Url::parse(&response.authorize_url)
            .map_err(|_| OAuthError::invalid_response("invalid authorize_url"))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("nonce", &grok_oauth_nonce_from_state(state));
            query.append_pair("plan", "generic");
            query.append_pair("referrer", "sub2api");
        }
        response.authorize_url = url.to_string();
        Ok(response)
    }

    async fn exchange_code(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        code: &str,
        state: &str,
        pkce_verifier: Option<&str>,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        self.inner
            .exchange_code(executor, ctx, code, state, pkce_verifier)
            .await
            .map(Self::enrich_token_set)
    }

    async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        self.inner
            .import_credentials(executor, ctx, input)
            .await
            .map(Self::enrich_token_set)
    }

    async fn refresh(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        self.inner
            .refresh(executor, ctx, account)
            .await
            .map(Self::enrich_token_set)
    }

    fn resolve_request_auth(
        &self,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthRequestAuth, OAuthError> {
        self.inner.resolve_request_auth(account)
    }

    fn account_fingerprint(&self, account: &ProviderOAuthAccount) -> Option<String> {
        self.inner.account_fingerprint(account)
    }
}

/// Applies Grok CLI request identity and merges stable account identity fields.
///
/// ID token claims take precedence over direct token-response fields and access
/// token claims. This keeps opaque access tokens usable while avoiding stale or
/// empty response fields masking the OIDC identity.
pub fn enrich_grok_oauth_auth_config(auth_config: &mut Map<String, Value>, token_payload: &Value) {
    apply_grok_oauth_auth_config_defaults(auth_config);

    let direct_fields = token_payload.as_object();
    let id_token_claims = grok_oauth_token_claims(token_payload, &["id_token", "idToken"]);
    let access_token_claims =
        grok_oauth_token_claims(token_payload, &["access_token", "accessToken"]);
    for field in ["email", "sub", "team_id"] {
        let value = grok_oauth_identity_field(id_token_claims.as_ref(), field)
            .or_else(|| grok_oauth_identity_field(direct_fields, field))
            .or_else(|| grok_oauth_identity_field(access_token_claims.as_ref(), field));
        if let Some(value) = value {
            auth_config.insert(field.to_string(), value);
        }
    }
}

/// Applies the fixed Grok CLI upstream identity to an OAuth account config.
///
/// The protocol headers intentionally replace stale values from prior Aether
/// releases, while unrelated caller-provided headers remain intact. This is
/// limited to request headers so callers can compose it with their own token
/// and identity persistence logic.
pub fn apply_grok_oauth_auth_config_defaults(auth_config: &mut Map<String, Value>) {
    let headers = auth_config
        .entry("headers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !headers.is_object() {
        *headers = Value::Object(Map::new());
    }
    let headers = headers
        .as_object_mut()
        .expect("headers was normalized to an object");
    for required_header in ["X-XAI-Token-Auth", "x-grok-client-version", "User-Agent"] {
        headers.retain(|name, _| !name.eq_ignore_ascii_case(required_header));
    }
    headers.insert("X-XAI-Token-Auth".to_string(), json!("xai-grok-cli"));
    headers.insert(
        "x-grok-client-version".to_string(),
        json!(GROK_OAUTH_CLI_VERSION),
    );
    headers.insert(
        "User-Agent".to_string(),
        json!(format!("xai-grok-workspace/{GROK_OAUTH_CLI_VERSION}")),
    );
}

fn grok_oauth_token_claims(
    token_payload: &Value,
    token_fields: &[&str],
) -> Option<Map<String, Value>> {
    token_fields.iter().find_map(|token_field| {
        token_payload
            .get(*token_field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .and_then(decode_jwt_claims)
    })
}

fn grok_oauth_identity_field(source: Option<&Map<String, Value>>, field: &str) -> Option<Value> {
    source?
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| Value::String(value.to_string()))
}

fn decode_jwt_claims(token: &str) -> Option<Map<String, Value>> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

fn grok_oauth_nonce_from_state(state: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(GROK_OAUTH_NONCE_DOMAIN);
    hasher.update(state.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_grok_oauth_auth_config_defaults, enrich_grok_oauth_auth_config,
        grok_oauth_nonce_from_state, GrokOAuthProviderOAuthAdapter, GROK_OAUTH_TEMPLATE,
    };
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::{
        ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthTransportContext,
    };
    use async_trait::async_trait;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn grok_oauth_template_uses_xai_pkce_flow() {
        assert_eq!(GROK_OAUTH_TEMPLATE.provider_type, "grok_oauth");
        assert_eq!(
            GROK_OAUTH_TEMPLATE.authorize_url,
            "https://auth.x.ai/oauth2/authorize"
        );
        assert_eq!(
            GROK_OAUTH_TEMPLATE.token_url,
            "https://auth.x.ai/oauth2/token"
        );
        assert!(GROK_OAUTH_TEMPLATE.use_pkce);
        assert!(GROK_OAUTH_TEMPLATE.client_secret.is_empty());
        assert!(GROK_OAUTH_TEMPLATE.scopes.contains(&"grok-cli:access"));
    }

    #[test]
    fn grok_oauth_authorize_url_includes_xai_cli_parameters() {
        let adapter = GrokOAuthProviderOAuthAdapter::default();
        let ctx = ProviderOAuthTransportContext {
            provider_id: "provider-1".to_string(),
            provider_type: "grok_oauth".to_string(),
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
        let state = "state-secret-for-test";
        let authorization = adapter
            .build_authorize_url(&ctx, state, Some("challenge-123"))
            .expect("authorization URL should build");
        let url = url::Url::parse(&authorization.authorize_url)
            .expect("authorization URL should be valid");
        let query_value = |key: &str| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };

        assert_eq!(query_value("state").as_deref(), Some(state));
        assert_eq!(
            query_value("code_challenge").as_deref(),
            Some("challenge-123")
        );
        assert_eq!(
            query_value("code_challenge_method").as_deref(),
            Some("S256")
        );
        assert_eq!(query_value("plan").as_deref(), Some("generic"));
        assert_eq!(query_value("referrer").as_deref(), Some("sub2api"));

        let nonce = query_value("nonce").expect("nonce should be present");
        assert_eq!(nonce, grok_oauth_nonce_from_state(state));
        assert_ne!(nonce, state);
        assert!(!nonce.contains(state));
    }

    #[test]
    fn grok_oauth_header_defaults_replace_stale_cli_values_and_preserve_other_headers() {
        let mut auth_config = serde_json::Map::new();
        auth_config.insert(
            "headers".to_string(),
            json!({
                "User-Agent": "aether-grok-oauth/1.0",
                "X-XAI-Token-Auth": "wrong-token-auth",
                "X-Grok-Client-Version": "0.0.0",
                "X-Custom-Header": "keep-me"
            }),
        );

        apply_grok_oauth_auth_config_defaults(&mut auth_config);

        let headers = auth_config["headers"]
            .as_object()
            .expect("headers should be an object");
        assert_eq!(
            headers
                .get("X-XAI-Token-Auth")
                .and_then(|value| value.as_str()),
            Some("xai-grok-cli")
        );
        assert_eq!(
            headers
                .get("x-grok-client-version")
                .and_then(|value| value.as_str()),
            Some("0.2.93")
        );
        assert_eq!(
            headers.get("User-Agent").and_then(|value| value.as_str()),
            Some("xai-grok-workspace/0.2.93")
        );
        assert_eq!(
            headers
                .get("X-Custom-Header")
                .and_then(|value| value.as_str()),
            Some("keep-me")
        );
        for required_header in ["X-XAI-Token-Auth", "x-grok-client-version", "User-Agent"] {
            assert_eq!(
                headers
                    .keys()
                    .filter(|name| name.eq_ignore_ascii_case(required_header))
                    .count(),
                1,
                "{required_header} should have one canonical value"
            );
        }
    }

    #[test]
    fn grok_oauth_identity_injects_cli_headers_and_jwt_claims() {
        let claims = json!({
            "email": "user@x.ai",
            "sub": "subject-123",
            "team_id": "team-456"
        });
        let token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims should encode"))
        );
        let mut auth_config = serde_json::Map::new();

        enrich_grok_oauth_auth_config(&mut auth_config, &json!({ "access_token": token }));

        let headers = auth_config
            .get("headers")
            .and_then(|value| value.as_object())
            .expect("headers should be injected");
        assert_eq!(
            headers.get("User-Agent").and_then(|v| v.as_str()),
            Some("xai-grok-workspace/0.2.93")
        );
        assert_eq!(
            headers
                .get("x-grok-client-version")
                .and_then(|v| v.as_str()),
            Some("0.2.93")
        );
        assert_eq!(
            headers.get("X-XAI-Token-Auth").and_then(|v| v.as_str()),
            Some("xai-grok-cli")
        );
        assert_eq!(
            auth_config.get("email").and_then(|v| v.as_str()),
            Some("user@x.ai")
        );
        assert_eq!(
            auth_config.get("sub").and_then(|v| v.as_str()),
            Some("subject-123")
        );
        assert_eq!(
            auth_config.get("team_id").and_then(|v| v.as_str()),
            Some("team-456")
        );
    }

    #[test]
    fn grok_oauth_identity_uses_id_token_when_access_token_is_opaque() {
        let claims = json!({
            "email": "user@x.ai",
            "sub": "subject-123",
            "team_id": "team-456"
        });
        let id_token = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims should encode"))
        );
        let mut auth_config = serde_json::Map::new();

        enrich_grok_oauth_auth_config(
            &mut auth_config,
            &json!({
                "access_token": "opaque-access-token",
                "id_token": id_token
            }),
        );

        assert_eq!(auth_config.get("email"), Some(&json!("user@x.ai")));
        assert_eq!(auth_config.get("sub"), Some(&json!("subject-123")));
        assert_eq!(auth_config.get("team_id"), Some(&json!("team-456")));
    }

    #[test]
    fn grok_oauth_identity_prefers_id_token_and_fills_missing_access_token_claims() {
        let id_claims = json!({
            "email": "id-token@x.ai",
            "sub": "id-token-subject"
        });
        let access_claims = json!({
            "email": "access-token@x.ai",
            "sub": "access-token-subject",
            "team_id": "access-token-team"
        });
        let jwt = |claims: &Value| {
            format!(
                "header.{}.signature",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims should encode"))
            )
        };
        let mut auth_config = serde_json::Map::new();

        enrich_grok_oauth_auth_config(
            &mut auth_config,
            &json!({
                "email": "top-level@x.ai",
                "account_name": "Grok subscription",
                "id_token": jwt(&id_claims),
                "access_token": jwt(&access_claims)
            }),
        );

        assert_eq!(auth_config.get("email"), Some(&json!("id-token@x.ai")));
        assert_eq!(auth_config.get("sub"), Some(&json!("id-token-subject")));
        assert_eq!(
            auth_config.get("team_id"),
            Some(&json!("access-token-team"))
        );
    }

    #[derive(Debug, Clone)]
    struct StaticExecutor {
        seen_request: Arc<Mutex<Option<OAuthHttpRequest>>>,
    }

    #[async_trait]
    impl OAuthHttpExecutor for StaticExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            *self.seen_request.lock().expect("mutex should lock") = Some(request);
            Ok(OAuthHttpResponse {
                status_code: 200,
                body_text: json!({
                    "access_token": "new-access-token",
                    "expires_in": 3600
                })
                .to_string(),
                json_body: None,
            })
        }
    }

    #[tokio::test]
    async fn grok_oauth_refresh_rebuilds_cli_identity_headers() {
        let executor = StaticExecutor {
            seen_request: Arc::new(Mutex::new(None)),
        };
        let adapter = GrokOAuthProviderOAuthAdapter::default()
            .with_token_url_for_tests("https://auth.example.test/token");
        let ctx = ProviderOAuthTransportContext {
            provider_id: "provider-grok-oauth".to_string(),
            provider_type: "grok_oauth".to_string(),
            endpoint_id: None,
            key_id: Some("key-grok-oauth".to_string()),
            auth_type: Some("oauth".to_string()),
            decrypted_api_key: None,
            decrypted_auth_config: None,
            provider_config: None,
            endpoint_config: None,
            key_config: None,
            network: crate::network::OAuthNetworkContext::provider_operation(None),
        };
        let account = ProviderOAuthAccount {
            provider_type: "grok_oauth".to_string(),
            access_token: "old-access-token".to_string(),
            auth_config: json!({
                "provider_type": "grok_oauth",
                "refresh_token": "old-refresh-token",
                "headers": {
                    "X-XAI-Token-Auth": "stale",
                    "X-Grok-Client-Version": "0.0.0",
                    "User-Agent": "aether-grok-oauth/1.0"
                }
            }),
            expires_at_unix_secs: Some(1),
            identity: BTreeMap::new(),
        };

        let refreshed = adapter
            .refresh(&executor, &ctx, &account)
            .await
            .expect("refresh should succeed");

        assert_eq!(
            refreshed.auth_config["headers"]["X-XAI-Token-Auth"],
            "xai-grok-cli"
        );
        assert_eq!(
            refreshed.auth_config["headers"]["x-grok-client-version"],
            "0.2.93"
        );
        assert_eq!(
            refreshed.auth_config["headers"]["User-Agent"],
            "xai-grok-workspace/0.2.93"
        );
        assert_eq!(
            refreshed.token_set.refresh_token.as_deref(),
            Some("old-refresh-token")
        );
    }
}
