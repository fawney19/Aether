use crate::core::{current_unix_secs, OAuthAuthorizeResponse, OAuthError, OAuthTokenSet};
use crate::network::{OAuthHttpExecutor, OAuthHttpRequest};
use crate::provider::{
    ProviderOAuthAccount, ProviderOAuthAccountState, ProviderOAuthAdapter,
    ProviderOAuthCapabilities, ProviderOAuthImportInput, ProviderOAuthProbeResult,
    ProviderOAuthRequestAuth, ProviderOAuthTokenSet, ProviderOAuthTransportContext,
};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const WINDSURF_PROVIDER_TYPE: &str = "windsurf";
pub const WINDSURF_SIGNIN_URL: &str = "https://windsurf.com/windsurf/signin";
pub const WINDSURF_CLIENT_ID: &str = "3GUryQ7ldAeKEuD2obYnppsnmj58eP5u";
pub const WINDSURF_SHOW_AUTH_TOKEN_REDIRECT: &str = "show-auth-token";
const AUTH1_PASSWORD_LOGIN_URL: &str = "https://windsurf.com/_devin-auth/password/login";
const WINDSURF_POST_AUTH_URL: &str =
    "https://windsurf.com/_backend/exa.seat_management_pb.SeatManagementService/WindsurfPostAuth";
const WINDSURF_POST_AUTH_LEGACY_URL: &str =
    "https://server.self-serve.windsurf.com/exa.seat_management_pb.SeatManagementService/WindsurfPostAuth";
const WINDSURF_REGISTER_USER_URL: &str =
    "https://register.windsurf.com/exa.seat_management_pb.SeatManagementService/RegisterUser";
const WINDSURF_REGISTER_USER_LEGACY_URL: &str = "https://api.codeium.com/register_user/";

#[derive(Debug, Clone, Default)]
pub struct WindsurfProviderOAuthAdapter;

impl WindsurfProviderOAuthAdapter {
    async fn import_raw_api_key(
        &self,
        input: &ProviderOAuthImportInput,
        api_key: &str,
        auth_method: &str,
        source: &str,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(OAuthError::invalid_request("windsurf api_key is required"));
        }

        let mut auth_config = Map::new();
        auth_config.insert("provider_type".to_string(), json!(WINDSURF_PROVIDER_TYPE));
        auth_config.insert("auth_method".to_string(), json!(auth_method));
        auth_config.insert("register_source".to_string(), json!(source));
        auth_config.insert("updated_at".to_string(), json!(current_unix_secs()));
        if let Some(name) = input.name.as_deref().and_then(non_empty_str) {
            auth_config.insert("name".to_string(), json!(name));
        }
        if let Some(raw) = input.raw_credentials.as_ref() {
            copy_optional_string(raw, &mut auth_config, "email", &["email"]);
            copy_optional_string(
                raw,
                &mut auth_config,
                "social_provider",
                &["social_provider", "socialProvider"],
            );
        }
        if auth_config.get("email").is_some() {
            auth_config.insert("email_verified".to_string(), json!(false));
        }
        insert_secret_fingerprint(&mut auth_config, "credential_fingerprint", api_key);

        Ok(provider_token_set(
            api_key,
            Value::Object(auth_config),
            None,
        ))
    }

    async fn register_with_token(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: &ProviderOAuthImportInput,
        token: &str,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(OAuthError::invalid_request("windsurf token is required"));
        }

        let mut errors = Vec::new();
        for (url, source) in [
            (WINDSURF_REGISTER_USER_URL, "new"),
            (WINDSURF_REGISTER_USER_LEGACY_URL, "legacy"),
        ] {
            let response = executor
                .execute(OAuthHttpRequest {
                    request_id: format!("provider-oauth:windsurf-register:{source}"),
                    method: reqwest::Method::POST,
                    url: url.to_string(),
                    headers: json_connect_headers(),
                    content_type: Some("application/json".to_string()),
                    json_body: Some(json!({ "firebase_id_token": token })),
                    body_bytes: None,
                    network: ctx.network.clone(),
                })
                .await;
            match response {
                Ok(response) if (200..300).contains(&response.status_code) => {
                    let payload = response
                        .json_body
                        .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
                        .ok_or_else(|| {
                            OAuthError::invalid_response("RegisterUser response is not json")
                        })?;
                    if let Some(api_key) = string_any(&payload, &["api_key", "apiKey"]) {
                        let mut auth_config = Map::new();
                        auth_config
                            .insert("provider_type".to_string(), json!(WINDSURF_PROVIDER_TYPE));
                        auth_config.insert("auth_method".to_string(), json!("token"));
                        auth_config.insert("register_source".to_string(), json!(source));
                        insert_secret_fingerprint(&mut auth_config, "id_token_fingerprint", token);
                        insert_secret_fingerprint(
                            &mut auth_config,
                            "credential_fingerprint",
                            &api_key,
                        );
                        auth_config.insert("updated_at".to_string(), json!(current_unix_secs()));
                        copy_optional_string(&payload, &mut auth_config, "name", &["name"]);
                        let payload_email_verified = string_any(&payload, &["email"]).is_some();
                        copy_optional_string(&payload, &mut auth_config, "email", &["email"]);
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "account_id",
                            &["account_id", "accountId", "user_id", "userId"],
                        );
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "primary_org_id",
                            &[
                                "primary_org_id",
                                "primaryOrgId",
                                "organization_id",
                                "organizationId",
                            ],
                        );
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "api_server_url",
                            &["api_server_url", "apiServerUrl"],
                        );
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "plan_name",
                            &["plan_name", "planName", "plan"],
                        );
                        if let Some(name) = input.name.as_deref().and_then(non_empty_str) {
                            auth_config
                                .entry("name".to_string())
                                .or_insert_with(|| json!(name));
                        }
                        if let Some(raw) = input.raw_credentials.as_ref() {
                            copy_optional_string(raw, &mut auth_config, "email", &["email"]);
                            copy_optional_string(
                                raw,
                                &mut auth_config,
                                "social_provider",
                                &["social_provider", "socialProvider"],
                            );
                        }
                        if auth_config.get("email").is_some() {
                            auth_config.insert(
                                "email_verified".to_string(),
                                json!(payload_email_verified),
                            );
                        }
                        return Ok(provider_token_set(
                            &api_key,
                            Value::Object(auth_config),
                            None,
                        ));
                    }
                    errors.push(format!("{source}=missing api_key"));
                }
                Ok(response) => errors.push(format!(
                    "{source}=HTTP {} {}",
                    response.status_code,
                    truncate_body(&response.body_text)
                )),
                Err(error) => errors.push(format!("{source}={error}")),
            }
        }

        Err(OAuthError::invalid_response(format!(
            "RegisterUser failed: {}",
            errors.join(" | ")
        )))
    }

    async fn login_with_password(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: &ProviderOAuthImportInput,
        email: &str,
        password: &str,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let email = email.trim();
        let password = password.trim();
        if email.is_empty() || password.is_empty() {
            return Err(OAuthError::invalid_request(
                "windsurf email and password are required",
            ));
        }

        let login_response = executor
            .execute(OAuthHttpRequest {
                request_id: "provider-oauth:windsurf-auth1-login".to_string(),
                method: reqwest::Method::POST,
                url: AUTH1_PASSWORD_LOGIN_URL.to_string(),
                headers: windsurf_browser_json_headers(),
                content_type: Some("application/json".to_string()),
                json_body: Some(json!({ "email": email, "password": password })),
                body_bytes: None,
                network: ctx.network.clone(),
            })
            .await?;
        if !(200..300).contains(&login_response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: login_response.status_code,
                body_excerpt: truncate_body(&login_response.body_text),
            });
        }
        let login_payload = login_response
            .json_body
            .or_else(|| serde_json::from_str::<Value>(&login_response.body_text).ok())
            .ok_or_else(|| OAuthError::invalid_response("Auth1 response is not json"))?;
        let auth1_token = string_any(&login_payload, &["token", "access_token", "accessToken"])
            .ok_or_else(|| OAuthError::invalid_response("Auth1 response missing token"))?;

        let mut post_auth_errors = Vec::new();
        for (url, source) in [
            (WINDSURF_POST_AUTH_URL, "new"),
            (WINDSURF_POST_AUTH_LEGACY_URL, "legacy"),
        ] {
            let mut headers = proto_headers();
            headers.insert("x-devin-auth1-token".to_string(), auth1_token.clone());
            let response = executor
                .execute(OAuthHttpRequest {
                    request_id: format!("provider-oauth:windsurf-post-auth:{source}"),
                    method: reqwest::Method::POST,
                    url: url.to_string(),
                    headers,
                    content_type: Some("application/proto".to_string()),
                    json_body: None,
                    body_bytes: Some(Vec::new()),
                    network: ctx.network.clone(),
                })
                .await;
            match response {
                Ok(response) if (200..300).contains(&response.status_code) => {
                    let payload = parse_windsurf_post_auth_payload(&response).ok_or_else(|| {
                        OAuthError::invalid_response(
                            "WindsurfPostAuth response missing sessionToken",
                        )
                    })?;
                    if let Some(session_token) = string_any(&payload, &["sessionToken"]) {
                        let mut auth_config = Map::new();
                        auth_config
                            .insert("provider_type".to_string(), json!(WINDSURF_PROVIDER_TYPE));
                        auth_config.insert("auth_method".to_string(), json!("email_password"));
                        auth_config.insert("register_source".to_string(), json!(source));
                        auth_config.insert("email".to_string(), json!(email));
                        auth_config.insert("email_verified".to_string(), json!(true));
                        auth_config.insert("updated_at".to_string(), json!(current_unix_secs()));
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "account_id",
                            &["accountId", "account_id"],
                        );
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "primary_org_id",
                            &["primaryOrgId", "primary_org_id"],
                        );
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "api_server_url",
                            &["apiServerUrl", "api_server_url"],
                        );
                        copy_optional_string(
                            &payload,
                            &mut auth_config,
                            "plan_name",
                            &["planName", "plan_name", "plan"],
                        );
                        if let Some(name) = input.name.as_deref().and_then(non_empty_str) {
                            auth_config.insert("name".to_string(), json!(name));
                        }
                        insert_secret_fingerprint(
                            &mut auth_config,
                            "credential_fingerprint",
                            &session_token,
                        );
                        return Ok(provider_token_set(
                            &session_token,
                            Value::Object(auth_config),
                            None,
                        ));
                    }
                    post_auth_errors.push(format!("{source}=missing sessionToken"));
                }
                Ok(response) => post_auth_errors.push(format!(
                    "{source}=HTTP {} {}",
                    response.status_code,
                    truncate_body(&response.body_text)
                )),
                Err(error) => post_auth_errors.push(format!("{source}={error}")),
            }
        }

        Err(OAuthError::invalid_response(format!(
            "WindsurfPostAuth failed: {}",
            post_auth_errors.join(" | ")
        )))
    }
}

#[async_trait]
impl ProviderOAuthAdapter for WindsurfProviderOAuthAdapter {
    fn provider_type(&self) -> &'static str {
        WINDSURF_PROVIDER_TYPE
    }

    fn capabilities(&self) -> ProviderOAuthCapabilities {
        ProviderOAuthCapabilities {
            supports_authorization_code: false,
            supports_refresh_token_import: true,
            supports_batch_import: true,
            supports_device_flow: true,
            supports_account_probe: true,
            rotates_refresh_token: false,
        }
    }

    fn build_authorize_url(
        &self,
        _ctx: &ProviderOAuthTransportContext,
        state: &str,
        _code_challenge: Option<&str>,
    ) -> Result<OAuthAuthorizeResponse, OAuthError> {
        let mut url = url::Url::parse(WINDSURF_SIGNIN_URL)
            .map_err(|_| OAuthError::invalid_response("invalid windsurf signin url"))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "token");
            query.append_pair("client_id", WINDSURF_CLIENT_ID);
            query.append_pair("redirect_uri", WINDSURF_SHOW_AUTH_TOKEN_REDIRECT);
            query.append_pair("state", state);
            query.append_pair("prompt", "login");
            query.append_pair("redirect_parameters_type", "query");
            query.append_pair("workflow", "");
        }
        Ok(OAuthAuthorizeResponse {
            authorize_url: url.to_string(),
            state: state.to_string(),
            code_challenge: None,
        })
    }

    async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let raw = input.raw_credentials.as_ref();
        if let Some(api_key) = raw.and_then(|value| string_any(value, &["api_key", "apiKey"])) {
            return self
                .import_raw_api_key(&input, &api_key, "api_key", "manual")
                .await;
        }
        if let Some(api_key) = input
            .refresh_token
            .as_deref()
            .and_then(|value| windsurf_raw_api_key(value).map(ToOwned::to_owned))
        {
            return self
                .import_raw_api_key(&input, &api_key, "api_key", "manual")
                .await;
        }
        if let Some(token) = raw.and_then(|value| {
            string_any(
                value,
                &[
                    "token",
                    "auth_token",
                    "authToken",
                    "access_token",
                    "accessToken",
                    "refresh_token",
                    "refreshToken",
                ],
            )
        }) {
            if windsurf_raw_api_key(&token).is_some() {
                return self
                    .import_raw_api_key(&input, &token, "api_key", "manual")
                    .await;
            }
            return self
                .register_with_token(executor, ctx, &input, &token)
                .await;
        }
        if let Some(token) = input.refresh_token.as_deref().and_then(non_empty_str) {
            return self.register_with_token(executor, ctx, &input, token).await;
        }
        if let (Some(email), Some(password)) = (
            raw.and_then(|value| string_any(value, &["email"])),
            raw.and_then(|value| string_any(value, &["password"])),
        ) {
            return self
                .login_with_password(executor, ctx, &input, &email, &password)
                .await;
        }

        Err(OAuthError::invalid_request(
            "windsurf credentials require api_key, token, or email/password",
        ))
    }

    async fn refresh(
        &self,
        _executor: &dyn OAuthHttpExecutor,
        _ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        Ok(provider_token_set(
            &account.access_token,
            account.auth_config.clone(),
            account.expires_at_unix_secs,
        ))
    }

    fn resolve_request_auth(
        &self,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthRequestAuth, OAuthError> {
        Ok(ProviderOAuthRequestAuth::Header {
            name: "authorization".to_string(),
            value: format!("Bearer {}", account.access_token.trim()),
        })
    }

    fn account_fingerprint(&self, account: &ProviderOAuthAccount) -> Option<String> {
        Some(secret_fingerprint(&account.access_token))
    }

    async fn probe_account_state(
        &self,
        _executor: &dyn OAuthHttpExecutor,
        _ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<Option<ProviderOAuthProbeResult>, OAuthError> {
        let metadata = account
            .identity
            .get(WINDSURF_PROVIDER_TYPE)
            .cloned()
            .or_else(|| account.auth_config.get(WINDSURF_PROVIDER_TYPE).cloned());
        let email = string_any(&account.auth_config, &["email"])
            .or_else(|| {
                account
                    .identity
                    .get("email")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .or_else(|| {
                metadata
                    .as_ref()
                    .and_then(|value| string_any(value, &["email"]))
            });
        let invalid_reason = string_any(
            &account.auth_config,
            &["oauth_invalid_reason", "invalid_reason"],
        )
        .or_else(|| {
            metadata
                .as_ref()
                .and_then(|value| string_any(value, &["last_error", "invalid_reason"]))
        });
        Ok(Some(ProviderOAuthProbeResult {
            state: ProviderOAuthAccountState {
                is_valid: !account.access_token.trim().is_empty() && invalid_reason.is_none(),
                email,
                quota: metadata,
                invalid_reason,
                raw: Some(json!({
                    "auth_config": account.auth_config,
                    "identity": account.identity,
                })),
            },
        }))
    }
}

fn provider_token_set(
    api_key: &str,
    auth_config: Value,
    expires_at_unix_secs: Option<u64>,
) -> ProviderOAuthTokenSet {
    ProviderOAuthTokenSet {
        token_set: OAuthTokenSet {
            access_token: api_key.trim().to_string(),
            refresh_token: None,
            token_type: Some("windsurf_api_key".to_string()),
            scope: None,
            expires_at_unix_secs,
            raw_payload: Some(json!({
                "access_token": api_key.trim(),
                "token_type": "windsurf_api_key",
            })),
        },
        auth_config,
    }
}

fn windsurf_raw_api_key(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.starts_with("devin-session-token$") || value.starts_with("sk-") {
        Some(value)
    } else {
        None
    }
}

fn json_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
        ("user-agent".to_string(), "windsurf/1.9600.41".to_string()),
    ])
}

fn windsurf_browser_json_headers() -> BTreeMap<String, String> {
    let mut headers = windsurf_browser_headers();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert(
        "accept".to_string(),
        "application/json, text/plain, */*".to_string(),
    );
    headers
}

fn windsurf_browser_headers() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "user-agent".to_string(),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_2_1) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36".to_string(),
        ),
        ("accept".to_string(), "application/json, text/plain, */*".to_string()),
        ("accept-language".to_string(), "en-US,en;q=0.9".to_string()),
        ("accept-encoding".to_string(), "identity".to_string()),
        ("origin".to_string(), "https://windsurf.com".to_string()),
        ("referer".to_string(), "https://windsurf.com/".to_string()),
        ("sec-ch-ua".to_string(), "\"Chromium\";v=\"134\", \"Google Chrome\";v=\"134\", \"Not.A/Brand\";v=\"99\"".to_string()),
        ("sec-ch-ua-mobile".to_string(), "?0".to_string()),
        ("sec-ch-ua-platform".to_string(), "\"macOS\"".to_string()),
        ("sec-fetch-dest".to_string(), "empty".to_string()),
        ("sec-fetch-mode".to_string(), "cors".to_string()),
        ("sec-fetch-site".to_string(), "cross-site".to_string()),
    ])
}

fn json_connect_headers() -> BTreeMap<String, String> {
    let mut headers = json_headers();
    headers.insert("connect-protocol-version".to_string(), "1".to_string());
    headers
}

fn proto_headers() -> BTreeMap<String, String> {
    let mut headers = windsurf_browser_headers();
    headers.insert("content-type".to_string(), "application/proto".to_string());
    headers.insert("content-length".to_string(), "0".to_string());
    headers.insert("connect-protocol-version".to_string(), "1".to_string());
    headers.insert(
        "referer".to_string(),
        "https://windsurf.com/account/login".to_string(),
    );
    headers
}

fn parse_windsurf_post_auth_payload(response: &crate::network::OAuthHttpResponse) -> Option<Value> {
    let payload = response
        .json_body
        .clone()
        .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok());
    if payload
        .as_ref()
        .and_then(|value| string_any(value, &["sessionToken"]))
        .is_some()
    {
        return payload;
    }
    let session_token = extract_windsurf_session_token_from_text(&response.body_text)?;
    let account_id = extract_windsurf_raw_match(&response.body_text, "account-");
    let primary_org_id = extract_windsurf_raw_match(&response.body_text, "org-");
    let mut payload = Map::new();
    payload.insert("sessionToken".to_string(), json!(session_token));
    if let Some(account_id) = account_id {
        payload.insert("accountId".to_string(), json!(account_id));
    }
    if let Some(primary_org_id) = primary_org_id {
        payload.insert("primaryOrgId".to_string(), json!(primary_org_id));
    }
    Some(Value::Object(payload))
}

fn extract_windsurf_session_token_from_text(value: &str) -> Option<String> {
    let start = value.find("devin-session-token$")?;
    let token = value[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '$' | '.' | '_' | '-'))
        .collect::<String>();
    (token.len() > "devin-session-token$".len()).then_some(token)
}

fn extract_windsurf_raw_match(value: &str, prefix: &str) -> Option<String> {
    let start = value.find(prefix)?;
    let matched = value[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit() || *ch == '-')
        .collect::<String>();
    (matched.len() > prefix.len()).then_some(matched)
}

fn copy_optional_string(
    value: &Value,
    target: &mut Map<String, Value>,
    key: &str,
    aliases: &[&str],
) {
    if let Some(text) = string_any(value, aliases) {
        target.entry(key.to_string()).or_insert_with(|| json!(text));
    }
}

fn string_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .and_then(non_empty_str)
            .map(ToOwned::to_owned)
    })
}

fn insert_secret_fingerprint(target: &mut Map<String, Value>, key: &str, secret: &str) {
    let secret = secret.trim();
    if !secret.is_empty() {
        target.insert(key.to_string(), json!(secret_fingerprint(secret)));
    }
}

fn non_empty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn truncate_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "-".to_string();
    }
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_sensitive_json(&mut value);
        return value.to_string().chars().take(500).collect();
    }
    if contains_sensitive_marker(body) {
        "[REDACTED upstream error body]".to_string()
    } else {
        body.chars().take(500).collect()
    }
}

fn redact_sensitive_json(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if is_sensitive_key(key) {
                    *value = json!("[REDACTED]");
                } else {
                    redact_sensitive_json(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_sensitive_json(item);
            }
        }
        Value::String(text) if looks_like_sensitive_secret(text) => {
            *text = "[REDACTED]".to_string();
        }
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.contains("token")
        || normalized.contains("apikey")
        || normalized.contains("password")
        || normalized.contains("authorization")
        || normalized.contains("secret")
}

fn looks_like_sensitive_secret(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("devin-session-token$")
        || value.starts_with("sk-")
        || (value.len() > 80 && value.split('.').count() == 3)
}

fn contains_sensitive_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "token",
        "api_key",
        "apikey",
        "password",
        "authorization",
        "sessiontoken",
        "firebase_id_token",
        "idtoken",
        "secret",
    ]
    .iter()
    .any(|marker| value.contains(marker))
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

#[cfg(test)]
mod tests {
    use super::{
        secret_fingerprint, truncate_body, WindsurfProviderOAuthAdapter, AUTH1_PASSWORD_LOGIN_URL,
        WINDSURF_POST_AUTH_URL, WINDSURF_REGISTER_USER_URL,
    };
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::{
        ProviderOAuthAdapter, ProviderOAuthImportInput, ProviderOAuthTransportContext,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingExecutor {
        requests: Arc<Mutex<Vec<OAuthHttpRequest>>>,
        raw_post_auth_body: Option<String>,
    }

    #[async_trait]
    impl OAuthHttpExecutor for RecordingExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            self.requests
                .lock()
                .expect("requests lock")
                .push(request.clone());
            if request.url == WINDSURF_REGISTER_USER_URL {
                return Ok(OAuthHttpResponse {
                    status_code: 200,
                    body_text: r#"{"apiKey":"sk-ws-01-registered","name":"Alice","email":"alice@example.com","accountId":"acct-1","primaryOrgId":"org-1","planName":"Pro","apiServerUrl":"https://server.codeium.com"}"#.to_string(),
                    json_body: Some(json!({
                        "apiKey": "sk-ws-01-registered",
                        "name": "Alice",
                        "email": "alice@example.com",
                        "accountId": "acct-1",
                        "primaryOrgId": "org-1",
                        "planName": "Pro",
                        "apiServerUrl": "https://server.codeium.com"
                    })),
                });
            }
            if request.url == AUTH1_PASSWORD_LOGIN_URL {
                return Ok(OAuthHttpResponse {
                    status_code: 200,
                    body_text: r#"{"token":"auth1-token"}"#.to_string(),
                    json_body: Some(json!({"token": "auth1-token"})),
                });
            }
            if request.url == WINDSURF_POST_AUTH_URL {
                if let Some(body_text) = self.raw_post_auth_body.clone() {
                    return Ok(OAuthHttpResponse {
                        status_code: 200,
                        body_text,
                        json_body: None,
                    });
                }
                return Ok(OAuthHttpResponse {
                    status_code: 200,
                    body_text: r#"{"sessionToken":"devin-session-token$password","accountId":"acct-password","primaryOrgId":"org-password","planName":"Pro"}"#.to_string(),
                    json_body: Some(json!({
                        "sessionToken": "devin-session-token$password",
                        "accountId": "acct-password",
                        "primaryOrgId": "org-password",
                        "planName": "Pro"
                    })),
                });
            }
            Ok(OAuthHttpResponse {
                status_code: 200,
                body_text: "{}".to_string(),
                json_body: Some(json!({})),
            })
        }
    }

    fn ctx() -> ProviderOAuthTransportContext {
        ProviderOAuthTransportContext {
            provider_id: "provider-windsurf".to_string(),
            provider_type: "windsurf".to_string(),
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

    #[tokio::test]
    async fn imports_raw_api_key_without_network() {
        let executor = RecordingExecutor::default();
        let adapter = WindsurfProviderOAuthAdapter;
        let result = adapter
            .import_credentials(
                &executor,
                &ctx(),
                ProviderOAuthImportInput {
                    provider_type: "windsurf".to_string(),
                    name: Some("Alice".to_string()),
                    refresh_token: None,
                    raw_credentials: Some(json!({
                        "api_key": "devin-session-token$abc",
                        "email": "alice@example.com"
                    })),
                    network: crate::network::OAuthNetworkContext::provider_operation(None),
                },
            )
            .await
            .expect("api key should import");

        assert_eq!(result.token_set.access_token, "devin-session-token$abc");
        assert_eq!(result.auth_config["auth_method"], json!("api_key"));
        assert_eq!(result.auth_config["email"], json!("alice@example.com"));
        assert_eq!(result.auth_config["email_verified"], json!(false));
        assert_eq!(
            result.auth_config["credential_fingerprint"],
            json!(secret_fingerprint("devin-session-token$abc"))
        );
        assert!(executor.requests.lock().expect("requests lock").is_empty());
    }

    #[tokio::test]
    async fn exchanges_show_auth_token_with_register_user() {
        let executor = RecordingExecutor::default();
        let adapter = WindsurfProviderOAuthAdapter;
        let result = adapter
            .import_credentials(
                &executor,
                &ctx(),
                ProviderOAuthImportInput {
                    provider_type: "windsurf".to_string(),
                    name: None,
                    refresh_token: None,
                    raw_credentials: Some(json!({
                        "token": "firebase-id-token",
                        "email": "alice@example.com"
                    })),
                    network: crate::network::OAuthNetworkContext::provider_operation(None),
                },
            )
            .await
            .expect("token should register");

        assert_eq!(result.token_set.access_token, "sk-ws-01-registered");
        assert_eq!(result.auth_config["auth_method"], json!("token"));
        assert_eq!(result.auth_config["register_source"], json!("new"));
        assert!(result.auth_config.get("id_token").is_none());
        assert_eq!(
            result.auth_config["id_token_fingerprint"],
            json!(secret_fingerprint("firebase-id-token"))
        );
        assert_eq!(
            result.auth_config["credential_fingerprint"],
            json!(secret_fingerprint("sk-ws-01-registered"))
        );
        assert_eq!(result.auth_config["email"], json!("alice@example.com"));
        assert_eq!(result.auth_config["email_verified"], json!(true));
        assert_eq!(result.auth_config["account_id"], json!("acct-1"));
        assert_eq!(result.auth_config["primary_org_id"], json!("org-1"));
        assert_eq!(result.auth_config["plan_name"], json!("Pro"));
        let requests = executor.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .json_body
                .as_ref()
                .and_then(|body| body.get("firebase_id_token"))
                .and_then(serde_json::Value::as_str),
            Some("firebase-id-token")
        );
    }

    #[tokio::test]
    async fn imports_email_password_without_storing_password() {
        let executor = RecordingExecutor::default();
        let adapter = WindsurfProviderOAuthAdapter;
        let result = adapter
            .import_credentials(
                &executor,
                &ctx(),
                ProviderOAuthImportInput {
                    provider_type: "windsurf".to_string(),
                    name: None,
                    refresh_token: None,
                    raw_credentials: Some(json!({
                        "email": "alice@example.com",
                        "password": "secret-password"
                    })),
                    network: crate::network::OAuthNetworkContext::provider_operation(None),
                },
            )
            .await
            .expect("email password should import");

        assert_eq!(
            result.token_set.access_token,
            "devin-session-token$password"
        );
        assert_eq!(result.auth_config["auth_method"], json!("email_password"));
        assert_eq!(result.auth_config["email"], json!("alice@example.com"));
        assert_eq!(result.auth_config["email_verified"], json!(true));
        assert_eq!(result.auth_config["account_id"], json!("acct-password"));
        assert_eq!(result.auth_config["primary_org_id"], json!("org-password"));
        assert_eq!(result.auth_config["plan_name"], json!("Pro"));
        assert_eq!(
            result.auth_config["credential_fingerprint"],
            json!(secret_fingerprint("devin-session-token$password"))
        );
        assert!(result.auth_config.get("password").is_none());

        let requests = executor.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0]
                .json_body
                .as_ref()
                .and_then(|body| body.get("password"))
                .and_then(serde_json::Value::as_str),
            Some("secret-password")
        );
        assert_eq!(
            requests[1]
                .headers
                .get("x-devin-auth1-token")
                .map(String::as_str),
            Some("auth1-token")
        );
    }

    #[tokio::test]
    async fn imports_email_password_from_raw_post_auth_body() {
        let executor = RecordingExecutor {
            raw_post_auth_body: Some(
                "\u{0}\u{8}devin-session-token$raw-password-token\u{0}".to_string(),
            ),
            ..RecordingExecutor::default()
        };
        let adapter = WindsurfProviderOAuthAdapter;
        let result = adapter
            .import_credentials(
                &executor,
                &ctx(),
                ProviderOAuthImportInput {
                    provider_type: "windsurf".to_string(),
                    name: None,
                    refresh_token: None,
                    raw_credentials: Some(json!({
                        "email": "alice@example.com",
                        "password": "secret-password"
                    })),
                    network: crate::network::OAuthNetworkContext::provider_operation(None),
                },
            )
            .await
            .expect("raw post auth body should import");

        assert_eq!(
            result.token_set.access_token,
            "devin-session-token$raw-password-token"
        );
        assert_eq!(result.auth_config["auth_method"], json!("email_password"));
        assert_eq!(result.auth_config["email"], json!("alice@example.com"));
        assert!(result.auth_config.get("password").is_none());
    }

    #[tokio::test]
    async fn imports_session_token_from_token_field_without_register_user() {
        let executor = RecordingExecutor::default();
        let adapter = WindsurfProviderOAuthAdapter;
        let result = adapter
            .import_credentials(
                &executor,
                &ctx(),
                ProviderOAuthImportInput {
                    provider_type: "windsurf".to_string(),
                    name: None,
                    refresh_token: None,
                    raw_credentials: Some(json!({
                        "token": "devin-session-token$abc",
                        "email": "alice@example.com"
                    })),
                    network: crate::network::OAuthNetworkContext::provider_operation(None),
                },
            )
            .await
            .expect("session token should import directly");

        assert_eq!(result.token_set.access_token, "devin-session-token$abc");
        assert_eq!(result.auth_config["auth_method"], json!("api_key"));
        assert_eq!(result.auth_config["email"], json!("alice@example.com"));
        assert_eq!(
            result.auth_config["credential_fingerprint"],
            json!(secret_fingerprint("devin-session-token$abc"))
        );
        assert!(executor.requests.lock().expect("requests lock").is_empty());
    }

    #[tokio::test]
    async fn imports_session_token_from_access_token_alias_without_register_user() {
        let executor = RecordingExecutor::default();
        let adapter = WindsurfProviderOAuthAdapter;
        let result = adapter
            .import_credentials(
                &executor,
                &ctx(),
                ProviderOAuthImportInput {
                    provider_type: "windsurf".to_string(),
                    name: None,
                    refresh_token: None,
                    raw_credentials: Some(json!({
                        "access_token": "devin-session-token$alias",
                        "email": "alice@example.com"
                    })),
                    network: crate::network::OAuthNetworkContext::provider_operation(None),
                },
            )
            .await
            .expect("session token alias should import directly");

        assert_eq!(result.token_set.access_token, "devin-session-token$alias");
        assert_eq!(result.auth_config["auth_method"], json!("api_key"));
        assert_eq!(result.auth_config["email"], json!("alice@example.com"));
        assert!(executor.requests.lock().expect("requests lock").is_empty());
    }

    #[test]
    fn windsurf_error_body_redacts_sensitive_fields() {
        let body = truncate_body(
            r#"{"error":"invalid","firebase_id_token":"firebase-id-token","sessionToken":"devin-session-token$abc","nested":{"apiKey":"sk-secret"}}"#,
        );

        assert!(body.contains("[REDACTED]"));
        assert!(!body.contains("firebase-id-token"));
        assert!(!body.contains("devin-session-token$abc"));
        assert!(!body.contains("sk-secret"));
    }
}
