use crate::core::{
    current_unix_secs, redacted_oauth_error_body_excerpt, OAuthAuthorizeResponse, OAuthError,
    OAuthTokenSet,
};
use crate::network::{OAuthHttpExecutor, OAuthHttpRequest};
use crate::provider::ProviderOAuthAdapter;
use crate::provider::{
    ProviderOAuthAccount, ProviderOAuthAccountState, ProviderOAuthCapabilities,
    ProviderOAuthImportInput, ProviderOAuthProbeResult, ProviderOAuthRequestAuth,
    ProviderOAuthTokenSet, ProviderOAuthTransportContext,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use url::form_urlencoded;

use super::claude_code::{
    CLAUDE_CODE_AUTHORIZE_URL, CLAUDE_CODE_CLIENT_ID, CLAUDE_CODE_OAUTH_SCOPES,
    CLAUDE_CODE_PROVIDER_TYPE, CLAUDE_CODE_REDIRECT_URI, CLAUDE_CODE_TOKEN_URL,
};

pub const GEMINI_CLI_OAUTH_CLIENT_ID_ENV: &str = "AETHER_GEMINI_CLI_OAUTH_CLIENT_ID";
pub const GEMINI_CLI_OAUTH_CLIENT_SECRET_ENV: &str = "AETHER_GEMINI_CLI_OAUTH_CLIENT_SECRET";
pub const ANTIGRAVITY_OAUTH_CLIENT_ID_ENV: &str = "AETHER_ANTIGRAVITY_OAUTH_CLIENT_ID";
pub const ANTIGRAVITY_OAUTH_CLIENT_SECRET_ENV: &str = "AETHER_ANTIGRAVITY_OAUTH_CLIENT_SECRET";
const CODEX_IDENTITY_FINGERPRINT_FIELD: &str = "codex_identity_fingerprint";
const CODEX_IDENTITY_FINGERPRINT_VERSION: &str = "codex-persisted-fingerprint:v1";

pub fn derive_codex_identity_fingerprint(
    account_id: Option<&str>,
    account_user_id: Option<&str>,
    user_id: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    let account = normalized_codex_identity_value(account_id);
    let member = normalized_codex_identity_value(account_user_id)
        .or_else(|| normalized_codex_identity_value(user_id))
        .or_else(|| normalized_codex_identity_value(email))?;

    let mut digest = Sha256::new();
    digest.update(CODEX_IDENTITY_FINGERPRINT_VERSION.as_bytes());
    digest.update([0]);
    digest.update(account.as_deref().unwrap_or("").as_bytes());
    digest.update([0]);
    digest.update(member.as_bytes());
    let digest = digest.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    Some(format!("{CODEX_IDENTITY_FINGERPRINT_VERSION}:{encoded}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericProviderOAuthTemplate {
    pub provider_type: &'static str,
    pub display_name: &'static str,
    pub authorize_url: &'static str,
    pub token_url: &'static str,
    pub client_id: &'static str,
    pub client_id_env: Option<&'static str>,
    pub client_secret_env: Option<&'static str>,
    pub scopes: &'static [&'static str],
    pub redirect_uri: &'static str,
    pub use_pkce: bool,
    pub uses_json_payload: bool,
    pub include_scope_in_token_request: bool,
}

pub const GENERIC_PROVIDER_OAUTH_TEMPLATES: &[GenericProviderOAuthTemplate] = &[
    GenericProviderOAuthTemplate {
        provider_type: CLAUDE_CODE_PROVIDER_TYPE,
        display_name: "ClaudeCode",
        authorize_url: CLAUDE_CODE_AUTHORIZE_URL,
        token_url: CLAUDE_CODE_TOKEN_URL,
        client_id: CLAUDE_CODE_CLIENT_ID,
        client_id_env: None,
        client_secret_env: None,
        scopes: CLAUDE_CODE_OAUTH_SCOPES,
        redirect_uri: CLAUDE_CODE_REDIRECT_URI,
        use_pkce: true,
        uses_json_payload: true,
        include_scope_in_token_request: false,
    },
    GenericProviderOAuthTemplate {
        provider_type: "codex",
        display_name: "Codex",
        authorize_url: "https://auth.openai.com/oauth/authorize",
        token_url: "https://auth.openai.com/oauth/token",
        client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
        client_id_env: None,
        client_secret_env: None,
        scopes: &["openid", "email", "profile", "offline_access"],
        redirect_uri: "http://localhost:1455/auth/callback",
        use_pkce: true,
        uses_json_payload: false,
        include_scope_in_token_request: true,
    },
    GenericProviderOAuthTemplate {
        provider_type: "chatgpt_web",
        display_name: "ChatGPT Web",
        authorize_url: "https://auth.openai.com/oauth/authorize",
        token_url: "https://auth.openai.com/oauth/token",
        client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
        client_id_env: None,
        client_secret_env: None,
        scopes: &["openid", "email", "profile", "offline_access"],
        redirect_uri: "http://localhost:1455/auth/callback",
        use_pkce: true,
        uses_json_payload: false,
        include_scope_in_token_request: true,
    },
    GenericProviderOAuthTemplate {
        provider_type: "gemini_cli",
        display_name: "GeminiCli",
        authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        client_id: "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
        client_id_env: Some(GEMINI_CLI_OAUTH_CLIENT_ID_ENV),
        client_secret_env: Some(GEMINI_CLI_OAUTH_CLIENT_SECRET_ENV),
        scopes: &[
            "https://www.googleapis.com/auth/cloud-platform",
            "https://www.googleapis.com/auth/userinfo.email",
            "https://www.googleapis.com/auth/userinfo.profile",
        ],
        redirect_uri: "http://localhost:8085/oauth2callback",
        use_pkce: false,
        uses_json_payload: false,
        include_scope_in_token_request: true,
    },
    GenericProviderOAuthTemplate {
        provider_type: "antigravity",
        display_name: "Antigravity",
        authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
        token_url: "https://oauth2.googleapis.com/token",
        client_id: "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
        client_id_env: Some(ANTIGRAVITY_OAUTH_CLIENT_ID_ENV),
        client_secret_env: Some(ANTIGRAVITY_OAUTH_CLIENT_SECRET_ENV),
        scopes: &[
            "https://www.googleapis.com/auth/cloud-platform",
            "https://www.googleapis.com/auth/userinfo.email",
            "https://www.googleapis.com/auth/userinfo.profile",
            "https://www.googleapis.com/auth/cclog",
            "https://www.googleapis.com/auth/experimentsandconfigs",
        ],
        redirect_uri: "http://localhost:51121/oauth2callback",
        use_pkce: true,
        uses_json_payload: false,
        include_scope_in_token_request: true,
    },
];

#[derive(Clone)]
pub struct GenericProviderOAuthAdapter {
    template: GenericProviderOAuthTemplate,
    token_url_override: Option<String>,
    client_id_override: Option<String>,
    client_secret_override: Option<String>,
}

impl std::fmt::Debug for GenericProviderOAuthAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenericProviderOAuthAdapter")
            .field("provider_type", &self.template.provider_type)
            .field("has_token_url_override", &self.token_url_override.is_some())
            .field("client_id_env", &self.template.client_id_env)
            .field("client_secret_env", &self.template.client_secret_env)
            .finish_non_exhaustive()
    }
}

impl GenericProviderOAuthAdapter {
    pub fn new(template: GenericProviderOAuthTemplate) -> Self {
        Self {
            template,
            token_url_override: None,
            client_id_override: None,
            client_secret_override: None,
        }
    }

    pub fn for_provider_type(provider_type: &str) -> Option<Self> {
        template_for_provider_type(provider_type).map(Self::new)
    }

    pub fn with_token_url_override(mut self, token_url: impl Into<String>) -> Self {
        self.token_url_override = Some(token_url.into());
        self
    }

    pub fn with_token_url_for_tests(self, token_url: impl Into<String>) -> Self {
        self.with_token_url_override(token_url)
    }

    #[doc(hidden)]
    pub fn with_oauth_credentials_for_tests(
        mut self,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        self.client_id_override = Some(client_id.into());
        self.client_secret_override = Some(client_secret.into());
        self
    }

    #[cfg(test)]
    fn without_oauth_client_secret_for_tests(mut self) -> Self {
        self.client_secret_override = Some(String::new());
        self
    }

    fn token_url(&self) -> String {
        self.token_url_override
            .clone()
            .unwrap_or_else(|| self.template.token_url.to_string())
    }

    fn client_id(&self) -> String {
        if let Some(value) = self.client_id_override.clone().and_then(non_empty_owned) {
            return value;
        }

        self.template
            .client_id_env
            .and_then(non_empty_environment_value)
            .unwrap_or_else(|| self.template.client_id.to_string())
    }

    fn client_secret(&self) -> Result<Option<String>, OAuthError> {
        let Some(env_name) = self.template.client_secret_env else {
            return Ok(None);
        };

        if let Some(value) = self.client_secret_override.clone() {
            return required_client_secret(env_name, non_empty_owned(value)).map(Some);
        }

        let configured = non_empty_environment_value(env_name);
        self.resolve_client_secret(configured).map(Some)
    }

    fn resolve_client_secret(&self, configured: Option<String>) -> Result<String, OAuthError> {
        let configured = configured.or_else(|| {
            let default_template = template_for_provider_type(self.template.provider_type)?;
            if self.client_id() != default_template.client_id {
                return None;
            }
            match self.template.provider_type {
                "gemini_cli" => Some("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".to_string()),
                "antigravity" => Some("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".to_string()),
                _ => None,
            }
        });
        required_client_secret(
            self.template
                .client_secret_env
                .unwrap_or(ANTIGRAVITY_OAUTH_CLIENT_SECRET_ENV),
            configured,
        )
    }

    async fn exchange_grant(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        grant_type: &str,
        code_or_refresh_token: &str,
        state: Option<&str>,
        pkce_verifier: Option<&str>,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let client_id = self.client_id();
        let client_secret = self.client_secret()?;
        let scope = (!self.template.scopes.is_empty()).then(|| self.template.scopes.join(" "));
        let request_id = match grant_type {
            "authorization_code" => "provider-oauth:exchange-code".to_string(),
            "refresh_token" => "provider-oauth:refresh-token".to_string(),
            _ => format!(
                "provider-oauth:{}:{grant_type}",
                self.template.provider_type
            ),
        };
        let response = if self.template.uses_json_payload {
            let mut body = serde_json::Map::from_iter([
                (
                    "grant_type".to_string(),
                    Value::String(grant_type.to_string()),
                ),
                ("client_id".to_string(), Value::String(client_id.clone())),
            ]);
            if let Some(client_secret) = client_secret.as_ref() {
                body.insert(
                    "client_secret".to_string(),
                    Value::String(client_secret.clone()),
                );
            }
            if grant_type == "authorization_code" {
                body.insert(
                    "code".to_string(),
                    Value::String(code_or_refresh_token.to_string()),
                );
                body.insert(
                    "redirect_uri".to_string(),
                    Value::String(self.template.redirect_uri.to_string()),
                );
                if let Some(state) = state {
                    body.insert("state".to_string(), Value::String(state.to_string()));
                }
                if let Some(verifier) = pkce_verifier {
                    body.insert(
                        "code_verifier".to_string(),
                        Value::String(verifier.to_string()),
                    );
                }
            } else {
                body.insert(
                    "refresh_token".to_string(),
                    Value::String(code_or_refresh_token.to_string()),
                );
            }
            if self.template.include_scope_in_token_request {
                if let Some(scope) = scope.as_ref() {
                    body.insert("scope".to_string(), Value::String(scope.clone()));
                }
            }
            executor
                .execute(OAuthHttpRequest {
                    request_id: request_id.clone(),
                    method: reqwest::Method::POST,
                    url: self.token_url(),
                    headers: json_headers(self.template.provider_type),
                    content_type: Some("application/json".to_string()),
                    json_body: Some(Value::Object(body)),
                    body_bytes: None,
                    network: ctx.network.clone(),
                    transport_profile: None,
                })
                .await?
        } else {
            let form_body = {
                let mut form = form_urlencoded::Serializer::new(String::new());
                form.append_pair("grant_type", grant_type);
                form.append_pair("client_id", &client_id);
                if grant_type == "authorization_code" {
                    form.append_pair("redirect_uri", self.template.redirect_uri);
                    form.append_pair("code", code_or_refresh_token);
                    if let Some(verifier) = pkce_verifier {
                        form.append_pair("code_verifier", verifier);
                    }
                } else {
                    form.append_pair("refresh_token", code_or_refresh_token);
                }
                if self.template.include_scope_in_token_request {
                    if let Some(scope) = scope.as_ref() {
                        form.append_pair("scope", scope);
                    }
                }
                if let Some(client_secret) = client_secret.as_deref() {
                    form.append_pair("client_secret", client_secret);
                }
                form.finish().into_bytes()
            };
            executor
                .execute(OAuthHttpRequest {
                    request_id,
                    method: reqwest::Method::POST,
                    url: self.token_url(),
                    headers: form_headers(),
                    content_type: Some("application/x-www-form-urlencoded".to_string()),
                    json_body: None,
                    body_bytes: Some(form_body),
                    network: ctx.network.clone(),
                    transport_profile: None,
                })
                .await?
        };
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: truncate_body(&response.body_text),
            });
        }
        let payload = response
            .json_body
            .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
            .ok_or_else(|| OAuthError::invalid_response("token response is not json"))?;
        self.token_set_from_payload(payload)
    }

    fn token_set_from_payload(&self, payload: Value) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let token_set = OAuthTokenSet::from_token_payload(payload.clone())
            .ok_or_else(|| OAuthError::invalid_response("token response missing access_token"))?;
        let mut auth_config = serde_json::Map::new();
        auth_config.insert(
            "provider_type".to_string(),
            json!(self.template.provider_type),
        );
        auth_config.insert("updated_at".to_string(), json!(current_unix_secs()));
        if let Some(token_type) = token_set.token_type.as_ref() {
            auth_config.insert("token_type".to_string(), json!(token_type));
        }
        if let Some(refresh_token) = token_set.refresh_token.as_ref() {
            auth_config.insert("refresh_token".to_string(), json!(refresh_token));
        }
        if let Some(expires_at) = token_set.expires_at_unix_secs {
            auth_config.insert("expires_at".to_string(), json!(expires_at));
        }
        if let Some(scope) = token_set.scope.as_ref() {
            auth_config.insert("scope".to_string(), json!(scope));
        }
        enrich_generic_identity(self.template.provider_type, &mut auth_config, &payload);
        ensure_codex_identity_fingerprint(self.template.provider_type, &mut auth_config);
        Ok(ProviderOAuthTokenSet {
            token_set,
            auth_config: Value::Object(auth_config),
        })
    }
}

#[async_trait]
impl ProviderOAuthAdapter for GenericProviderOAuthAdapter {
    fn provider_type(&self) -> &'static str {
        self.template.provider_type
    }

    fn capabilities(&self) -> ProviderOAuthCapabilities {
        ProviderOAuthCapabilities::GENERIC_AUTH_CODE
    }

    fn build_authorize_url(
        &self,
        _ctx: &ProviderOAuthTransportContext,
        state: &str,
        code_challenge: Option<&str>,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        self.client_secret()?;
        let client_id = self.client_id();
        let mut url = url::Url::parse(self.template.authorize_url)
            .map_err(|_| OAuthError::invalid_request("authorize_url must be absolute"))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", &client_id);
            query.append_pair("redirect_uri", self.template.redirect_uri);
            query.append_pair("state", state);
            if !self.template.scopes.is_empty() {
                query.append_pair("scope", &self.template.scopes.join(" "));
            }
            if let Some(challenge) = code_challenge {
                query.append_pair("code_challenge", challenge);
                query.append_pair("code_challenge_method", "S256");
            }
        }
        Ok(OAuthAuthorizeResponse {
            authorize_url: url.to_string(),
            state: state.to_string(),
            code_challenge: code_challenge.map(ToOwned::to_owned),
        })
    }

    async fn exchange_code(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        code: &str,
        state: &str,
        pkce_verifier: Option<&str>,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        self.exchange_grant(
            executor,
            ctx,
            "authorization_code",
            code,
            Some(state),
            pkce_verifier,
        )
        .await
    }

    async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let refresh_token = input
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| OAuthError::invalid_request("refresh_token is required"))?;
        self.exchange_grant(executor, ctx, "refresh_token", refresh_token, None, None)
            .await
    }

    async fn refresh(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let refresh_token = ["refresh_token", "refreshToken"]
            .iter()
            .find_map(|field| account.auth_config.get(*field).and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| OAuthError::invalid_request("auth_config missing refresh_token"))?;
        let mut refreshed = self
            .exchange_grant(executor, ctx, "refresh_token", refresh_token, None, None)
            .await?;
        let existing_codex_identity_fingerprint = self
            .template
            .provider_type
            .eq_ignore_ascii_case("codex")
            .then(|| {
                codex_identity_fingerprint_value(&account.auth_config).or_else(|| {
                    derive_codex_identity_fingerprint_from_auth_config(&account.auth_config)
                })
            })
            .flatten();

        // Refresh responses often omit stable account metadata, and some providers
        // do not rotate refresh_token on every refresh. Preserve the stored config
        // as the base while letting the fresh token payload win.
        if let Some(existing) = account.auth_config.as_object() {
            let mut merged = existing.clone();
            if let Some(updated) = refreshed.auth_config.as_object() {
                for (key, value) in updated {
                    merged.insert(key.clone(), value.clone());
                }
            }
            if refreshed.token_set.refresh_token.is_none() {
                refreshed.token_set.refresh_token = Some(refresh_token.to_string());
                merged.insert("refresh_token".to_string(), json!(refresh_token));
            }
            if let Some(fingerprint) = existing_codex_identity_fingerprint {
                merged.insert(
                    CODEX_IDENTITY_FINGERPRINT_FIELD.to_string(),
                    Value::String(fingerprint),
                );
            }
            ensure_codex_identity_fingerprint(self.template.provider_type, &mut merged);
            refreshed.auth_config = Value::Object(merged);
        }
        Ok(refreshed)
    }

    fn resolve_request_auth(
        &self,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthRequestAuth, OAuthError> {
        Ok(account.request_bearer_auth())
    }

    fn account_fingerprint(&self, account: &ProviderOAuthAccount) -> Option<String> {
        if self.template.provider_type.eq_ignore_ascii_case("codex") {
            return codex_identity_fingerprint_value(&account.auth_config).or_else(|| {
                derive_codex_identity_fingerprint_from_auth_config(&account.auth_config)
            });
        }
        let refresh_token = account
            .auth_config
            .get("refresh_token")
            .and_then(Value::as_str)
            .or(Some(account.access_token.as_str()))?;
        Some(secret_fingerprint(refresh_token))
    }
}

fn non_empty_environment_value(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(non_empty_owned)
}

fn non_empty_owned(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn required_client_secret(
    env_name: &'static str,
    configured: Option<String>,
) -> Result<String, OAuthError> {
    configured.ok_or_else(|| {
        OAuthError::invalid_request(format!(
            "{env_name} must be configured for this OAuth provider"
        ))
    })
}

pub fn template_for_provider_type(provider_type: &str) -> Option<GenericProviderOAuthTemplate> {
    let normalized = provider_type.trim();
    GENERIC_PROVIDER_OAUTH_TEMPLATES
        .iter()
        .find(|template| normalized.eq_ignore_ascii_case(template.provider_type))
        .copied()
}

fn form_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ),
        ("accept".to_string(), "application/json".to_string()),
    ])
}

fn json_headers(provider_type: &str) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::from([
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
    ]);
    if provider_type.eq_ignore_ascii_case(CLAUDE_CODE_PROVIDER_TYPE) {
        headers.insert(
            "accept".to_string(),
            "application/json, text/plain, */*".to_string(),
        );
        headers.insert("user-agent".to_string(), "axios/1.13.6".to_string());
    }
    headers
}

fn truncate_body(body: &str) -> String {
    redacted_oauth_error_body_excerpt(body)
}

fn secret_fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut fingerprint = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(&mut fingerprint, "{byte:02x}");
    }
    fingerprint
}

fn codex_identity_fingerprint_value(auth_config: &Value) -> Option<String> {
    [
        CODEX_IDENTITY_FINGERPRINT_FIELD,
        "codex-identity-fingerprint",
        "codexIdentityFingerprint",
    ]
    .iter()
    .find_map(|field| auth_config.get(*field).and_then(Value::as_str))
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(ToOwned::to_owned)
}

fn codex_identity_claim(auth_config: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| auth_config.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn normalized_codex_identity_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn derive_codex_identity_fingerprint_from_auth_config(auth_config: &Value) -> Option<String> {
    let account = codex_identity_claim(
        auth_config,
        &[
            "account_id",
            "accountId",
            "chatgpt_account_id",
            "chatgptAccountId",
        ],
    );
    let account_user = codex_identity_claim(
        auth_config,
        &[
            "account_user_id",
            "accountUserId",
            "chatgpt_account_user_id",
            "chatgptAccountUserId",
        ],
    );
    let user = codex_identity_claim(
        auth_config,
        &["user_id", "userId", "chatgpt_user_id", "chatgptUserId"],
    );
    let email = codex_identity_claim(
        auth_config,
        &["email", "email_address", "emailAddress", "outlook_email"],
    );

    derive_codex_identity_fingerprint(
        account.as_deref(),
        account_user.as_deref(),
        user.as_deref(),
        email.as_deref(),
    )
}

fn ensure_codex_identity_fingerprint(
    provider_type: &str,
    auth_config: &mut serde_json::Map<String, Value>,
) {
    if !provider_type.eq_ignore_ascii_case("codex") {
        return;
    }
    let auth_config_value = Value::Object(auth_config.clone());
    let fingerprint = codex_identity_fingerprint_value(&auth_config_value)
        .or_else(|| derive_codex_identity_fingerprint_from_auth_config(&auth_config_value));
    if let Some(fingerprint) = fingerprint {
        auth_config.insert(
            CODEX_IDENTITY_FINGERPRINT_FIELD.to_string(),
            Value::String(fingerprint),
        );
    }
}

fn enrich_generic_identity(
    provider_type: &str,
    auth_config: &mut serde_json::Map<String, Value>,
    token_payload: &Value,
) {
    if let Some(object) = token_payload.as_object() {
        for field in [
            "email",
            "email_address",
            "emailAddress",
            "outlook_email",
            "account_id",
            "accountId",
            "account_user_id",
            "accountUserId",
            "plan_type",
            "user_id",
            "userId",
            "account_name",
            "is_fedramp",
            CODEX_IDENTITY_FINGERPRINT_FIELD,
            "codex-identity-fingerprint",
            "codexIdentityFingerprint",
        ] {
            if !auth_config.contains_key(field) {
                if let Some(value) = object.get(field).cloned() {
                    auth_config.insert(field.to_string(), value);
                }
            }
        }
    }
    if provider_type.eq_ignore_ascii_case(CLAUDE_CODE_PROVIDER_TYPE) {
        if let Some(organization_uuid) = token_payload
            .get("organization")
            .and_then(Value::as_object)
            .and_then(|value| value.get("uuid"))
            .cloned()
        {
            auth_config
                .entry("org_uuid".to_string())
                .or_insert(organization_uuid);
        }
        if let Some(account) = token_payload.get("account").and_then(Value::as_object) {
            if let Some(account_uuid) = account.get("uuid").cloned() {
                auth_config
                    .entry("account_uuid".to_string())
                    .or_insert(account_uuid);
            }
            if let Some(email) = account.get("email_address").cloned() {
                auth_config
                    .entry("email_address".to_string())
                    .or_insert_with(|| email.clone());
                auth_config.entry("email".to_string()).or_insert(email);
            }
        }
        return;
    }
    if !matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "codex" | "chatgpt_web"
    ) {
        return;
    }
    if let Some(access_token) = token_payload
        .get("access_token")
        .and_then(Value::as_str)
        .or_else(|| token_payload.get("id_token").and_then(Value::as_str))
    {
        if let Some(claims) = decode_jwt_claims(access_token) {
            for field in ["email", "sub"] {
                if let Some(value) = claims.get(field).cloned() {
                    let target = if field == "sub" { "user_id" } else { field };
                    auth_config.entry(target.to_string()).or_insert(value);
                }
            }
            if let Some(auth) = claims
                .get("https://api.openai.com/auth")
                .and_then(Value::as_object)
            {
                for (source, target) in [
                    ("chatgpt_account_id", "account_id"),
                    ("chatgpt_account_user_id", "account_user_id"),
                    ("chatgpt_plan_type", "plan_type"),
                    ("chatgpt_user_id", "user_id"),
                ] {
                    if let Some(value) = auth.get(source).cloned() {
                        auth_config.entry(target.to_string()).or_insert(value);
                    }
                }
                if let Some(value) = auth.get("organizations").cloned() {
                    auth_config
                        .entry("organizations".to_string())
                        .or_insert(value);
                }
                if let Some(value) = auth.get("chatgpt_account_is_fedramp").cloned() {
                    auth_config.entry("is_fedramp".to_string()).or_insert(value);
                }
            }
            if let Some(profile) = claims
                .get("https://api.openai.com/profile")
                .and_then(Value::as_object)
            {
                for field in ["email", "email_address", "emailAddress", "outlook_email"] {
                    if let Some(value) = profile.get(field).cloned() {
                        auth_config.entry("email".to_string()).or_insert(value);
                        break;
                    }
                }
            }
        }
    }
}

pub(super) fn provider_account_state_from_metadata(
    metadata_key: &str,
    account: &ProviderOAuthAccount,
) -> ProviderOAuthProbeResult {
    let metadata = account
        .identity
        .get(metadata_key)
        .cloned()
        .or_else(|| account.auth_config.get(metadata_key).cloned());
    let email = string_field(&account.auth_config, "email")
        .or_else(|| account.identity.get("email").and_then(value_to_string))
        .or_else(|| {
            metadata
                .as_ref()
                .and_then(|value| string_field(value, "email"))
        });
    let invalid_reason = string_field(&account.auth_config, "oauth_invalid_reason")
        .or_else(|| string_field(&account.auth_config, "invalid_reason"))
        .or_else(|| metadata.as_ref().and_then(metadata_invalid_reason));
    let raw = json!({
        "auth_config": account.auth_config,
        "identity": account.identity,
    });
    ProviderOAuthProbeResult {
        state: ProviderOAuthAccountState {
            is_valid: !account.access_token.trim().is_empty() && invalid_reason.is_none(),
            email,
            quota: metadata,
            invalid_reason,
            raw: Some(raw),
        },
    }
}

fn metadata_invalid_reason(value: &Value) -> Option<String> {
    if value
        .get("is_forbidden")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return string_field(value, "forbidden_reason")
            .or_else(|| string_field(value, "message"))
            .or_else(|| Some("account_forbidden".to_string()));
    }
    if value
        .get("account_disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return string_field(value, "message")
            .or_else(|| string_field(value, "reason"))
            .or_else(|| Some("account_disabled".to_string()));
    }
    string_field(value, "invalid_reason").or_else(|| string_field(value, "reason"))
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(value_to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn decode_jwt_claims(token: &str) -> Option<serde_json::Map<String, Value>> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    const MAX_UNVERIFIED_JWT_CLAIMS_BYTES: usize = 64 * 1024;

    let payload = token.split('.').nth(1)?;
    let max_encoded_len = MAX_UNVERIFIED_JWT_CLAIMS_BYTES
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or(usize::MAX)
        .saturating_mul(4);
    if payload.len() > max_encoded_len {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    if bytes.len() > MAX_UNVERIFIED_JWT_CLAIMS_BYTES {
        return None;
    }
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::{
        decode_jwt_claims, derive_codex_identity_fingerprint, enrich_generic_identity,
        template_for_provider_type, GenericProviderOAuthAdapter,
        ANTIGRAVITY_OAUTH_CLIENT_SECRET_ENV, CODEX_IDENTITY_FINGERPRINT_FIELD,
        GEMINI_CLI_OAUTH_CLIENT_SECRET_ENV,
    };
    use crate::core::OAuthError;
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::ProviderOAuthAdapter;
    use crate::provider::{ProviderOAuthAccount, ProviderOAuthTransportContext};
    use async_trait::async_trait;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn resolves_generic_provider_templates() {
        assert!(template_for_provider_type("codex").is_some());
        assert!(template_for_provider_type("claude_code").is_some());
        assert!(template_for_provider_type("kiro").is_none());
    }

    #[test]
    fn generic_adapter_exposes_provider_type() {
        let adapter = GenericProviderOAuthAdapter::for_provider_type("codex")
            .expect("codex template should exist");
        assert_eq!(adapter.provider_type(), "codex");
        assert!(adapter.capabilities().supports_refresh_token_import);
    }

    fn encoded_jwt(claims: &Value) -> String {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims should encode"))
        )
    }

    #[test]
    fn google_oauth_templates_reference_external_client_secrets() {
        let gemini = template_for_provider_type("gemini_cli").expect("gemini template");
        let antigravity = template_for_provider_type("antigravity").expect("antigravity template");

        assert_eq!(
            gemini.client_secret_env,
            Some(GEMINI_CLI_OAUTH_CLIENT_SECRET_ENV)
        );
        assert_eq!(
            antigravity.client_secret_env,
            Some(ANTIGRAVITY_OAUTH_CLIENT_SECRET_ENV)
        );
    }

    #[tokio::test]
    async fn google_default_credentials_support_authorization_and_refresh() {
        for provider_type in ["gemini_cli", "antigravity"] {
            let mut template = template_for_provider_type(provider_type).expect("template");
            template.client_id_env = None;
            template.client_secret_env = Some("AETHER_TEST_UNUSED_ANTIGRAVITY_SECRET");
            let adapter = GenericProviderOAuthAdapter::new(template);
            let ctx = transport_context(provider_type);
            adapter
                .build_authorize_url(&ctx, "state", None)
                .expect("authorize");
            let seen_request = Arc::new(Mutex::new(None));
            let executor = StaticExecutor {
                seen_request: Arc::clone(&seen_request),
                response_payload: json!({"access_token": "new-token", "expires_in": 3600}),
            };
            adapter
                .refresh(&executor, &ctx, &oauth_account(provider_type))
                .await
                .expect("refresh");
            let seen = seen_request.lock().expect("lock").clone().expect("request");
            let body = seen.body_bytes.expect("body");
            let fields = url::form_urlencoded::parse(&body)
                .into_owned()
                .collect::<BTreeMap<_, _>>();
            assert_eq!(fields["client_id"], template.client_id);
            assert_eq!(
                fields["client_secret"],
                adapter.resolve_client_secret(None).expect("default")
            );
            assert_eq!(fields["refresh_token"], "old-refresh-token");
            assert!(!format!("{adapter:?}").contains(&fields["client_secret"]));
        }
    }

    #[test]
    fn google_custom_client_requires_its_own_secret() {
        for provider_type in ["gemini_cli", "antigravity"] {
            let adapter = GenericProviderOAuthAdapter::for_provider_type(provider_type)
                .expect("adapter")
                .with_oauth_credentials_for_tests("custom-client", "custom-secret");
            assert!(adapter.resolve_client_secret(None).is_err());
            assert_eq!(
                adapter
                    .resolve_client_secret(Some("custom-secret".to_string()))
                    .expect("configured secret"),
                "custom-secret"
            );
            assert_eq!(
                adapter.client_secret().expect("override"),
                Some("custom-secret".to_string())
            );
        }
    }

    #[test]
    fn google_configured_secret_overrides_the_native_app_default() {
        for provider_type in ["gemini_cli", "antigravity"] {
            let adapter =
                GenericProviderOAuthAdapter::for_provider_type(provider_type).expect("adapter");
            assert_eq!(
                adapter
                    .resolve_client_secret(Some("configured-secret".to_string()))
                    .unwrap(),
                "configured-secret"
            );
            let mut template = template_for_provider_type(provider_type).expect("template");
            template.client_id = "custom-template-client";
            template.client_id_env = None;
            assert!(GenericProviderOAuthAdapter::new(template)
                .resolve_client_secret(None)
                .is_err());
        }
    }

    #[test]
    fn generic_adapter_debug_redacts_oauth_credentials() {
        let adapter = GenericProviderOAuthAdapter::for_provider_type("gemini_cli")
            .expect("gemini adapter")
            .with_token_url_override("https://token.example.test/private-path")
            .with_oauth_credentials_for_tests("private-client-id", "private-client-secret");

        let debug = format!("{adapter:?}");

        assert!(!debug.contains("private-client-id"));
        assert!(!debug.contains("private-client-secret"));
        assert!(!debug.contains("private-path"));
        assert!(debug.contains(GEMINI_CLI_OAUTH_CLIENT_SECRET_ENV));
    }

    #[test]
    fn codex_identity_extracts_fedramp_workspace_claim() {
        let claims = json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-fedramp",
                "chatgpt_account_is_fedramp": true
            }
        });
        let token = encoded_jwt(&claims);
        let mut auth_config = serde_json::Map::new();

        enrich_generic_identity("codex", &mut auth_config, &json!({"access_token": token}));

        assert_eq!(auth_config.get("account_id"), Some(&json!("acct-fedramp")));
        assert_eq!(auth_config.get("is_fedramp"), Some(&json!(true)));
    }

    #[test]
    fn generic_identity_rejects_oversized_jwt_claims_before_decode() {
        const MAX_UNVERIFIED_JWT_CLAIMS_BYTES: usize = 64 * 1024;
        let max_encoded_len = MAX_UNVERIFIED_JWT_CLAIMS_BYTES
            .saturating_add(2)
            .checked_div(3)
            .unwrap()
            .saturating_mul(4);
        let token = format!("header.{}.signature", "A".repeat(max_encoded_len + 1));

        assert_eq!(decode_jwt_claims(&token), None);
    }

    #[test]
    fn codex_persisted_fingerprint_is_member_scoped_and_token_independent() {
        let adapter = GenericProviderOAuthAdapter::for_provider_type("codex")
            .expect("codex adapter should exist");
        let claims = json!({
            "sub": "global-user-1",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "workspace-1",
                "chatgpt_account_user_id": "member-1"
            }
        });
        let rotated_claims = json!({
            "sub": "global-user-1",
            "iat": 12345,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "WORKSPACE-1",
                "chatgpt_account_user_id": "MEMBER-1"
            }
        });
        let other_member_claims = json!({
            "sub": "global-user-2",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "workspace-1",
                "chatgpt_account_user_id": "member-2"
            }
        });

        let first = adapter
            .token_set_from_payload(json!({"access_token": encoded_jwt(&claims)}))
            .expect("first token should parse");
        let rotated = adapter
            .token_set_from_payload(json!({"access_token": encoded_jwt(&rotated_claims)}))
            .expect("rotated token should parse");
        let other_member = adapter
            .token_set_from_payload(json!({"access_token": encoded_jwt(&other_member_claims)}))
            .expect("other member token should parse");

        let first_fingerprint = first.auth_config[CODEX_IDENTITY_FINGERPRINT_FIELD]
            .as_str()
            .expect("persisted fingerprint")
            .to_string();
        assert!(first_fingerprint.starts_with("codex-persisted-fingerprint:v1:"));
        assert_eq!(
            rotated.auth_config[CODEX_IDENTITY_FINGERPRINT_FIELD].as_str(),
            Some(first_fingerprint.as_str())
        );
        assert_ne!(
            other_member.auth_config[CODEX_IDENTITY_FINGERPRINT_FIELD].as_str(),
            Some(first_fingerprint.as_str())
        );

        let account = ProviderOAuthAccount {
            provider_type: "codex".to_string(),
            access_token: "unrelated-rotated-token".to_string(),
            auth_config: first.auth_config,
            expires_at_unix_secs: None,
            identity: BTreeMap::new(),
        };
        assert_eq!(
            adapter.account_fingerprint(&account).as_deref(),
            Some(first_fingerprint.as_str())
        );
    }

    #[derive(Debug, Clone)]
    struct StaticExecutor {
        seen_request: Arc<Mutex<Option<OAuthHttpRequest>>>,
        response_payload: Value,
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
                body_text: self.response_payload.to_string(),
                json_body: None,
            })
        }
    }

    fn transport_context(provider_type: &str) -> ProviderOAuthTransportContext {
        ProviderOAuthTransportContext {
            provider_id: "provider-1".to_string(),
            provider_type: provider_type.to_string(),
            endpoint_id: None,
            key_id: Some("key-1".to_string()),
            auth_type: Some("oauth".to_string()),
            decrypted_api_key: None,
            decrypted_auth_config: None,
            provider_config: None,
            endpoint_config: None,
            key_config: None,
            network: crate::network::OAuthNetworkContext::provider_operation(None),
        }
    }

    fn oauth_account(provider_type: &str) -> ProviderOAuthAccount {
        ProviderOAuthAccount {
            provider_type: provider_type.to_string(),
            access_token: "old-access-token".to_string(),
            auth_config: json!({
                "provider_type": provider_type,
                "refresh_token": "old-refresh-token",
                "updated_at": 1
            }),
            expires_at_unix_secs: Some(1),
            identity: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn google_oauth_fails_closed_before_network_without_client_secret() {
        let seen_request = Arc::new(Mutex::new(None));
        let executor = StaticExecutor {
            seen_request: Arc::clone(&seen_request),
            response_payload: json!({}),
        };
        let adapter = GenericProviderOAuthAdapter::for_provider_type("gemini_cli")
            .expect("gemini adapter")
            .without_oauth_client_secret_for_tests();
        let ctx = transport_context("gemini_cli");

        let authorize_error = adapter
            .build_authorize_url(&ctx, "state", None)
            .expect_err("authorization must fail without the configured secret");
        assert!(matches!(authorize_error, OAuthError::InvalidRequest(_)));

        let refresh_error = adapter
            .refresh(&executor, &ctx, &oauth_account("gemini_cli"))
            .await
            .expect_err("refresh must fail without the configured secret");
        assert!(matches!(refresh_error, OAuthError::InvalidRequest(_)));
        assert!(
            seen_request.lock().expect("mutex should lock").is_none(),
            "credential validation must happen before the HTTP executor runs"
        );
    }

    #[tokio::test]
    async fn google_oauth_injected_credentials_are_sent_in_token_form() {
        let seen_request = Arc::new(Mutex::new(None));
        let executor = StaticExecutor {
            seen_request: Arc::clone(&seen_request),
            response_payload: json!({
                "access_token": "new-access-token",
                "expires_in": 3600,
            }),
        };
        let adapter = GenericProviderOAuthAdapter::for_provider_type("gemini_cli")
            .expect("gemini adapter")
            .with_oauth_credentials_for_tests("test-client-id", "test-client-secret");
        let ctx = transport_context("gemini_cli");

        adapter
            .refresh(&executor, &ctx, &oauth_account("gemini_cli"))
            .await
            .expect("refresh should succeed");

        let seen = seen_request
            .lock()
            .expect("mutex should lock")
            .clone()
            .expect("request should be captured");
        let form = String::from_utf8(seen.body_bytes.expect("form body should exist"))
            .expect("form body should be utf8");
        let fields = url::form_urlencoded::parse(form.as_bytes())
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            fields.get("client_id").map(String::as_str),
            Some("test-client-id")
        );
        assert_eq!(
            fields.get("client_secret").map(String::as_str),
            Some("test-client-secret")
        );
        assert_eq!(
            fields.get("refresh_token").map(String::as_str),
            Some("old-refresh-token")
        );
    }

    #[tokio::test]
    async fn refresh_preserves_existing_metadata_when_refresh_token_is_not_rotated() {
        let seen_request = Arc::new(Mutex::new(None));
        let refreshed_token = encoded_jwt(&json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-123",
                "chatgpt_account_user_id": "replacement-member"
            }
        }));
        let executor = StaticExecutor {
            seen_request: Arc::clone(&seen_request),
            response_payload: json!({
                "access_token": refreshed_token,
                "expires_in": 3600
            }),
        };
        let adapter = GenericProviderOAuthAdapter::for_provider_type("codex")
            .expect("codex adapter should exist")
            .with_token_url_override("https://auth.example.test/token");
        let expected_legacy_fingerprint = derive_codex_identity_fingerprint(
            Some("acct-123"),
            Some("original-member"),
            None,
            Some("alice@example.com"),
        )
        .expect("legacy identity should produce a fingerprint");
        let ctx = ProviderOAuthTransportContext {
            provider_id: "provider-1".to_string(),
            provider_type: "codex".to_string(),
            endpoint_id: None,
            key_id: Some("key-1".to_string()),
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
            access_token: "old-access-token".to_string(),
            auth_config: json!({
                "provider_type": "codex",
                "refresh_token": "old-refresh-token",
                "email": "alice@example.com",
                "account_id": "acct-123",
                "account_user_id": "original-member",
                "updated_at": 1
            }),
            expires_at_unix_secs: Some(1),
            identity: BTreeMap::new(),
        };

        let refreshed = adapter
            .refresh(&executor, &ctx, &account)
            .await
            .expect("refresh should succeed");

        assert_eq!(refreshed.token_set.access_token, refreshed_token);
        assert_eq!(
            refreshed.token_set.refresh_token.as_deref(),
            Some("old-refresh-token")
        );
        assert_eq!(refreshed.auth_config["email"], "alice@example.com");
        assert_eq!(refreshed.auth_config["account_id"], "acct-123");
        assert_eq!(refreshed.auth_config["refresh_token"], "old-refresh-token");
        assert_eq!(
            refreshed.auth_config[CODEX_IDENTITY_FINGERPRINT_FIELD].as_str(),
            Some(expected_legacy_fingerprint.as_str())
        );

        let seen = seen_request
            .lock()
            .expect("mutex should lock")
            .clone()
            .expect("request should be captured");
        let form = String::from_utf8(seen.body_bytes.expect("form body should exist"))
            .expect("form body should be utf8");
        assert!(form.contains("grant_type=refresh_token"));
        assert!(form.contains("refresh_token=old-refresh-token"));
    }
}
