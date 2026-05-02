use super::quota::antigravity::refresh_antigravity_provider_quota_locally;
use super::quota::codex::refresh_codex_provider_quota_locally;
use super::quota::kiro::refresh_kiro_provider_quota_locally;
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::{AppState, GatewayError};
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};

pub(crate) fn provider_oauth_runtime_endpoint_for_provider(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Option<StoredProviderCatalogEndpoint> {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    match provider_type.as_str() {
        "codex" => endpoints
            .iter()
            .find(|endpoint| {
                endpoint.is_active
                    && crate::ai_serving::is_openai_responses_format(&endpoint.api_format)
            })
            .cloned(),
        "antigravity" => endpoints
            .iter()
            .find(|endpoint| {
                endpoint.is_active
                    && endpoint
                        .api_format
                        .trim()
                        .eq_ignore_ascii_case("gemini:generate_content")
            })
            .cloned(),
        "kiro" => endpoints
            .iter()
            .find(|endpoint| {
                endpoint.is_active
                    && endpoint
                        .api_format
                        .trim()
                        .eq_ignore_ascii_case("claude:messages")
            })
            .cloned()
            .or_else(|| {
                endpoints
                    .iter()
                    .find(|endpoint| endpoint.is_active)
                    .cloned()
            }),
        _ => endpoints
            .iter()
            .find(|endpoint| endpoint.is_active)
            .cloned(),
    }
}

pub(crate) async fn refresh_provider_oauth_account_state_after_update(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    key_id: &str,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<(bool, Option<String>), GatewayError> {
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !matches!(provider_type.as_str(), "codex" | "kiro" | "antigravity") {
        return Ok((false, None));
    }

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let Some(endpoint) = provider_oauth_runtime_endpoint_for_provider(&provider_type, &endpoints)
    else {
        return Ok((false, None));
    };
    let Some(key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok((false, None));
    };
    if provider_type == "kiro" && !provider_key_is_oauth_managed(&key, provider_type.as_str()) {
        return Ok((false, None));
    }

    let proxy_override = proxy_override.cloned();
    let payload = match provider_type.as_str() {
        "codex" => {
            refresh_codex_provider_quota_locally(
                state,
                provider,
                &endpoint,
                vec![key],
                proxy_override.clone(),
            )
            .await?
        }
        "kiro" => {
            refresh_kiro_provider_quota_locally(
                state,
                provider,
                &endpoint,
                vec![key],
                proxy_override.clone(),
            )
            .await?
        }
        "antigravity" => {
            refresh_antigravity_provider_quota_locally(
                state,
                provider,
                &endpoint,
                vec![key],
                proxy_override,
            )
            .await?
        }
        _ => None,
    };
    let Some(payload) = payload else {
        return Ok((false, None));
    };
    let success = payload
        .get("success")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let error = if success == 0 {
        payload
            .get("results")
            .and_then(serde_json::Value::as_array)
            .and_then(|results| results.first())
            .and_then(|value| value.get("message"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    } else {
        None
    };
    Ok((true, error))
}

pub(crate) fn spawn_provider_oauth_account_state_refresh_after_update(
    app: AppState,
    provider: StoredProviderCatalogProvider,
    key_id: String,
    proxy_override: Option<ProxySnapshot>,
) {
    tokio::spawn(async move {
        let _ = refresh_provider_oauth_account_state_after_update(
            &AdminAppState::new(&app),
            &provider,
            &key_id,
            proxy_override.as_ref(),
        )
        .await;
    });
}
