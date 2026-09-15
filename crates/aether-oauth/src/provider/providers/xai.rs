use super::generic::{template_for_provider_type, GenericProviderOAuthAdapter};
use crate::core::{
    current_unix_secs, redacted_oauth_error_body_excerpt, OAuthDeviceAuthorization, OAuthError,
};
use crate::network::{OAuthHttpExecutor, OAuthHttpRequest};
use crate::provider::{
    ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthCapabilities,
    ProviderOAuthImportInput, ProviderOAuthRequestAuth, ProviderOAuthTokenSet,
    ProviderOAuthTransportContext,
};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use url::form_urlencoded;

pub const XAI_PROVIDER_TYPE: &str = "xai";
pub const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
pub const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
pub const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
pub const XAI_OAUTH_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "grok-cli:access",
    "api:access",
];
pub const XAI_DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

const DEFAULT_DEVICE_EXPIRES_IN_SECS: u64 = 600;
const DEFAULT_DEVICE_POLL_INTERVAL_SECS: u64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub enum XaiDevicePollOutcome {
    Pending,
    SlowDown,
    Authorized(Box<ProviderOAuthTokenSet>),
}

#[derive(Clone)]
pub struct XaiProviderOAuthAdapter {
    inner: GenericProviderOAuthAdapter,
    device_url_override: Option<String>,
}

impl std::fmt::Debug for XaiProviderOAuthAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XaiProviderOAuthAdapter")
            .field(
                "has_device_url_override",
                &self.device_url_override.is_some(),
            )
            .finish_non_exhaustive()
    }
}

impl Default for XaiProviderOAuthAdapter {
    fn default() -> Self {
        Self {
            inner: GenericProviderOAuthAdapter::new(
                template_for_provider_type(XAI_PROVIDER_TYPE).expect("xai template should exist"),
            ),
            device_url_override: None,
        }
    }
}

impl XaiProviderOAuthAdapter {
    pub fn with_endpoint_overrides(
        mut self,
        device_url: impl Into<String>,
        token_url: impl Into<String>,
    ) -> Self {
        self.device_url_override = Some(device_url.into());
        self.inner = self.inner.with_token_url_override(token_url);
        self
    }

    fn device_url(&self) -> String {
        self.device_url_override
            .clone()
            .unwrap_or_else(|| XAI_DEVICE_CODE_URL.to_string())
    }

    pub async fn start_device_flow(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
    ) -> Result<OAuthDeviceAuthorization, OAuthError> {
        let form = form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", XAI_CLIENT_ID)
            .append_pair("scope", &XAI_OAUTH_SCOPES.join(" "))
            .finish()
            .into_bytes();
        let response = executor
            .execute(OAuthHttpRequest {
                request_id: "provider-oauth:xai-device-code".to_string(),
                method: reqwest::Method::POST,
                url: self.device_url(),
                headers: form_headers(),
                content_type: Some("application/x-www-form-urlencoded".to_string()),
                json_body: None,
                body_bytes: Some(form),
                network: ctx.network.clone(),
                transport_profile: None,
            })
            .await?;
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: redacted_oauth_error_body_excerpt(&response.body_text),
            });
        }
        let payload = response_json(&response)
            .ok_or_else(|| OAuthError::invalid_response("xAI device code response is not json"))?;
        parse_device_authorization(&payload)
    }

    pub async fn poll_device_token(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        device_code: &str,
    ) -> Result<XaiDevicePollOutcome, OAuthError> {
        let device_code = device_code.trim();
        if device_code.is_empty() {
            return Err(OAuthError::invalid_request("xAI device_code is required"));
        }
        let form = form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", XAI_DEVICE_CODE_GRANT_TYPE)
            .append_pair("device_code", device_code)
            .append_pair("client_id", XAI_CLIENT_ID)
            .finish()
            .into_bytes();
        let response = executor
            .execute(OAuthHttpRequest {
                request_id: "provider-oauth:xai-device-token".to_string(),
                method: reqwest::Method::POST,
                url: self.inner.token_url_for_provider(),
                headers: form_headers(),
                content_type: Some("application/x-www-form-urlencoded".to_string()),
                json_body: None,
                body_bytes: Some(form),
                network: ctx.network.clone(),
                transport_profile: None,
            })
            .await?;
        let payload = response_json(&response);
        if let Some(error_code) = payload.as_ref().and_then(oauth_error_code) {
            return match error_code.as_str() {
                "authorization_pending" => Ok(XaiDevicePollOutcome::Pending),
                "slow_down" => Ok(XaiDevicePollOutcome::SlowDown),
                "expired_token" => Err(OAuthError::invalid_request("xAI device code expired")),
                "access_denied" => Err(OAuthError::invalid_request(
                    "xAI device authorization denied",
                )),
                other => Err(OAuthError::invalid_response(format!(
                    "xAI device token error: {other}"
                ))),
            };
        }
        if !(200..300).contains(&response.status_code) {
            return Err(OAuthError::HttpStatus {
                status_code: response.status_code,
                body_excerpt: redacted_oauth_error_body_excerpt(&response.body_text),
            });
        }
        let payload = payload
            .ok_or_else(|| OAuthError::invalid_response("xAI device token response is not json"))?;
        let mut token_set = self.inner.token_set_from_payload(payload)?;
        let raw_payload = token_set.token_set.raw_payload.clone();
        mark_oauth_auth_config(&mut token_set.auth_config);
        enrich_xai_identity(&mut token_set.auth_config, raw_payload.as_ref());
        Ok(XaiDevicePollOutcome::Authorized(Box::new(token_set)))
    }

    async fn import_raw_api_key(
        &self,
        input: &ProviderOAuthImportInput,
        api_key: &str,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(OAuthError::invalid_request("xAI api_key is required"));
        }
        let mut auth_config = Map::new();
        auth_config.insert("provider_type".to_string(), json!(XAI_PROVIDER_TYPE));
        auth_config.insert("auth_method".to_string(), json!("api_key"));
        auth_config.insert("using_api".to_string(), json!(true));
        auth_config.insert("updated_at".to_string(), json!(current_unix_secs()));
        if let Some(name) = input
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            auth_config.insert("name".to_string(), json!(name));
        }
        Ok(ProviderOAuthTokenSet {
            token_set: crate::core::OAuthTokenSet {
                access_token: api_key.to_string(),
                refresh_token: None,
                token_type: Some("Bearer".to_string()),
                scope: None,
                expires_at_unix_secs: None,
                raw_payload: None,
            },
            auth_config: Value::Object(auth_config),
        })
    }
}

#[async_trait]
impl ProviderOAuthAdapter for XaiProviderOAuthAdapter {
    fn provider_type(&self) -> &'static str {
        XAI_PROVIDER_TYPE
    }

    fn capabilities(&self) -> ProviderOAuthCapabilities {
        ProviderOAuthCapabilities {
            supports_authorization_code: false,
            supports_cookie_authorization: false,
            supports_refresh_token_import: true,
            supports_batch_import: true,
            supports_device_flow: true,
            supports_account_probe: false,
            rotates_refresh_token: true,
        }
    }

    async fn import_credentials(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        input: ProviderOAuthImportInput,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        if let Some(api_key) =
            raw_credential_string(input.raw_credentials.as_ref(), &["api_key", "apiKey"])
        {
            return self.import_raw_api_key(&input, &api_key).await;
        }
        let refresh_token = input
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                raw_credential_string(
                    input.raw_credentials.as_ref(),
                    &["refresh_token", "refreshToken"],
                )
            });
        if let Some(refresh_token) = refresh_token {
            let mut imported = self
                .inner
                .import_credentials(
                    executor,
                    ctx,
                    ProviderOAuthImportInput {
                        refresh_token: Some(refresh_token),
                        ..input
                    },
                )
                .await?;
            let raw_payload = imported.token_set.raw_payload.clone();
            mark_oauth_auth_config(&mut imported.auth_config);
            enrich_xai_identity(&mut imported.auth_config, raw_payload.as_ref());
            return Ok(imported);
        }
        if let Some(access_token) = raw_credential_string(
            input.raw_credentials.as_ref(),
            &["access_token", "accessToken"],
        ) {
            return self.import_raw_api_key(&input, &access_token).await;
        }
        Err(OAuthError::invalid_request(
            "xAI credentials require api_key, access_token, or refresh_token",
        ))
    }

    async fn refresh(
        &self,
        executor: &dyn OAuthHttpExecutor,
        ctx: &ProviderOAuthTransportContext,
        account: &ProviderOAuthAccount,
    ) -> Result<ProviderOAuthTokenSet, OAuthError> {
        let mut refreshed = self.inner.refresh(executor, ctx, account).await?;
        let raw_payload = refreshed.token_set.raw_payload.clone();
        mark_oauth_auth_config(&mut refreshed.auth_config);
        enrich_xai_identity(&mut refreshed.auth_config, raw_payload.as_ref());
        Ok(refreshed)
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

fn mark_oauth_auth_config(auth_config: &mut Value) {
    let Some(object) = auth_config.as_object_mut() else {
        return;
    };
    object.insert("provider_type".to_string(), json!(XAI_PROVIDER_TYPE));
    object.insert("auth_method".to_string(), json!("oauth"));
    object.insert("using_api".to_string(), json!(false));
}

fn enrich_xai_identity(auth_config: &mut Value, raw_payload: Option<&Value>) {
    let Some(object) = auth_config.as_object_mut() else {
        return;
    };
    let id_token = raw_payload
        .and_then(|payload| payload.get("id_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(id_token) = id_token {
        object
            .entry("id_token".to_string())
            .or_insert_with(|| json!(id_token));
        if let Some(claims) = decode_jwt_claims(id_token) {
            if !object.contains_key("email") {
                if let Some(email) = claims.get("email").and_then(Value::as_str) {
                    let email = email.trim();
                    if !email.is_empty() {
                        object.insert("email".to_string(), json!(email));
                    }
                }
            }
            if !object.contains_key("sub") {
                if let Some(sub) = claims.get("sub").and_then(Value::as_str) {
                    let sub = sub.trim();
                    if !sub.is_empty() {
                        object.insert("sub".to_string(), json!(sub));
                    }
                }
            }
        }
    }
}

fn parse_device_authorization(payload: &Value) -> Result<OAuthDeviceAuthorization, OAuthError> {
    let device_code =
        json_non_empty_string(payload, &["device_code", "deviceCode"]).ok_or_else(|| {
            OAuthError::invalid_response("xAI device code response missing device_code")
        })?;
    let user_code =
        json_non_empty_string(payload, &["user_code", "userCode"]).ok_or_else(|| {
            OAuthError::invalid_response("xAI device code response missing user_code")
        })?;
    let verification_uri = json_non_empty_string(
        payload,
        &["verification_uri", "verificationUri", "verification_url"],
    )
    .unwrap_or_default();
    let verification_uri_complete = json_non_empty_string(
        payload,
        &[
            "verification_uri_complete",
            "verificationUriComplete",
            "verification_url_complete",
        ],
    )
    .unwrap_or_else(|| verification_uri.clone());
    if verification_uri.is_empty() && verification_uri_complete.is_empty() {
        return Err(OAuthError::invalid_response(
            "xAI device code response missing verification URI",
        ));
    }
    Ok(OAuthDeviceAuthorization {
        device_code,
        user_code,
        verification_uri: if verification_uri.is_empty() {
            verification_uri_complete.clone()
        } else {
            verification_uri
        },
        verification_uri_complete,
        expires_in: json_u64(payload, &["expires_in", "expiresIn"])
            .unwrap_or(DEFAULT_DEVICE_EXPIRES_IN_SECS),
        interval: json_u64(payload, &["interval"]).unwrap_or(DEFAULT_DEVICE_POLL_INTERVAL_SECS),
    })
}

fn raw_credential_string(raw: Option<&Value>, keys: &[&str]) -> Option<String> {
    let object = raw?.as_object()?;
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn response_json(response: &crate::network::OAuthHttpResponse) -> Option<Value> {
    response
        .json_body
        .clone()
        .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
}

fn oauth_error_code(payload: &Value) -> Option<String> {
    payload
        .get("error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_non_empty_string(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        payload
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn json_u64(payload: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| match payload.get(*key)? {
        Value::Number(number) => number.as_u64(),
        Value::String(string) => string.trim().parse::<u64>().ok(),
        _ => None,
    })
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

fn decode_jwt_claims(token: &str) -> Option<Map<String, Value>> {
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
        XaiDevicePollOutcome, XaiProviderOAuthAdapter, XAI_CLIENT_ID, XAI_DEVICE_CODE_GRANT_TYPE,
        XAI_OAUTH_SCOPES, XAI_PROVIDER_TYPE,
    };
    use crate::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
    use crate::provider::{
        ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthImportInput,
        ProviderOAuthTransportContext,
    };
    use async_trait::async_trait;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct ScriptedExecutor {
        seen_request: Arc<Mutex<Option<OAuthHttpRequest>>>,
        status_code: u16,
        payload: Value,
    }

    #[async_trait]
    impl OAuthHttpExecutor for ScriptedExecutor {
        async fn execute(
            &self,
            request: OAuthHttpRequest,
        ) -> Result<OAuthHttpResponse, crate::core::OAuthError> {
            *self.seen_request.lock().expect("mutex should lock") = Some(request);
            Ok(OAuthHttpResponse {
                status_code: self.status_code,
                body_text: self.payload.to_string(),
                json_body: Some(self.payload.clone()),
            })
        }
    }

    fn transport_context() -> ProviderOAuthTransportContext {
        ProviderOAuthTransportContext {
            provider_id: "provider-xai".to_string(),
            provider_type: XAI_PROVIDER_TYPE.to_string(),
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

    fn encoded_jwt(claims: &Value) -> String {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims should encode"))
        )
    }

    #[tokio::test]
    async fn imports_api_key_as_official_api_credential() {
        let adapter = XaiProviderOAuthAdapter::default();
        let executor = ScriptedExecutor {
            seen_request: Arc::new(Mutex::new(None)),
            status_code: 200,
            payload: json!({}),
        };
        let result = adapter
            .import_credentials(
                &executor,
                &transport_context(),
                ProviderOAuthImportInput {
                    provider_type: XAI_PROVIDER_TYPE.to_string(),
                    name: Some("work".to_string()),
                    refresh_token: None,
                    raw_credentials: Some(json!({"api_key": "xai-key-123"})),
                    network: crate::network::OAuthNetworkContext::provider_operation(None),
                },
            )
            .await
            .expect("api key import should succeed");

        assert_eq!(result.token_set.access_token, "xai-key-123");
        assert_eq!(result.auth_config["using_api"], json!(true));
        assert_eq!(result.auth_config["auth_method"], json!("api_key"));
        assert!(executor.seen_request.lock().expect("lock").is_none());
    }

    #[tokio::test]
    async fn device_poll_treats_authorization_pending_as_pending() {
        let adapter = XaiProviderOAuthAdapter::default();
        let seen = Arc::new(Mutex::new(None));
        let executor = ScriptedExecutor {
            seen_request: Arc::clone(&seen),
            status_code: 400,
            payload: json!({"error": "authorization_pending"}),
        };
        let outcome = adapter
            .poll_device_token(&executor, &transport_context(), "device-code")
            .await
            .expect("pending should not be fatal");
        assert_eq!(outcome, XaiDevicePollOutcome::Pending);

        let request = seen.lock().expect("lock").clone().expect("request");
        let body = request.body_bytes.expect("body");
        let fields = url::form_urlencoded::parse(&body)
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(fields["grant_type"], XAI_DEVICE_CODE_GRANT_TYPE);
        assert_eq!(fields["device_code"], "device-code");
        assert_eq!(fields["client_id"], XAI_CLIENT_ID);
    }

    #[tokio::test]
    async fn refresh_posts_client_id_and_refresh_token_without_scope() {
        let adapter = XaiProviderOAuthAdapter::default();
        let seen = Arc::new(Mutex::new(None));
        let id_token = encoded_jwt(&json!({"email": "user@x.ai", "sub": "subject-1"}));
        let executor = ScriptedExecutor {
            seen_request: Arc::clone(&seen),
            status_code: 200,
            payload: json!({
                "access_token": "new-access",
                "refresh_token": "new-refresh",
                "id_token": id_token,
                "expires_in": 3600
            }),
        };
        let account = ProviderOAuthAccount {
            provider_type: XAI_PROVIDER_TYPE.to_string(),
            access_token: "old-access".to_string(),
            auth_config: json!({
                "provider_type": XAI_PROVIDER_TYPE,
                "refresh_token": "old-refresh",
                "using_api": false,
            }),
            expires_at_unix_secs: None,
            identity: BTreeMap::new(),
        };
        let result = adapter
            .refresh(&executor, &transport_context(), &account)
            .await
            .expect("refresh should succeed");
        assert_eq!(result.token_set.access_token, "new-access");
        assert_eq!(result.auth_config["using_api"], json!(false));
        assert_eq!(result.auth_config["email"], json!("user@x.ai"));
        assert_eq!(result.auth_config["sub"], json!("subject-1"));

        let request = seen.lock().expect("lock").clone().expect("request");
        let body = request.body_bytes.expect("body");
        let fields = url::form_urlencoded::parse(&body)
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(fields["grant_type"], "refresh_token");
        assert_eq!(fields["client_id"], XAI_CLIENT_ID);
        assert_eq!(fields["refresh_token"], "old-refresh");
        assert!(!fields.contains_key("scope"));
        assert!(XAI_OAUTH_SCOPES.join(" ").contains("grok-cli:access"));
    }

    #[tokio::test]
    async fn start_device_flow_posts_client_id_and_scope() {
        let adapter = XaiProviderOAuthAdapter::default();
        let seen = Arc::new(Mutex::new(None));
        let executor = ScriptedExecutor {
            seen_request: Arc::clone(&seen),
            status_code: 200,
            payload: json!({
                "device_code": "dc-1",
                "user_code": "ABCD-EFGH",
                "verification_uri": "https://auth.x.ai/device",
                "verification_uri_complete": "https://auth.x.ai/device?user_code=ABCD-EFGH",
                "expires_in": 600,
                "interval": 5
            }),
        };
        let authorization = adapter
            .start_device_flow(&executor, &transport_context())
            .await
            .expect("device start should succeed");
        assert_eq!(authorization.user_code, "ABCD-EFGH");
        assert_eq!(authorization.device_code, "dc-1");

        let request = seen.lock().expect("lock").clone().expect("request");
        let body = request.body_bytes.expect("body");
        let fields = url::form_urlencoded::parse(&body)
            .into_owned()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(fields["client_id"], XAI_CLIENT_ID);
        assert_eq!(fields["scope"], XAI_OAUTH_SCOPES.join(" "));
    }
}
