use std::future::Future;
use std::pin::Pin;

use super::antigravity::refresh_antigravity_provider_quota_locally;
use super::chatgpt_web::refresh_chatgpt_web_provider_quota_locally;
use super::codex::refresh_codex_provider_quota_locally;
use super::gemini_cli::refresh_gemini_cli_provider_quota_locally;
use super::grok::refresh_grok_provider_quota_locally;
use super::kiro::refresh_kiro_provider_quota_locally;
use super::windsurf::refresh_windsurf_provider_quota_locally;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};

type ProviderQuotaRefreshFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<serde_json::Value>, GatewayError>> + Send + 'a>>;

type ProviderQuotaRefreshHandler = for<'a> fn(
    &'a AdminAppState<'a>,
    &'a StoredProviderCatalogProvider,
    &'a StoredProviderCatalogEndpoint,
    Vec<StoredProviderCatalogKey>,
    Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a>;

const PROVIDER_QUOTA_REFRESH_HANDLERS: &[(&str, ProviderQuotaRefreshHandler)] = &[
    (
        "antigravity",
        refresh_antigravity_provider_quota_locally_boxed,
    ),
    (
        "chatgpt_web",
        refresh_chatgpt_web_provider_quota_locally_boxed,
    ),
    ("codex", refresh_codex_provider_quota_locally_boxed),
    (
        "gemini_cli",
        refresh_gemini_cli_provider_quota_locally_boxed,
    ),
    ("grok", refresh_grok_provider_quota_locally_boxed),
    ("kiro", refresh_kiro_provider_quota_locally_boxed),
    ("windsurf", refresh_windsurf_provider_quota_locally_boxed),
];

pub(crate) async fn refresh_provider_pool_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    provider_type: &str,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    let Some((_, handler)) = PROVIDER_QUOTA_REFRESH_HANDLERS
        .iter()
        .find(|(supported_provider_type, _)| *supported_provider_type == normalized_provider_type)
    else {
        return Ok(None);
    };
    handler(state, provider, endpoint, keys, proxy_override).await
}

fn refresh_antigravity_provider_quota_locally_boxed<'a>(
    state: &'a AdminAppState<'a>,
    provider: &'a StoredProviderCatalogProvider,
    endpoint: &'a StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a> {
    Box::pin(refresh_antigravity_provider_quota_locally(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
    ))
}

fn refresh_chatgpt_web_provider_quota_locally_boxed<'a>(
    state: &'a AdminAppState<'a>,
    provider: &'a StoredProviderCatalogProvider,
    endpoint: &'a StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a> {
    Box::pin(refresh_chatgpt_web_provider_quota_locally(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
    ))
}

fn refresh_codex_provider_quota_locally_boxed<'a>(
    state: &'a AdminAppState<'a>,
    provider: &'a StoredProviderCatalogProvider,
    endpoint: &'a StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a> {
    Box::pin(refresh_codex_provider_quota_locally(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
    ))
}

fn refresh_gemini_cli_provider_quota_locally_boxed<'a>(
    state: &'a AdminAppState<'a>,
    provider: &'a StoredProviderCatalogProvider,
    endpoint: &'a StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a> {
    Box::pin(refresh_gemini_cli_provider_quota_locally(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
    ))
}

fn refresh_kiro_provider_quota_locally_boxed<'a>(
    state: &'a AdminAppState<'a>,
    provider: &'a StoredProviderCatalogProvider,
    endpoint: &'a StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a> {
    Box::pin(refresh_kiro_provider_quota_locally(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
    ))
}

fn refresh_grok_provider_quota_locally_boxed<'a>(
    state: &'a AdminAppState<'a>,
    provider: &'a StoredProviderCatalogProvider,
    endpoint: &'a StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a> {
    Box::pin(refresh_grok_provider_quota_locally(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
    ))
}

fn refresh_windsurf_provider_quota_locally_boxed<'a>(
    state: &'a AdminAppState<'a>,
    provider: &'a StoredProviderCatalogProvider,
    endpoint: &'a StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> ProviderQuotaRefreshFuture<'a> {
    Box::pin(refresh_windsurf_provider_quota_locally(
        state,
        provider,
        endpoint,
        keys,
        proxy_override,
    ))
}
