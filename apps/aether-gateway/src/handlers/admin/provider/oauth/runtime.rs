use super::quota::dispatch::refresh_provider_pool_quota_locally;
use super::quota::shared::provider_quota_refresh_endpoint_for_provider;
use super::quota::shared::provider_type_supports_quota_refresh;
use crate::handlers::admin::provider::write::provider::reconcile_admin_fixed_provider_template_endpoints;
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::task_runtime::{spawn_fire_and_forget, TASK_KEY_PROVIDER_OAUTH_ACCOUNT_REFRESH};
use crate::{AppState, GatewayError};
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};

pub(crate) fn provider_oauth_runtime_endpoint_for_provider(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Option<StoredProviderCatalogEndpoint> {
    select_provider_oauth_runtime_endpoint(provider_type, endpoints, false)
}

pub(crate) fn provider_oauth_maintenance_endpoint_for_provider(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Option<StoredProviderCatalogEndpoint> {
    select_provider_oauth_runtime_endpoint(provider_type, endpoints, true)
}

fn matching_endpoint<F>(
    endpoints: &[StoredProviderCatalogEndpoint],
    include_inactive: bool,
    predicate: F,
) -> Option<StoredProviderCatalogEndpoint>
where
    F: Fn(&StoredProviderCatalogEndpoint) -> bool,
{
    endpoints
        .iter()
        .find(|endpoint| endpoint.is_active && predicate(endpoint))
        .cloned()
        .or_else(|| {
            include_inactive.then(|| {
                endpoints
                    .iter()
                    .find(|endpoint| !endpoint.is_active && predicate(endpoint))
                    .cloned()
            })?
        })
}

fn select_provider_oauth_runtime_endpoint(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
    include_inactive: bool,
) -> Option<StoredProviderCatalogEndpoint> {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    match provider_type.as_str() {
        "codex" | "grok_oauth" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            crate::ai_serving::is_openai_responses_format(&endpoint.api_format)
        }),
        "chatgpt_web" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            endpoint
                .api_format
                .trim()
                .eq_ignore_ascii_case("openai:image")
        }),
        "grok" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            endpoint
                .api_format
                .trim()
                .eq_ignore_ascii_case("openai:chat")
        })
        .or_else(|| matching_endpoint(endpoints, include_inactive, |_| true)),
        "antigravity" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            endpoint
                .api_format
                .trim()
                .eq_ignore_ascii_case("gemini:generate_content")
        }),
        "kiro" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            endpoint
                .api_format
                .trim()
                .eq_ignore_ascii_case("claude:messages")
        })
        .or_else(|| matching_endpoint(endpoints, include_inactive, |_| true)),
        "claude_code" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            endpoint
                .api_format
                .trim()
                .eq_ignore_ascii_case("claude:messages")
        }),
        "gemini_cli" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            endpoint
                .api_format
                .trim()
                .eq_ignore_ascii_case("gemini:generate_content")
        }),
        "vertex_ai" => matching_endpoint(endpoints, include_inactive, |endpoint| {
            endpoint
                .api_format
                .trim()
                .eq_ignore_ascii_case("gemini:generate_content")
        })
        .or_else(|| {
            matching_endpoint(endpoints, include_inactive, |endpoint| {
                endpoint
                    .api_format
                    .trim()
                    .eq_ignore_ascii_case("claude:messages")
            })
        }),
        _ => matching_endpoint(endpoints, include_inactive, |_| true),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderOAuthRuntimeEndpoints {
    pub(crate) endpoints: Vec<StoredProviderCatalogEndpoint>,
    pub(crate) runtime_endpoint: Option<StoredProviderCatalogEndpoint>,
}

async fn resolve_provider_runtime_endpoints_with_selector(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    provider_type: &str,
    endpoint_selector: fn(
        &str,
        &[StoredProviderCatalogEndpoint],
        bool,
    ) -> Option<StoredProviderCatalogEndpoint>,
) -> Result<ProviderOAuthRuntimeEndpoints, GatewayError> {
    let mut endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let mut runtime_endpoint = endpoint_selector(provider_type, &endpoints, true);
    if runtime_endpoint.is_none()
        && state
            .fixed_provider_template(&provider.provider_type)
            .is_some()
        && state.has_provider_catalog_data_writer()
    {
        reconcile_admin_fixed_provider_template_endpoints(state, provider).await?;
        endpoints = state
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?;
        runtime_endpoint = endpoint_selector(provider_type, &endpoints, true);
    }

    Ok(ProviderOAuthRuntimeEndpoints {
        endpoints,
        runtime_endpoint,
    })
}

pub(crate) async fn resolve_provider_oauth_runtime_endpoints(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    provider_type: &str,
) -> Result<ProviderOAuthRuntimeEndpoints, GatewayError> {
    resolve_provider_runtime_endpoints_with_selector(
        state,
        provider,
        provider_type,
        select_provider_oauth_runtime_endpoint,
    )
    .await
}

async fn resolve_provider_quota_runtime_endpoints(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    provider_type: &str,
) -> Result<ProviderOAuthRuntimeEndpoints, GatewayError> {
    resolve_provider_runtime_endpoints_with_selector(
        state,
        provider,
        provider_type,
        provider_quota_refresh_endpoint_for_provider,
    )
    .await
}

pub(crate) async fn refresh_provider_oauth_account_state_after_update(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    key_id: &str,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<(bool, Option<String>), GatewayError> {
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !provider_type_supports_quota_refresh(&provider_type) {
        return Ok((false, None));
    }

    let ProviderOAuthRuntimeEndpoints {
        runtime_endpoint, ..
    } = resolve_provider_quota_runtime_endpoints(state, provider, &provider_type).await?;
    let Some(endpoint) = runtime_endpoint else {
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

    let payload = refresh_provider_pool_quota_locally(
        state,
        provider,
        &endpoint,
        &provider_type,
        vec![key],
        proxy_override.cloned(),
    )
    .await?;
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
    spawn_fire_and_forget(TASK_KEY_PROVIDER_OAUTH_ACCOUNT_REFRESH, async move {
        let _ = refresh_provider_oauth_account_state_after_update(
            &AdminAppState::new(&app),
            &provider,
            &key_id,
            proxy_override.as_ref(),
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::provider_oauth_runtime_endpoint_for_provider;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;

    fn endpoint(id: &str, api_format: &str) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            id.to_string(),
            "provider-grok-oauth".to_string(),
            api_format.to_string(),
            Some("openai".to_string()),
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://cli-chat-proxy.grok.com/v1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    #[test]
    fn grok_oauth_runtime_uses_responses_endpoint_even_if_legacy_chat_is_first() {
        let endpoints = vec![
            endpoint("legacy-chat", "openai:chat"),
            endpoint("responses", "openai:responses"),
        ];

        assert_eq!(
            provider_oauth_runtime_endpoint_for_provider("grok_oauth", &endpoints)
                .expect("Responses endpoint should be selected")
                .id,
            "responses"
        );
    }
}
