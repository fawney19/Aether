use std::collections::BTreeMap;

use aether_crypto::{rsa_pkcs1_sha256_sign, RsaPkcs1Sha256Error};
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::{form_urlencoded, Url};

use super::super::oauth_refresh::{
    CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthHttpRequest, LocalOAuthRefreshAdapter,
    LocalOAuthRefreshError, LocalResolvedOAuthRequestAuth,
};
use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::context::is_valid_vertex_region;

pub const VERTEX_API_KEY_QUERY_PARAM: &str = "key";
pub const VERTEX_SERVICE_ACCOUNT_AUTH_HEADER: &str = "authorization";
pub const VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE: &str = "vertex_ai";
pub const GOOGLE_OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const SERVICE_ACCOUNT_REFRESH_SKEW_SECS: u64 = 120;

#[derive(Clone, PartialEq, Eq)]
pub struct VertexApiKeyQueryAuth {
    pub name: &'static str,
    pub value: String,
}

impl std::fmt::Debug for VertexApiKeyQueryAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VertexApiKeyQueryAuth")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct VertexServiceAccountAuthConfig {
    pub client_email: String,
    pub private_key: String,
    pub project_id: String,
    pub token_uri: String,
    pub region: Option<String>,
    pub model_regions: BTreeMap<String, String>,
}

impl std::fmt::Debug for VertexServiceAccountAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VertexServiceAccountAuthConfig")
            .field("client_email", &self.client_email)
            .field("private_key", &"[REDACTED]")
            .field("project_id", &self.project_id)
            .field("token_uri", &"[REDACTED]")
            .field("region", &self.region)
            .field("model_regions", &self.model_regions)
            .finish()
    }
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
    let token_uri = resolve_vertex_service_account_token_uri(value.get("token_uri"))?;
    let region = json_string(value.get("region")).filter(|value| is_valid_vertex_region(value));
    let model_regions = value
        .get("model_regions")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(model, region)| {
                    let model = model.trim();
                    let region = region.as_str()?.trim();
                    (!model.is_empty() && is_valid_vertex_region(region))
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

fn resolve_vertex_service_account_token_uri(value: Option<&Value>) -> Option<String> {
    let Some(value) = value else {
        return Some(GOOGLE_OAUTH_TOKEN_URL.to_string());
    };
    let raw = value.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = Url::parse(raw).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !parsed
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("oauth2.googleapis.com"))
        || parsed.port().is_some()
        || parsed.path() != "/token"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(GOOGLE_OAUTH_TOKEN_URL.to_string())
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
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        if !entry
            .provider_type
            .eq_ignore_ascii_case(VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE)
        {
            return None;
        }
        if !vertex_service_account_cached_entry_matches_transport(transport, entry) {
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

    fn refresh_fingerprint(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<String> {
        let source_fingerprint = vertex_service_account_credential_fingerprint(transport)?;
        Some(
            entry
                .filter(|entry| {
                    vertex_service_account_cached_entry_matches_transport(transport, entry)
                })
                .map(|entry| {
                    let mut digest = Sha256::new();
                    digest.update(source_fingerprint.as_bytes());
                    digest.update([0]);
                    digest.update(entry.auth_header_value.as_bytes());
                    digest.update([0]);
                    digest.update(entry.expires_at_unix_secs.unwrap_or_default().to_be_bytes());
                    format!("{:x}", digest.finalize())
                })
                .unwrap_or(source_fingerprint),
        )
    }

    fn shares_refresh_through_transport_persistence(&self) -> bool {
        false
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
            source_fingerprint: vertex_service_account_credential_fingerprint(transport),
        }))
    }
}

fn vertex_service_account_credential_fingerprint(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    supports_local_vertex_service_account_auth_resolution(transport).then(|| {
        let provider_type = transport.provider.provider_type.trim().to_ascii_lowercase();
        let auth_type = transport.key.auth_type.trim().to_ascii_lowercase();
        let auth_config = transport
            .key
            .decrypted_auth_config
            .as_deref()
            .unwrap_or_default();
        let mut digest = Sha256::new();
        for field in [
            provider_type.as_bytes(),
            auth_type.as_bytes(),
            auth_config.as_bytes(),
            transport.key.decrypted_api_key.as_bytes(),
        ] {
            digest.update((field.len() as u64).to_be_bytes());
            digest.update(field);
        }
        format!("{:x}", digest.finalize())
    })
}

fn vertex_service_account_cached_entry_matches_transport(
    transport: &GatewayProviderTransportSnapshot,
    entry: &CachedOAuthEntry,
) -> bool {
    entry
        .provider_type
        .eq_ignore_ascii_case(VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE)
        && vertex_service_account_credential_fingerprint(transport)
            .as_deref()
            .is_some_and(|fingerprint| entry.source_fingerprint.as_deref() == Some(fingerprint))
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
    let signature = rsa_pkcs1_sha256_sign(auth_config.private_key.as_bytes(), message.as_bytes())
        .map_err(|error| LocalOAuthRefreshError::InvalidResponse {
        provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
        message: match error {
            RsaPkcs1Sha256Error::InvalidPrivateKey => {
                "vertex service account private_key parse failed".to_string()
            }
            _ => "vertex service account signing failed".to_string(),
        },
    })?;
    Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
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
    aether_oauth::core::redacted_oauth_error_body_excerpt(value)
}

#[cfg(test)]
mod tests {
    use super::super::super::oauth_refresh::{
        CachedOAuthEntry, LocalOAuthRefreshAdapter, LocalResolvedOAuthRequestAuth,
    };
    use super::super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
    use aws_lc_rs::rsa::{KeyPair as AwsRsaKeyPair, KeySize};
    use aws_lc_rs::signature::{KeyPair as _, UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use base64::Engine as _;
    use serde_json::{json, Value};

    use super::{
        build_vertex_service_account_assertion, parse_vertex_service_account_auth_config,
        resolve_local_vertex_api_key_query_auth,
        supports_local_vertex_service_account_auth_resolution,
        vertex_service_account_credential_fingerprint, VertexApiKeyQueryAuth,
        VertexServiceAccountAuthConfig, VertexServiceAccountRefreshAdapter, GOOGLE_OAUTH_TOKEN_URL,
        VERTEX_API_KEY_QUERY_PARAM, VERTEX_SERVICE_ACCOUNT_AUTH_HEADER,
        VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
    };

    #[test]
    fn vertex_auth_debug_output_redacts_api_keys_and_private_keys() {
        let query_auth = VertexApiKeyQueryAuth {
            name: VERTEX_API_KEY_QUERY_PARAM,
            value: "vertex-api-key-canary".to_string(),
        };
        let service_account = VertexServiceAccountAuthConfig {
            client_email: "service@example.invalid".to_string(),
            private_key: "vertex-private-key-canary".to_string(),
            project_id: "project-1".to_string(),
            token_uri: GOOGLE_OAUTH_TOKEN_URL.to_string(),
            region: None,
            model_regions: std::collections::BTreeMap::new(),
        };

        let query_debug = format!("{query_auth:?}");
        assert!(!query_debug.contains("vertex-api-key-canary"));
        assert!(query_debug.contains("[REDACTED]"));
        let service_account_debug = format!("{service_account:?}");
        assert!(!service_account_debug.contains("vertex-private-key-canary"));
        assert!(service_account_debug.contains("[REDACTED]"));
    }

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

    fn read_der_tlv<'a>(input: &mut &'a [u8], expected_tag: u8) -> &'a [u8] {
        assert_eq!(input.first().copied(), Some(expected_tag));
        let length_byte = input[1];
        let (header_len, value_len) = if length_byte & 0x80 == 0 {
            (2, usize::from(length_byte))
        } else {
            let length_bytes = usize::from(length_byte & 0x7f);
            let value_len = input[2..2 + length_bytes]
                .iter()
                .fold(0usize, |value, byte| (value << 8) | usize::from(*byte));
            (2 + length_bytes, value_len)
        };
        let end = header_len + value_len;
        let value = &input[header_len..end];
        *input = &input[end..];
        value
    }

    fn pkcs1_private_key_from_pkcs8(pkcs8: &[u8]) -> Vec<u8> {
        let mut input = pkcs8;
        let mut sequence = read_der_tlv(&mut input, 0x30);
        let _version = read_der_tlv(&mut sequence, 0x02);
        let _algorithm = read_der_tlv(&mut sequence, 0x30);
        read_der_tlv(&mut sequence, 0x04).to_vec()
    }

    fn sample_service_account_transport(private_key: &str) -> GatewayProviderTransportSnapshot {
        let mut transport = sample_transport();
        transport.key.auth_type = "service_account".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            serde_json::json!({
                "client_email": "svc@example.iam.gserviceaccount.com",
                "private_key": private_key,
                "project_id": "demo-project"
            })
            .to_string(),
        );
        transport
    }

    #[test]
    fn cached_token_is_bound_to_vertex_service_account_generation() {
        let source_transport = sample_service_account_transport("SOURCE-PRIVATE-KEY");
        let source_fingerprint = vertex_service_account_credential_fingerprint(&source_transport)
            .expect("source service account should have a fingerprint");
        let entry = CachedOAuthEntry {
            provider_type: VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE.to_string(),
            auth_header_name: VERTEX_SERVICE_ACCOUNT_AUTH_HEADER.to_string(),
            auth_header_value: "Bearer source-access-token".to_string(),
            expires_at_unix_secs: Some(u64::MAX),
            metadata: None,
            source_fingerprint: Some(source_fingerprint),
        };
        let adapter = VertexServiceAccountRefreshAdapter;

        assert!(!adapter.shares_refresh_through_transport_persistence());
        assert_eq!(
            adapter.resolve_cached(&source_transport, &entry),
            Some(LocalResolvedOAuthRequestAuth::Header {
                name: VERTEX_SERVICE_ACCOUNT_AUTH_HEADER.to_string(),
                value: "Bearer source-access-token".to_string(),
            })
        );
        assert_ne!(
            adapter.refresh_fingerprint(&source_transport, None),
            adapter.refresh_fingerprint(&source_transport, Some(&entry))
        );

        let replacement_transport = sample_service_account_transport("ADMIN-PRIVATE-KEY");
        assert!(adapter
            .resolve_cached(&replacement_transport, &entry)
            .is_none());
        assert_eq!(
            adapter.refresh_fingerprint(&replacement_transport, Some(&entry)),
            vertex_service_account_credential_fingerprint(&replacement_transport)
        );
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
        assert_eq!(config.token_uri, GOOGLE_OAUTH_TOKEN_URL);
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
    fn service_account_regions_reject_url_syntax() {
        let raw = r#"{
            "client_email":"svc@example.iam.gserviceaccount.com",
            "private_key":"TEST-PRIVATE-KEY",
            "project_id":"demo-project",
            "region":"attacker.example/",
            "model_regions":{
                "gemini-2.0-flash":"attacker.example/",
                "gemini-2.5-pro":"us-central1"
            }
        }"#;
        let config = parse_vertex_service_account_auth_config(Some(raw))
            .expect("service account config should parse");
        assert!(config.region.is_none());
        assert!(!config.model_regions.contains_key("gemini-2.0-flash"));
        assert_eq!(
            config
                .model_regions
                .get("gemini-2.5-pro")
                .map(String::as_str),
            Some("us-central1")
        );
    }

    #[test]
    fn service_account_token_uri_is_limited_to_google_oauth_endpoint() {
        let config_with_token_uri = |token_uri: Value| {
            serde_json::json!({
                "client_email": "svc@example.iam.gserviceaccount.com",
                "private_key": "TEST-PRIVATE-KEY",
                "project_id": "demo-project",
                "token_uri": token_uri,
            })
            .to_string()
        };

        let official = parse_vertex_service_account_auth_config(Some(&config_with_token_uri(
            Value::String(GOOGLE_OAUTH_TOKEN_URL.to_string()),
        )))
        .expect("official Google OAuth token URI should be accepted");
        assert_eq!(official.token_uri, GOOGLE_OAUTH_TOKEN_URL);

        for token_uri in [
            Value::String("http://oauth2.googleapis.com/token".to_string()),
            Value::String("https://127.0.0.1/token".to_string()),
            Value::String("https://oauth2.googleapis.com.evil.example/token".to_string()),
            Value::String("https://user@oauth2.googleapis.com/token".to_string()),
            Value::String("https://oauth2.googleapis.com:8443/token".to_string()),
            Value::String("https://oauth2.googleapis.com/token/../metadata".to_string()),
            Value::String("https://oauth2.googleapis.com/token?target=metadata".to_string()),
            Value::String("https://oauth2.googleapis.com/token#fragment".to_string()),
            Value::String(String::new()),
            Value::Null,
        ] {
            let raw = config_with_token_uri(token_uri.clone());
            assert!(
                parse_vertex_service_account_auth_config(Some(&raw)).is_none(),
                "token URI should be rejected: {token_uri}"
            );
        }
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
    fn signs_with_2048_bit_pkcs1_service_account_private_key() {
        let key_pair = AwsRsaKeyPair::generate(KeySize::Rsa2048)
            .expect("2048-bit test RSA private key should generate");
        let pkcs8 = AsDer::<Pkcs8V1Der<'static>>::as_der(&key_pair)
            .expect("test RSA private key should encode as PKCS#8");
        let pkcs1 = pkcs1_private_key_from_pkcs8(pkcs8.as_ref());
        let private_key = format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
            STANDARD.encode(pkcs1)
        );
        let auth_config = parse_vertex_service_account_auth_config(Some(
            &json!({
                "client_email": "svc@example.iam.gserviceaccount.com",
                "private_key": private_key,
                "project_id": "demo-project"
            })
            .to_string(),
        ))
        .expect("service account config should parse");
        let assertion = build_vertex_service_account_assertion(&auth_config, 1_700_000_000)
            .expect("PKCS#1 private key should sign");
        let parts = assertion.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 3);
        let message = format!("{}.{}", parts[0], parts[1]);
        let signature = URL_SAFE_NO_PAD
            .decode(parts[2])
            .expect("JWT signature should decode");
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key_pair.public_key().as_ref())
            .verify(message.as_bytes(), &signature)
            .expect("AWS-LC signature should verify");
    }
}
