use super::value::build_admin_provider_summary_value;
use crate::handlers::admin::request::AdminAppState;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn build_admin_provider_summary_payload(
    state: &AdminAppState<'_>,
    provider_id: &str,
) -> Option<serde_json::Value> {
    let state = state.as_ref();
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let provider_ids = vec![provider_id.to_string()];
    let provider = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await
        .ok()?
        .into_iter()
        .next()?;
    let (
        endpoints_result,
        keys_result,
        quota_snapshot_result,
        model_stats_result,
        active_global_model_ids_result,
    ) = tokio::join!(
        state.list_provider_catalog_endpoints_by_provider_ids(&provider_ids),
        state.list_provider_catalog_key_summaries_by_provider_ids(&provider_ids),
        state.read_provider_quota_snapshot(provider_id),
        state.list_provider_model_stats(&provider_ids),
        state.list_active_global_model_ids_by_provider_ids(&provider_ids),
    );
    let endpoints = endpoints_result.ok().unwrap_or_default();
    let keys = keys_result.ok().unwrap_or_default();
    let quota_snapshot = quota_snapshot_result.ok().flatten();
    let model_stats = model_stats_result
        .ok()
        .unwrap_or_default()
        .into_iter()
        .find(|stats| stats.provider_id == provider_id);
    let active_global_model_ids = active_global_model_ids_result
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.provider_id == provider_id)
        .map(|row| row.global_model_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(build_admin_provider_summary_value(
        &provider,
        &endpoints,
        &keys,
        quota_snapshot.as_ref(),
        model_stats.as_ref(),
        active_global_model_ids,
        now_unix_secs,
    ))
}

pub(crate) async fn build_admin_providers_summary_payload(
    state: &AdminAppState<'_>,
    page: usize,
    page_size: usize,
    search: &str,
    status: &str,
    api_format: &str,
    model_id: &str,
) -> Option<serde_json::Value> {
    let state = state.as_ref();
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let normalized_search = search.trim().to_ascii_lowercase();
    let search_keywords = normalized_search
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let normalized_status = status.trim().to_ascii_lowercase();
    let normalized_api_format = api_format.trim();
    let normalized_model_id = model_id.trim();
    let requires_api_format_filter =
        normalized_api_format != "all" && !normalized_api_format.is_empty();
    let requires_model_filter = normalized_model_id != "all" && !normalized_model_id.is_empty();

    let mut providers = state
        .list_provider_catalog_providers(false)
        .await
        .ok()
        .unwrap_or_default();
    let all_provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let all_endpoints = if !requires_api_format_filter || all_provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_provider_catalog_endpoints_by_provider_ids(&all_provider_ids)
            .await
            .ok()
            .unwrap_or_default()
    };
    let active_global_model_refs = if !requires_model_filter || all_provider_ids.is_empty() {
        Vec::new()
    } else {
        state
            .list_active_global_model_ids_by_provider_ids(&all_provider_ids)
            .await
            .ok()
            .unwrap_or_default()
    };

    let mut api_formats_by_provider = BTreeMap::<String, BTreeSet<String>>::new();
    for endpoint in &all_endpoints {
        api_formats_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .insert(endpoint.api_format.clone());
    }
    let mut active_global_model_ids_by_provider = BTreeMap::<String, BTreeSet<String>>::new();
    for row in active_global_model_refs {
        active_global_model_ids_by_provider
            .entry(row.provider_id)
            .or_default()
            .insert(row.global_model_id);
    }

    providers.retain(|provider| {
        if !search_keywords.is_empty() {
            let provider_name = provider.name.to_ascii_lowercase();
            if !search_keywords
                .iter()
                .all(|keyword| provider_name.contains(keyword))
            {
                return false;
            }
        }

        match normalized_status.as_str() {
            "active" if !provider.is_active => return false,
            "inactive" if provider.is_active => return false,
            _ => {}
        }

        if requires_api_format_filter
            && !api_formats_by_provider
                .get(&provider.id)
                .is_some_and(|items| items.contains(normalized_api_format))
        {
            return false;
        }

        if requires_model_filter
            && !active_global_model_ids_by_provider
                .get(&provider.id)
                .is_some_and(|items| items.contains(normalized_model_id))
        {
            return false;
        }

        true
    });

    providers.sort_by(|left, right| {
        right
            .is_active
            .cmp(&left.is_active)
            .then_with(|| left.provider_priority.cmp(&right.provider_priority))
            .then_with(|| left.created_at_unix_ms.cmp(&right.created_at_unix_ms))
    });

    let total = providers.len();
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let providers = providers
        .into_iter()
        .skip(offset)
        .take(page_size)
        .collect::<Vec<_>>();
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let (endpoints, keys, model_stats, page_active_global_model_refs) = if provider_ids.is_empty() {
        (Vec::new(), Vec::new(), Vec::new(), Vec::new())
    } else {
        let (endpoints_result, keys_result, model_stats_result, active_global_model_refs_result) = tokio::join!(
            state.list_provider_catalog_endpoints_by_provider_ids(&provider_ids),
            state.list_provider_catalog_key_summaries_by_provider_ids(&provider_ids),
            state.list_provider_model_stats(&provider_ids),
            state.list_active_global_model_ids_by_provider_ids(&provider_ids),
        );
        (
            endpoints_result.ok().unwrap_or_default(),
            keys_result.ok().unwrap_or_default(),
            model_stats_result.ok().unwrap_or_default(),
            active_global_model_refs_result.ok().unwrap_or_default(),
        )
    };
    let mut endpoints_by_provider = BTreeMap::<String, Vec<StoredProviderCatalogEndpoint>>::new();
    for endpoint in endpoints {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }
    let mut keys_by_provider = BTreeMap::<String, Vec<StoredProviderCatalogKey>>::new();
    for key in keys {
        keys_by_provider
            .entry(key.provider_id.clone())
            .or_default()
            .push(key);
    }
    let model_stats_by_provider = model_stats
        .into_iter()
        .map(|stats| (stats.provider_id.clone(), stats))
        .collect::<BTreeMap<_, _>>();
    let mut active_global_model_ids_by_provider = BTreeMap::<String, BTreeSet<String>>::new();
    for row in page_active_global_model_refs {
        active_global_model_ids_by_provider
            .entry(row.provider_id)
            .or_default()
            .insert(row.global_model_id);
    }
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut items = Vec::with_capacity(providers.len());
    for provider in providers {
        let active_global_model_ids = active_global_model_ids_by_provider
            .get(&provider.id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        items.push(build_admin_provider_summary_value(
            &provider,
            endpoints_by_provider
                .get(&provider.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            keys_by_provider
                .get(&provider.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            None,
            model_stats_by_provider.get(&provider.id),
            active_global_model_ids,
            now_unix_secs,
        ));
    }

    Some(json!({
        "total": total,
        "page": page,
        "page_size": page_size,
        "items": items,
    }))
}
