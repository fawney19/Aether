use std::collections::BTreeMap;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;
use serde_json::{json, Value};
use sha2::Sha256;
use url::form_urlencoded;

use super::super::oauth_refresh::{
    CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthHttpRequest, LocalOAuthRefreshAdapter,
    LocalOAuthRefreshError, LocalResolvedOAuthRequestAuth,
};
use super::super::snapshot::GatewayProviderTransportSnapshot;

pub const VERTEX_API_KEY_QUERY_PARAM: &str = "key";
pub const VERTEX_SERVICE_ACCOUNT_AUTH_HEADER: &str = "authorization";
pub const VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE: &str = "vertex_ai";
pub const GOOGLE_OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const SERVICE_ACCOUNT_REFRESH_SKEW_SECS: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexApiKeyQueryAuth {
    pub name: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexServiceAccountAuthConfig {
    pub client_email: String,
    pub private_key: String,
    pub project_id: String,
    pub token_uri: String,
    pub region: Option<String>,
    pub model_regions: BTreeMap<String, String>,
}

pub fn resolve_local_vertex_api_key_query_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<VertexApiKeyQueryAuth> {
    if !super::is_vertex_api_key_transport_context(transport) {
        return None;
    }

    if transport.key.decrypted_auth_config.is_some() {
        return None;
    }

    if !transport
        .key
        .auth_type
        .trim()
        .eq_ignore_ascii_case("api_key")
    {
        return None;
    }

    let secret = transport.key.decrypted_api_key.trim();
    if secret.is_empty() {
        return None;
    }

    Some(VertexApiKeyQueryAuth {
        name: VERTEX_API_KEY_QUERY_PARAM,
        value: secret.to_string(),
    })
}

pub fn resolve_local_vertex_service_account_auth_config(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<VertexServiceAccountAuthConfig> {
    if !super::is_vertex_service_account_transport_context(transport) {
        return None;
    }
    parse_vertex_service_account_auth_config(transport.key.decrypted_auth_config.as_deref())
}

pub fn supports_local_vertex_service_account_auth_resolution(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    resolve_local_vertex_service_account_auth_config(transport).is_some()
}

pub fn parse_vertex_service_account_auth_config(
    raw: Option<&str>,
) -> Option<VertexServiceAccountAuthConfig> {
    let raw = raw.map(str::trim).filter(|value| !value.is_empty())?;
    let value: Value = serde_json::from_str(raw).ok()?;
    parse_vertex_service_account_auth_config_value(&value)
}

fn parse_vertex_service_account_auth_config_value(
    value: &Value,
) -> Option<VertexServiceAccountAuthConfig> {
    let client_email = json_string(value.get("client_email"))?;
    let private_key = json_string(value.get("private_key"))?;
    let project_id = json_string(value.get("project_id"))?;
    let token_uri =
        json_string(value.get("token_uri")).unwrap_or_else(|| GOOGLE_OAUTH_TOKEN_URL.to_string());
    let region = json_string(value.get("region"));
    let model_regions = value
        .get("model_regions")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(model, region)| {
                    let model = model.trim();
                    let region = region.as_str()?.trim();
                    (!model.is_empty() && !region.is_empty())
                        .then(|| (model.to_string(), region.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    Some(VertexServiceAccountAuthConfig {
        client_email,
        private_key,
        project_id,
        token_uri,
        region,
        model_regions,
    })
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Default)]
pub struct VertexServiceAccountRefreshAdapter;

#[async_trait]
impl LocalOAuthRefreshAdapter for VertexServiceAccountRefreshAdapter {
    fn provider_type(&self) -> &'static str {
        VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE
    }

    fn supports(&self, transport: &GatewayProviderTransportSnapshot) -> bool {
        supports_local_vertex_service_account_auth_resolution(transport)
    }

    fn resolve_cached(
        &self,
        _transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        if !entry
            .provider_type
            .eq_ignore_ascii_case(VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE)
        {
            return None;
        }
        if service_account_token_expires_soon(entry.expires_at_unix_secs) {
            return None;
        }
        let name = entry.auth_header_name.trim();
        let value = entry.auth_header_value.trim();
        if name.is_empty() || value.is_empty() {
            return None;
        }
        Some(LocalResolvedOAuthRequestAuth::Header {
            name: name.to_ascii_lowercase(),
            value: value.to_string(),
        })
    }

    fn resolve_without_refresh(
        &self,
        _transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        None
    }

    fn should_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> bool {
        supports_local_vertex_service_account_auth_resolution(transport)
            && entry
                .and_then(|cached| self.resolve_cached(transport, cached))
                .is_none()
    }

    async fn refresh(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        _entry: Option<&CachedOAuthEntry>,
    ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
        let Some(auth_config) = resolve_local_vertex_service_account_auth_config(transport) else {
            return Ok(None);
        };
        let now = aether_oauth::core::current_unix_secs();
        let assertion = build_vertex_service_account_assertion(&auth_config, now)?;
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer")
            .append_pair("assertion", &assertion)
            .finish();
        let response = executor
            .execute(
                VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
                transport,
                &LocalOAuthHttpRequest {
                    request_id: "vertex_ai:service-account-token",
                    method: reqwest::Method::POST,
                    url: auth_config.token_uri.clone(),
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "application/x-www-form-urlencoded".to_string(),
                    )]),
                    json_body: None,
                    body_bytes: Some(body.into_bytes()),
                },
            )
            .await?;
        if response.status_code != 200 {
            return Err(LocalOAuthRefreshError::HttpStatus {
                provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
                status_code: response.status_code,
                body_excerpt: body_excerpt(&response.body_text),
            });
        }
        let body_json: Value = serde_json::from_str(&response.body_text).map_err(|err| {
            LocalOAuthRefreshError::InvalidResponse {
                provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
                message: format!("vertex service account token response is not JSON: {err}"),
            }
        })?;
        let access_token = json_string(body_json.get("access_token")).ok_or_else(|| {
            LocalOAuthRefreshError::InvalidResponse {
                provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
                message: "vertex service account token response missing access_token".to_string(),
            }
        })?;
        let expires_in = body_json
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3600);

        Ok(Some(CachedOAuthEntry {
            provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE.to_string(),
            auth_header_name: VERTEX_SERVICE_ACCOUNT_AUTH_HEADER.to_string(),
            auth_header_value: format!("Bearer {access_token}"),
            expires_at_unix_secs: Some(now.saturating_add(expires_in)),
            metadata: Some(json!({
                "project_id": auth_config.project_id,
                "client_email": auth_config.client_email,
            })),
        }))
    }
}

pub fn build_vertex_service_account_assertion(
    auth_config: &VertexServiceAccountAuthConfig,
    now_unix_secs: u64,
) -> Result<String, LocalOAuthRefreshError> {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_string(&json!({
            "iss": auth_config.client_email,
            "sub": auth_config.client_email,
            "scope": GOOGLE_CLOUD_PLATFORM_SCOPE,
            "aud": auth_config.token_uri,
            "iat": now_unix_secs,
            "exp": now_unix_secs.saturating_add(3600),
        }))
        .map_err(|err| LocalOAuthRefreshError::InvalidResponse {
            provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
            message: format!("vertex service account jwt payload encode failed: {err}"),
        })?,
    );
    let message = format!("{header}.{payload}");
    let private_key = decode_vertex_service_account_private_key(auth_config.private_key.as_str())?;
    let signing_key = SigningKey::<Sha256>::new(private_key);
    let signature = signing_key.sign(message.as_bytes());
    Ok(format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

fn decode_vertex_service_account_private_key(
    private_key_pem: &str,
) -> Result<RsaPrivateKey, LocalOAuthRefreshError> {
    match RsaPrivateKey::from_pkcs8_pem(private_key_pem) {
        Ok(private_key) => Ok(private_key),
        Err(pkcs8_err) => RsaPrivateKey::from_pkcs1_pem(private_key_pem).map_err(|pkcs1_err| {
            LocalOAuthRefreshError::InvalidResponse {
                provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
                message: format!(
                    "vertex service account private_key parse failed: pkcs8: {pkcs8_err}; pkcs1: {pkcs1_err}"
                ),
            }
        }),
    }
}

fn service_account_token_expires_soon(expires_at_unix_secs: Option<u64>) -> bool {
    expires_at_unix_secs
        .map(|expires_at_unix_secs| {
            aether_oauth::core::current_unix_secs()
                >= expires_at_unix_secs.saturating_sub(SERVICE_ACCOUNT_REFRESH_SKEW_SECS)
        })
        .unwrap_or(true)
}

fn body_excerpt(value: &str) -> String {
    value.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use rsa::pkcs1::{EncodeRsaPrivateKey, LineEnding};
    use rsa::rand_core::OsRng;
    use rsa::RsaPrivateKey;

    use super::{
        decode_vertex_service_account_private_key, parse_vertex_service_account_auth_config,
        resolve_local_vertex_api_key_query_auth,
        supports_local_vertex_service_account_auth_resolution, VERTEX_API_KEY_QUERY_PARAM,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Vertex".to_string(),
                provider_type: "vertex_ai".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "gemini:generate_content".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://aiplatform.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["gemini:generate_content".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "vertex-secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn resolves_query_auth_for_vertex_api_key_subset() {
        let auth = resolve_local_vertex_api_key_query_auth(&sample_transport())
            .expect("vertex api key query auth should resolve");
        assert_eq!(auth.name, VERTEX_API_KEY_QUERY_PARAM);
        assert_eq!(auth.value, "vertex-secret");
    }

    #[test]
    fn rejects_non_api_key_transport() {
        let mut transport = sample_transport();
        transport.key.auth_type = "service_account".to_string();
        assert!(resolve_local_vertex_api_key_query_auth(&transport).is_none());
    }

    #[test]
    fn rejects_vertex_auth_config_transport() {
        let mut transport = sample_transport();
        transport.key.decrypted_auth_config = Some("{\"project_id\":\"demo-project\"}".to_string());
        assert!(resolve_local_vertex_api_key_query_auth(&transport).is_none());
    }

    #[test]
    fn resolves_query_auth_for_custom_aiplatform_transport() {
        let mut transport = sample_transport();
        transport.provider.provider_type = "custom".to_string();
        transport.endpoint.api_format = "gemini:generate_content".to_string();

        let auth = resolve_local_vertex_api_key_query_auth(&transport)
            .expect("custom aiplatform transport should resolve");
        assert_eq!(auth.value, "vertex-secret");
    }

    #[test]
    fn parses_service_account_auth_config() {
        let config = parse_vertex_service_account_auth_config(Some(
            r#"{
                "client_email":"svc@example.iam.gserviceaccount.com",
                "private_key":"TEST-PRIVATE-KEY",
                "project_id":"demo-project",
                "region":"global",
                "model_regions":{"gemini-2.0-flash":"us-central1"}
            }"#,
        ))
        .expect("service account config should parse");

        assert_eq!(config.client_email, "svc@example.iam.gserviceaccount.com");
        assert_eq!(config.project_id, "demo-project");
        assert_eq!(config.region.as_deref(), Some("global"));
        assert_eq!(
            config
                .model_regions
                .get("gemini-2.0-flash")
                .map(String::as_str),
            Some("us-central1")
        );
    }

    #[test]
    fn supports_vertex_service_account_auth_resolution() {
        let mut transport = sample_transport();
        transport.key.auth_type = "service_account".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "client_email":"svc@example.iam.gserviceaccount.com",
                "private_key":"TEST-PRIVATE-KEY",
                "project_id":"demo-project"
            }"#
            .to_string(),
        );

        assert!(supports_local_vertex_service_account_auth_resolution(
            &transport
        ));
    }

    #[test]
    fn decodes_pkcs1_service_account_private_key() {
        let mut rng = OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 1024)
            .expect("test RSA private key should generate")
            .to_pkcs1_pem(LineEnding::LF)
            .expect("test RSA private key should encode as PKCS#1 PEM");

        decode_vertex_service_account_private_key(private_key.as_str())
            .expect("PKCS#1 private key should decode");
    }
}
