use super::{
    admin_provider_pool_config, build_admin_pool_error_response,
    read_admin_provider_pool_cooldown_counts,
    ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::provider::pool as admin_provider_pool_pure;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use std::collections::BTreeMap;

pub(super) async fn build_admin_pool_overview_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
        ));
    }

    let providers = state.list_provider_catalog_providers(false).await?;
    let pool_enabled_providers = providers
        .into_iter()
        .filter_map(|provider| {
            admin_provider_pool_config(&provider).map(|config| (provider, config))
        })
        .collect::<Vec<_>>();
    let provider_ids = pool_enabled_providers
        .iter()
        .map(|(provider, _)| provider.id.clone())
        .collect::<Vec<_>>();
    let key_stats = if provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_key_stats_by_provider_ids(&provider_ids)
            .await?
    };
    let key_stats_by_provider = key_stats
        .into_iter()
        .map(|item| (item.provider_id.clone(), item))
        .collect::<BTreeMap<_, _>>();
    let redis_runner = state.redis_kv_runner();
    let cooldown_counts_by_provider = match redis_runner.as_ref() {
        Some(runner) if !provider_ids.is_empty() => {
            read_admin_provider_pool_cooldown_counts(runner, &provider_ids).await
        }
        _ => BTreeMap::new(),
    };

    let providers = pool_enabled_providers
        .into_iter()
        .map(|(provider, _)| provider)
        .collect::<Vec<_>>();

    Ok(
        Json(admin_provider_pool_pure::build_admin_pool_overview_payload(
            &providers,
            &key_stats_by_provider,
            &cooldown_counts_by_provider,
        ))
        .into_response(),
    )
}
