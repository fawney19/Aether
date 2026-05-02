use std::collections::BTreeMap;

use aether_oauth::provider::providers::KiroProviderOAuthAdapter as CoreKiroProviderOAuthAdapter;
use aether_oauth::provider::{ProviderOAuthAccount, ProviderOAuthAdapter};
use async_trait::async_trait;

use super::super::oauth_refresh::{
    oauth_error_to_local_refresh_error, provider_oauth_transport_context_from_snapshot,
    CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthRefreshAdapter, LocalOAuthRefreshError,
    LocalResolvedOAuthRequestAuth, ProviderOAuthLocalHttpExecutor,
};
use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::auth::{
    build_kiro_request_auth_from_config, resolve_local_kiro_request_auth, PROVIDER_TYPE,
};
use super::credentials::KiroAuthConfig;

#[cfg(test)]
const IDC_AMZ_USER_AGENT: &str = "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";

#[derive(Debug, Clone, Default)]
pub struct KiroOAuthRefreshAdapter {
    social_refresh_base_url: Option<String>,
    idc_refresh_base_url: Option<String>,
}

impl KiroOAuthRefreshAdapter {
    pub fn with_refresh_base_urls(
        mut self,
        social_refresh_base_url: Option<String>,
        idc_refresh_base_url: Option<String>,
    ) -> Self {
        self.social_refresh_base_url = social_refresh_base_url;
        self.idc_refresh_base_url = idc_refresh_base_url;
        self
    }

    pub async fn refresh_auth_config(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        auth_config: &KiroAuthConfig,
    ) -> Result<KiroAuthConfig, LocalOAuthRefreshError> {
        let adapter = CoreKiroProviderOAuthAdapter::default().with_refresh_base_urls(
            self.social_refresh_base_url.clone(),
            self.idc_refresh_base_url.clone(),
        );
        let oauth_executor =
            ProviderOAuthLocalHttpExecutor::new(PROVIDER_TYPE, transport, executor);
        let ctx = provider_oauth_transport_context_from_snapshot(transport);
        let account = ProviderOAuthAccount {
            provider_type: PROVIDER_TYPE.to_string(),
            access_token: auth_config
                .cached_access_token()
                .map(ToOwned::to_owned)
                .unwrap_or_default(),
            auth_config: auth_config.to_json_value(),
            expires_at_unix_secs: auth_config.expires_at,
            identity: BTreeMap::new(),
        };
        let refreshed = adapter
            .refresh(&oauth_executor, &ctx, &account)
            .await
            .map_err(|error| oauth_error_to_local_refresh_error(PROVIDER_TYPE, error))?;
        KiroAuthConfig::from_json_value(&refreshed.auth_config).ok_or_else(|| {
            LocalOAuthRefreshError::InvalidResponse {
                provider_type: PROVIDER_TYPE,
                message: "kiro refresh returned invalid auth_config".to_string(),
            }
        })
    }

    fn auth_config_from_entry(entry: &CachedOAuthEntry) -> Option<KiroAuthConfig> {
        entry
            .metadata
            .as_ref()
            .filter(|_| entry.provider_type.eq_ignore_ascii_case(PROVIDER_TYPE))
            .and_then(KiroAuthConfig::from_json_value)
    }

    fn base_auth_config(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<KiroAuthConfig> {
        entry.and_then(Self::auth_config_from_entry).or_else(|| {
            KiroAuthConfig::from_raw_json(transport.key.decrypted_auth_config.as_deref())
        })
    }

    fn build_cached_entry(auth_config: &KiroAuthConfig) -> Option<CachedOAuthEntry> {
        let request_auth = build_kiro_request_auth_from_config(auth_config.clone(), None)?;
        Some(CachedOAuthEntry {
            provider_type: PROVIDER_TYPE.to_string(),
            auth_header_name: request_auth.name.to_string(),
            auth_header_value: request_auth.value,
            expires_at_unix_secs: auth_config.expires_at,
            metadata: Some(auth_config.to_json_value()),
        })
    }

    fn refreshable_auth_config(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<KiroAuthConfig> {
        let auth_config = self.base_auth_config(transport, entry)?;
        auth_config
            .can_refresh_access_token()
            .then_some(auth_config)
    }
}

#[async_trait]
impl LocalOAuthRefreshAdapter for KiroOAuthRefreshAdapter {
    fn provider_type(&self) -> &'static str {
        PROVIDER_TYPE
    }

    fn resolve_cached(
        &self,
        _transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        let auth_config = Self::auth_config_from_entry(entry)?;
        let request_auth = build_kiro_request_auth_from_config(auth_config, None)?;
        Some(LocalResolvedOAuthRequestAuth::Kiro(request_auth))
    }

    fn resolve_without_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        resolve_local_kiro_request_auth(transport).map(LocalResolvedOAuthRequestAuth::Kiro)
    }

    fn should_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> bool {
        entry
            .and_then(|cached| self.resolve_cached(transport, cached))
            .is_none()
            && self.resolve_without_refresh(transport).is_none()
            && self.refreshable_auth_config(transport, entry).is_some()
    }

    async fn refresh(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
        let Some(auth_config) = self.refreshable_auth_config(transport, entry) else {
            return Ok(None);
        };
        let refreshed = self
            .refresh_auth_config(executor, transport, &auth_config)
            .await?;
        Ok(Self::build_cached_entry(&refreshed))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::super::super::oauth_refresh::{
        LocalOAuthRefreshAdapter, LocalResolvedOAuthRequestAuth, ReqwestLocalOAuthHttpExecutor,
    };
    use super::super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{KiroOAuthRefreshAdapter, IDC_AMZ_USER_AGENT};
    use axum::body::to_bytes;
    use axum::extract::Request;
    use axum::response::IntoResponse;
    use axum::routing::any;
    use axum::{Json, Router};
    use http::StatusCode;
    use serde_json::{json, Value};
    use tokio::task::JoinHandle;

    #[derive(Debug, Clone)]
    struct SeenRefreshRequest {
        body: Value,
        authorization: String,
        host: String,
        user_agent: String,
        x_amz_user_agent: String,
    }

    fn sample_transport(raw_auth_config: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Kiro".to_string(),
                provider_type: "kiro".to_string(),
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
                api_format: "claude:messages".to_string(),
                api_family: Some("claude".to_string()),
                endpoint_kind: Some("cli".to_string()),
                is_active: true,
                base_url: "https://kiro.example".to_string(),
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
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: Some(vec!["claude:messages".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "__placeholder__".to_string(),
                decrypted_auth_config: Some(raw_auth_config.to_string()),
            },
        }
    }

    async fn start_server(app: Router) -> (String, JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener
            .local_addr()
            .expect("listener should expose local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn refreshes_social_token_via_adapter() {
        let seen_request = Arc::new(Mutex::new(None::<SeenRefreshRequest>));
        let seen_request_clone = Arc::clone(&seen_request);
        let server = Router::new().route(
            "/refreshToken",
            any(move |request: Request| {
                let seen_request_inner = Arc::clone(&seen_request_clone);
                async move {
                    let (parts, body) = request.into_parts();
                    let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                    let body: Value =
                        serde_json::from_slice(&raw_body).expect("body should parse as json");
                    *seen_request_inner.lock().expect("mutex should lock") =
                        Some(SeenRefreshRequest {
                            body,
                            authorization: parts
                                .headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                            host: parts
                                .headers
                                .get("host")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                            user_agent: parts
                                .headers
                                .get("user-agent")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                            x_amz_user_agent: parts
                                .headers
                                .get("x-amz-user-agent")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        });
                    (
                        StatusCode::OK,
                        Json(json!({
                            "accessToken": "cached-kiro-access-token",
                            "refreshToken": "s".repeat(120),
                            "expiresIn": 3600,
                            "profileArn": "arn:aws:bedrock:demo"
                        })),
                    )
                        .into_response()
                }
            }),
        );
        let (server_url, server_handle) = start_server(server).await;
        let adapter =
            KiroOAuthRefreshAdapter::default().with_refresh_base_urls(Some(server_url), None);
        let transport = sample_transport(
            r#"{
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                "machine_id":"123e4567-e89b-12d3-a456-426614174000",
                "kiro_version":"1.2.3"
            }"#,
        );
        let executor = ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new());

        let entry = adapter
            .refresh(&executor, &transport, None)
            .await
            .expect("refresh should succeed")
            .expect("cached entry should exist");
        let resolved = adapter
            .resolve_cached(&transport, &entry)
            .expect("cached entry should resolve");
        let seen_request = seen_request
            .lock()
            .expect("mutex should lock")
            .clone()
            .expect("refresh request should be captured");

        assert_eq!(seen_request.body["refreshToken"], json!("r".repeat(120)));
        assert_eq!(seen_request.authorization, "");
        assert!(!seen_request.user_agent.is_empty());
        assert_eq!(seen_request.x_amz_user_agent, "");
        assert!(!seen_request.host.trim().is_empty());
        match resolved {
            LocalResolvedOAuthRequestAuth::Kiro(auth) => {
                assert_eq!(auth.value, "Bearer cached-kiro-access-token");
                assert_eq!(
                    auth.auth_config.profile_arn.as_deref(),
                    Some("arn:aws:bedrock:demo")
                );
                assert!(auth.auth_config.expires_at.is_some());
            }
            other => panic!("unexpected resolved auth: {other:?}"),
        }

        server_handle.abort();
    }

    #[tokio::test]
    async fn refreshes_idc_token_via_adapter() {
        let seen_request = Arc::new(Mutex::new(None::<SeenRefreshRequest>));
        let seen_request_clone = Arc::clone(&seen_request);
        let server = Router::new().route(
            "/token",
            any(move |request: Request| {
                let seen_request_inner = Arc::clone(&seen_request_clone);
                async move {
                    let (parts, body) = request.into_parts();
                    let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                    let body: Value =
                        serde_json::from_slice(&raw_body).expect("body should parse as json");
                    *seen_request_inner.lock().expect("mutex should lock") =
                        Some(SeenRefreshRequest {
                            body,
                            authorization: parts
                                .headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                            host: parts
                                .headers
                                .get("host")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                            user_agent: parts
                                .headers
                                .get("user-agent")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                            x_amz_user_agent: parts
                                .headers
                                .get("x-amz-user-agent")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        });
                    (
                        StatusCode::OK,
                        Json(json!({
                            "accessToken": "cached-idc-access-token",
                            "refreshToken": "i".repeat(120),
                            "expiresIn": 1800
                        })),
                    )
                        .into_response()
                }
            }),
        );
        let (server_url, server_handle) = start_server(server).await;
        let adapter =
            KiroOAuthRefreshAdapter::default().with_refresh_base_urls(None, Some(server_url));
        let transport = sample_transport(
            r#"{
                "auth_method":"identity_center",
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                "client_id":"cid",
                "client_secret":"secret",
                "profile_arn":"arn:aws:bedrock:demo"
            }"#,
        );
        let executor = ReqwestLocalOAuthHttpExecutor::new(reqwest::Client::new());

        let entry = adapter
            .refresh(&executor, &transport, None)
            .await
            .expect("refresh should succeed")
            .expect("cached entry should exist");
        let resolved = adapter
            .resolve_cached(&transport, &entry)
            .expect("cached entry should resolve");
        let seen_request = seen_request
            .lock()
            .expect("mutex should lock")
            .clone()
            .expect("refresh request should be captured");

        assert_eq!(
            seen_request.body["grantType"].as_str(),
            Some("refresh_token")
        );
        assert_eq!(seen_request.body["clientId"].as_str(), Some("cid"));
        assert_eq!(seen_request.user_agent, "node");
        assert_eq!(seen_request.x_amz_user_agent, IDC_AMZ_USER_AGENT);
        assert!(!seen_request.host.trim().is_empty());
        match resolved {
            LocalResolvedOAuthRequestAuth::Kiro(auth) => {
                assert_eq!(auth.value, "Bearer cached-idc-access-token");
                assert!(auth.auth_config.profile_arn_for_payload().is_none());
                assert!(auth.auth_config.expires_at.is_some());
            }
            other => panic!("unexpected resolved auth: {other:?}"),
        }

        server_handle.abort();
    }
}
