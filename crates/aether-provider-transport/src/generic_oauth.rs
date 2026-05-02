use std::collections::BTreeMap;

use aether_oauth::provider::providers::{
    GenericProviderOAuthAdapter, GENERIC_PROVIDER_OAUTH_TEMPLATES,
};
use aether_oauth::provider::{ProviderOAuthAccount, ProviderOAuthAdapter, ProviderOAuthTokenSet};
use async_trait::async_trait;
use serde_json::Value;

use super::oauth_refresh::{
    oauth_error_to_local_refresh_error, provider_oauth_transport_context_from_snapshot,
    CachedOAuthEntry, LocalOAuthHttpExecutor, LocalOAuthRefreshAdapter, LocalOAuthRefreshError,
    LocalResolvedOAuthRequestAuth, ProviderOAuthLocalHttpExecutor,
};
use super::snapshot::GatewayProviderTransportSnapshot;

const AUTH_HEADER_NAME: &str = "authorization";
const OAUTH_REFRESH_SKEW_SECS: u64 = 120;
const PLACEHOLDER_API_KEY: &str = "__placeholder__";

pub fn supports_local_generic_oauth_request_auth_resolution(
    transport: &GatewayProviderTransportSnapshot,
) -> bool {
    transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
        && generic_provider_type(transport.provider.provider_type.as_str()).is_some()
}

#[derive(Debug, Clone, Default)]
pub struct GenericOAuthRefreshAdapter {
    token_url_overrides: BTreeMap<String, String>,
}

impl GenericOAuthRefreshAdapter {
    pub fn with_token_url_for_tests(
        mut self,
        provider_type: &str,
        token_url: impl Into<String>,
    ) -> Self {
        self.token_url_overrides
            .insert(provider_type.trim().to_ascii_lowercase(), token_url.into());
        self
    }

    fn adapter_for_provider_type(
        &self,
        provider_type: &'static str,
    ) -> Option<GenericProviderOAuthAdapter> {
        let adapter = GenericProviderOAuthAdapter::for_provider_type(provider_type)?;
        if let Some(token_url) = self.token_url_overrides.get(provider_type) {
            return Some(adapter.with_token_url_override(token_url.clone()));
        }
        Some(adapter)
    }

    fn auth_config_from_transport(transport: &GatewayProviderTransportSnapshot) -> Option<Value> {
        transport
            .key
            .decrypted_auth_config
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| serde_json::from_str::<Value>(value).ok())
    }

    fn auth_config_from_entry(
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<Value> {
        entry
            .metadata
            .as_ref()
            .filter(|_| {
                entry
                    .provider_type
                    .eq_ignore_ascii_case(transport.provider.provider_type.as_str())
            })
            .cloned()
    }

    fn auth_config_updated_at(auth_config: &Value) -> Option<u64> {
        auth_config
            .as_object()
            .and_then(|object| object.get("updated_at"))
            .and_then(|value| parse_u64_value(Some(value)))
    }

    fn base_auth_config(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Option<Value> {
        let cached = entry.and_then(|cached| Self::auth_config_from_entry(transport, cached));
        let transport_auth = Self::auth_config_from_transport(transport);

        match (cached, transport_auth) {
            (Some(cached), Some(transport_auth)) => {
                let cached_updated_at = Self::auth_config_updated_at(&cached);
                let transport_updated_at = Self::auth_config_updated_at(&transport_auth);
                if transport_updated_at > cached_updated_at {
                    Some(transport_auth)
                } else {
                    Some(cached)
                }
            }
            (Some(cached), None) => Some(cached),
            (None, Some(transport_auth)) => Some(transport_auth),
            (None, None) => None,
        }
    }

    fn resolve_direct_header(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        if !supports_local_generic_oauth_request_auth_resolution(transport) {
            return None;
        }

        let secret = transport.key.decrypted_api_key.trim();
        if secret.is_empty() || secret == PLACEHOLDER_API_KEY {
            return None;
        }

        let auth_config = Self::auth_config_from_transport(transport);
        let refreshable = auth_config
            .as_ref()
            .and_then(refresh_token_from_auth_config)
            .is_some();
        if refreshable && auth_config_expires_soon(auth_config.as_ref()) {
            return None;
        }

        Some(LocalResolvedOAuthRequestAuth::Header {
            name: AUTH_HEADER_NAME.to_string(),
            value: format!("Bearer {secret}"),
        })
    }

    fn build_cached_entry(
        provider_type: &'static str,
        refreshed: ProviderOAuthTokenSet,
    ) -> CachedOAuthEntry {
        CachedOAuthEntry {
            provider_type: provider_type.to_string(),
            auth_header_name: AUTH_HEADER_NAME.to_string(),
            auth_header_value: refreshed.token_set.bearer_header_value(),
            expires_at_unix_secs: refreshed.token_set.expires_at_unix_secs,
            metadata: Some(refreshed.auth_config),
        }
    }
}

#[async_trait]
impl LocalOAuthRefreshAdapter for GenericOAuthRefreshAdapter {
    fn provider_type(&self) -> &'static str {
        "generic_oauth"
    }

    fn supports(&self, transport: &GatewayProviderTransportSnapshot) -> bool {
        supports_local_generic_oauth_request_auth_resolution(transport)
    }

    fn resolve_cached(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: &CachedOAuthEntry,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        if !entry
            .provider_type
            .eq_ignore_ascii_case(transport.provider.provider_type.as_str())
        {
            return None;
        }
        if expires_at_requires_refresh(entry.expires_at_unix_secs) {
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
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<LocalResolvedOAuthRequestAuth> {
        self.resolve_direct_header(transport)
    }

    fn should_refresh(
        &self,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> bool {
        if !supports_local_generic_oauth_request_auth_resolution(transport) {
            return false;
        }
        if entry
            .and_then(|cached| self.resolve_cached(transport, cached))
            .is_some()
            || self.resolve_direct_header(transport).is_some()
        {
            return false;
        }

        self.base_auth_config(transport, entry)
            .as_ref()
            .and_then(refresh_token_from_auth_config)
            .is_some()
    }

    async fn refresh(
        &self,
        executor: &dyn LocalOAuthHttpExecutor,
        transport: &GatewayProviderTransportSnapshot,
        entry: Option<&CachedOAuthEntry>,
    ) -> Result<Option<CachedOAuthEntry>, LocalOAuthRefreshError> {
        let Some(provider_type) = generic_provider_type(transport.provider.provider_type.as_str())
        else {
            return Ok(None);
        };
        let Some(auth_config) = self.base_auth_config(transport, entry) else {
            return Ok(None);
        };
        let Some(refresh_token) = refresh_token_from_auth_config(&auth_config) else {
            tracing::warn!(
                key_id = %transport.key.id,
                provider_id = %transport.provider.id,
                provider_type,
                "gateway generic oauth refresh skipped because auth_config has no refresh_token"
            );
            return Ok(None);
        };
        let Some(adapter) = self.adapter_for_provider_type(provider_type) else {
            return Ok(None);
        };

        tracing::info!(
            key_id = %transport.key.id,
            provider_id = %transport.provider.id,
            endpoint_id = %transport.endpoint.id,
            provider_type,
            request_refresh_token_len = refresh_token.len(),
            "gateway generic oauth refresh delegated to provider oauth adapter"
        );

        let oauth_executor =
            ProviderOAuthLocalHttpExecutor::new(provider_type, transport, executor);
        let ctx = provider_oauth_transport_context_from_snapshot(transport);
        let account = ProviderOAuthAccount {
            provider_type: provider_type.to_string(),
            access_token: current_access_token(transport, entry).unwrap_or_default(),
            expires_at_unix_secs: auth_config_expires_at(&auth_config),
            auth_config,
            identity: BTreeMap::new(),
        };
        let refreshed = adapter
            .refresh(&oauth_executor, &ctx, &account)
            .await
            .map_err(|error| oauth_error_to_local_refresh_error(provider_type, error))?;

        tracing::info!(
            key_id = %transport.key.id,
            provider_id = %transport.provider.id,
            endpoint_id = %transport.endpoint.id,
            provider_type,
            expires_at_unix_secs = ?refreshed.token_set.expires_at_unix_secs,
            response_has_refresh_token = refreshed.token_set.refresh_token.is_some(),
            "gateway generic oauth refresh succeeded"
        );

        Ok(Some(Self::build_cached_entry(provider_type, refreshed)))
    }
}

fn generic_provider_type(provider_type: &str) -> Option<&'static str> {
    let normalized = provider_type.trim();
    GENERIC_PROVIDER_OAUTH_TEMPLATES
        .iter()
        .find(|template| normalized.eq_ignore_ascii_case(template.provider_type))
        .map(|template| template.provider_type)
}

fn refresh_token_from_auth_config(auth_config: &Value) -> Option<String> {
    auth_config
        .as_object()
        .and_then(|object| object.get("refresh_token"))
        .and_then(non_empty_string)
}

fn auth_config_expires_at(auth_config: &Value) -> Option<u64> {
    auth_config
        .as_object()
        .and_then(|object| object.get("expires_at"))
        .and_then(|value| parse_u64_value(Some(value)))
}

fn auth_config_expires_soon(auth_config: Option<&Value>) -> bool {
    expires_at_requires_refresh(auth_config.and_then(auth_config_expires_at))
}

fn expires_at_requires_refresh(expires_at_unix_secs: Option<u64>) -> bool {
    expires_at_unix_secs
        .map(|expires_at_unix_secs| {
            aether_oauth::core::current_unix_secs()
                >= expires_at_unix_secs.saturating_sub(OAUTH_REFRESH_SKEW_SECS)
        })
        .unwrap_or(false)
}

fn parse_u64_value(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(string) => string.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn non_empty_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn current_access_token(
    transport: &GatewayProviderTransportSnapshot,
    entry: Option<&CachedOAuthEntry>,
) -> Option<String> {
    entry
        .and_then(|entry| {
            entry
                .auth_header_value
                .trim()
                .strip_prefix("Bearer ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            let secret = transport.key.decrypted_api_key.trim();
            (!secret.is_empty() && secret != PLACEHOLDER_API_KEY).then(|| secret.to_string())
        })
}
