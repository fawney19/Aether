use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::credentials::{generate_machine_id, KiroAuthConfig};

pub const PROVIDER_TYPE: &str = "kiro";
pub const KIRO_AUTH_HEADER: &str = "authorization";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroBearerAuth {
    pub name: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroRequestAuth {
    pub name: &'static str,
    pub value: String,
    pub auth_config: KiroAuthConfig,
    pub machine_id: String,
}

pub fn build_kiro_request_auth_from_config(
    auth_config: KiroAuthConfig,
    fallback_secret: Option<&str>,
) -> Option<KiroRequestAuth> {
    let cached_token_needs_refresh = auth_config.cached_access_token_requires_refresh(120);
    let fallback_secret = fallback_secret
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "__placeholder__");
    if cached_token_needs_refresh && auth_config.can_refresh_access_token() {
        return None;
    }

    let token = auth_config
        .cached_access_token()
        .filter(|_| !cached_token_needs_refresh)
        .or(fallback_secret)?;
    let machine_id = generate_machine_id(&auth_config, Some(token))?;

    Some(KiroRequestAuth {
        name: KIRO_AUTH_HEADER,
        value: format!("Bearer {token}"),
        auth_config,
        machine_id,
    })
}

pub fn resolve_local_kiro_bearer_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<KiroBearerAuth> {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(PROVIDER_TYPE)
    {
        return None;
    }
    if transport.key.decrypted_auth_config.is_some() {
        return None;
    }
    if !kiro_auth_type_supported(transport.key.auth_type.as_str()) {
        return None;
    }

    let secret = transport.key.decrypted_api_key.trim();
    if secret.is_empty() {
        return None;
    }

    Some(KiroBearerAuth {
        name: KIRO_AUTH_HEADER,
        value: format!("Bearer {secret}"),
    })
}

pub fn supports_local_kiro_auth_prerequisites(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    resolve_local_kiro_bearer_auth(transport).is_some()
}

pub fn resolve_local_kiro_request_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<KiroRequestAuth> {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(PROVIDER_TYPE)
    {
        return None;
    }
    if !kiro_auth_type_supported(transport.key.auth_type.as_str()) {
        return None;
    }

    let auth_config = KiroAuthConfig::from_raw_json(transport.key.decrypted_auth_config.as_deref())
        .unwrap_or(KiroAuthConfig {
            auth_method: None,
            refresh_token: None,
            expires_at: None,
            profile_arn: None,
            region: None,
            auth_region: None,
            api_region: None,
            client_id: None,
            client_secret: None,
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: None,
        });
    let fallback_secret = transport
        .key
        .decrypted_api_key
        .trim()
        .strip_prefix("__placeholder__")
        .map(|_| "")
        .unwrap_or(transport.key.decrypted_api_key.trim());
    build_kiro_request_auth_from_config(auth_config, Some(fallback_secret))
}

pub fn supports_local_kiro_request_auth_resolution(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    resolve_local_kiro_request_auth(transport).is_some()
        || KiroAuthConfig::from_raw_json(transport.key.decrypted_auth_config.as_deref())
            .is_some_and(|auth_config| {
                transport
                    .provider
                    .provider_type
                    .trim()
                    .eq_ignore_ascii_case(PROVIDER_TYPE)
                    && kiro_auth_type_supported(transport.key.auth_type.as_str())
                    && auth_config.can_refresh_access_token()
            })
}

fn kiro_auth_type_supported(auth_type: &str) -> bool {
    matches!(
        auth_type.trim().to_ascii_lowercase().as_str(),
        "bearer" | "oauth"
    )
}

#[cfg(test)]
mod tests {
    use super::super::super::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use super::{
        resolve_local_kiro_bearer_auth, resolve_local_kiro_request_auth,
        supports_local_kiro_auth_prerequisites, supports_local_kiro_request_auth_resolution,
        KIRO_AUTH_HEADER,
    };

    fn sample_transport() -> GatewayProviderTransportSnapshot {
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
                api_format: "claude:cli".to_string(),
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
                api_formats: Some(vec!["claude:cli".to_string()]),
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "upstream-key".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn resolves_bearer_auth_for_known_kiro_subset() {
        let auth = resolve_local_kiro_bearer_auth(&sample_transport())
            .expect("kiro bearer auth should resolve");
        assert_eq!(auth.name, KIRO_AUTH_HEADER);
        assert_eq!(auth.value, "Bearer upstream-key");
        assert!(supports_local_kiro_auth_prerequisites(&sample_transport()));
    }

    #[test]
    fn rejects_auth_config_subset() {
        let mut transport = sample_transport();
        transport.key.decrypted_auth_config = Some("{\"mode\":\"custom\"}".to_string());
        assert!(resolve_local_kiro_bearer_auth(&transport).is_none());
        assert!(!supports_local_kiro_auth_prerequisites(&transport));
    }

    #[test]
    fn rejects_non_bearer_subset() {
        let mut transport = sample_transport();
        transport.key.auth_type = "api_key".to_string();
        assert!(resolve_local_kiro_bearer_auth(&transport).is_none());
    }

    #[test]
    fn resolves_request_auth_when_legacy_oauth_auth_type_is_used() {
        let mut transport = sample_transport();
        transport.key.auth_type = "oauth".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "access_token":"cached-token",
                "expires_at":4102444800,
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                "machine_id":"123e4567-e89b-12d3-a456-426614174000",
                "api_region":"us-west-2"
            }"#
            .to_string(),
        );

        let auth = resolve_local_kiro_request_auth(&transport)
            .expect("request auth should resolve from legacy oauth auth_type");
        assert_eq!(auth.value, "Bearer cached-token");
        assert!(supports_local_kiro_request_auth_resolution(&transport));
    }

    #[test]
    fn resolves_request_auth_from_cached_access_token() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "access_token":"cached-token",
                "expires_at":4102444800,
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                "machine_id":"123e4567-e89b-12d3-a456-426614174000",
                "api_region":"us-west-2"
            }"#
            .to_string(),
        );

        let auth = resolve_local_kiro_request_auth(&transport)
            .expect("request auth should resolve from cached token");
        assert_eq!(auth.name, KIRO_AUTH_HEADER);
        assert_eq!(auth.value, "Bearer cached-token");
        assert_eq!(auth.auth_config.effective_api_region(), "us-west-2");
        assert_eq!(
            auth.machine_id,
            "123e4567e89b12d3a456426614174000123e4567e89b12d3a456426614174000"
        );
    }

    #[test]
    fn skips_expired_cached_access_token_without_fallback_secret() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "access_token":"expired-token",
                "expires_at": 1,
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
            }"#
            .to_string(),
        );

        assert!(resolve_local_kiro_request_auth(&transport).is_none());
    }

    #[test]
    fn skips_refreshable_cached_access_token_without_expiry() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "access_token":"cached-token-without-expiry",
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
            }"#
            .to_string(),
        );

        assert!(resolve_local_kiro_request_auth(&transport).is_none());
        assert!(supports_local_kiro_request_auth_resolution(&transport));
    }

    #[test]
    fn refreshable_expired_cached_access_token_does_not_fallback_to_decrypted_api_key() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "stale-upstream-token".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "access_token":"expired-token",
                "expires_at": 1,
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
            }"#
            .to_string(),
        );

        assert!(resolve_local_kiro_request_auth(&transport).is_none());
        assert!(supports_local_kiro_request_auth_resolution(&transport));
    }

    #[test]
    fn supports_refresh_only_resolution_without_cached_access_token() {
        let mut transport = sample_transport();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
            }"#
            .to_string(),
        );

        assert!(resolve_local_kiro_request_auth(&transport).is_none());
        assert!(supports_local_kiro_request_auth_resolution(&transport));
    }
}
