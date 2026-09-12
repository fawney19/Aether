use std::time::Duration;

use tracing::warn;

use crate::admin_api::{reconcile_admin_fixed_provider_template_endpoints, AdminAppState};
use crate::task_runtime::{
    spawn_fire_and_forget, task_definition, TASK_KEY_FIXED_PROVIDER_RECONCILIATION,
};
use crate::{AppState, GatewayError};

const FIXED_PROVIDER_RECONCILIATION_LOCK_KEY: &str =
    "task_runtime:lock:maintenance.provider.fixed_template.reconcile";
const FIXED_PROVIDER_RECONCILIATION_LOCK_TTL: Duration = Duration::from_secs(10 * 60);
const FIXED_PROVIDER_RECONCILIATION_RETRY_DELAY: Duration = Duration::from_secs(2);

pub(crate) async fn perform_fixed_provider_reconciliation_once(
    state: &AppState,
) -> Result<bool, GatewayError> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(false);
    }

    let Some(lock) = state
        .runtime_state
        .lock_try_acquire(
            FIXED_PROVIDER_RECONCILIATION_LOCK_KEY,
            state.tunnel.local_instance_id(),
            FIXED_PROVIDER_RECONCILIATION_LOCK_TTL,
        )
        .await
        .map_err(|error| GatewayError::Internal(error.to_string()))?
    else {
        return Ok(false);
    };

    let result = reconcile_fixed_provider_templates(state).await;
    if let Err(error) = state.runtime_state.lock_release(&lock).await {
        warn!(
            event_name = "fixed_provider_reconciliation_lock_release_failed",
            log_type = "ops",
            error = ?error,
            "gateway fixed provider reconciliation lock release failed"
        );
    }
    result.map(|()| true)
}

async fn reconcile_fixed_provider_templates(state: &AppState) -> Result<(), GatewayError> {
    let providers = state.list_provider_catalog_providers(false).await?;
    let admin_state = AdminAppState::new(state);
    let mut failures = Vec::new();
    for provider in &providers {
        if admin_state
            .fixed_provider_template(&provider.provider_type)
            .is_none()
        {
            continue;
        }
        if let Err(error) =
            reconcile_admin_fixed_provider_template_endpoints(&admin_state, provider).await
        {
            failures.push(format!(
                "provider {} endpoint reconciliation failed: {error:?}",
                provider.id,
            ));
            continue;
        }
    }
    if !failures.is_empty() {
        return Err(GatewayError::Internal(failures.join("; ")));
    }
    Ok(())
}

async fn perform_fixed_provider_reconciliation_with_retry(state: &AppState) {
    let max_attempts = task_definition(TASK_KEY_FIXED_PROVIDER_RECONCILIATION)
        .map(|definition| definition.retry_policy.max_attempts)
        .unwrap_or(1)
        .max(1);
    for attempt in 1..=max_attempts {
        match perform_fixed_provider_reconciliation_once(state).await {
            Ok(_) => return,
            Err(error) if attempt < max_attempts => {
                warn!(
                    event_name = "fixed_provider_reconciliation_retrying",
                    log_type = "ops",
                    attempt,
                    max_attempts,
                    error = ?error,
                    "gateway fixed provider reconciliation will retry"
                );
                tokio::time::sleep(FIXED_PROVIDER_RECONCILIATION_RETRY_DELAY).await;
            }
            Err(error) => {
                warn!(
                    event_name = "fixed_provider_reconciliation_failed",
                    log_type = "ops",
                    attempt,
                    max_attempts,
                    error = ?error,
                    "gateway fixed provider reconciliation failed"
                );
                return;
            }
        }
    }
}

pub(crate) fn spawn_fixed_provider_reconciliation_task(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return None;
    }

    Some(spawn_fire_and_forget(
        TASK_KEY_FIXED_PROVIDER_RECONCILIATION,
        async move {
            perform_fixed_provider_reconciliation_with_retry(&state).await;
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use serde_json::json;

    use super::{
        perform_fixed_provider_reconciliation_once, FIXED_PROVIDER_RECONCILIATION_LOCK_KEY,
        FIXED_PROVIDER_RECONCILIATION_LOCK_TTL,
    };
    use crate::data::GatewayDataState;
    use crate::AppState;

    #[tokio::test]
    async fn fixed_provider_reconciliation_respects_runtime_singleton_lock() {
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![],
            vec![],
            vec![],
        ));
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository),
            );
        let lock = state
            .runtime_state
            .lock_try_acquire(
                FIXED_PROVIDER_RECONCILIATION_LOCK_KEY,
                "another-gateway",
                FIXED_PROVIDER_RECONCILIATION_LOCK_TTL,
            )
            .await
            .expect("runtime lock should be available")
            .expect("runtime lock should be acquired");

        assert!(!perform_fixed_provider_reconciliation_once(&state)
            .await
            .expect("locked reconciliation should skip"));

        assert!(state
            .runtime_state
            .lock_release(&lock)
            .await
            .expect("runtime lock should release"));
        assert!(perform_fixed_provider_reconciliation_once(&state)
            .await
            .expect("unlocked reconciliation should run"));
    }

    #[tokio::test]
    async fn fixed_provider_reconciliation_preserves_existing_endpoint_and_is_idempotent() {
        let mut provider = StoredProviderCatalogProvider::new(
            "provider-codex".to_string(),
            "Codex".to_string(),
            None,
            "codex".to_string(),
        )
        .expect("provider should build");
        provider.is_active = false;
        provider.max_retries = Some(2);

        let mut responses = StoredProviderCatalogEndpoint::new(
            "endpoint-codex-responses".to_string(),
            provider.id.clone(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("cli".to_string()),
            false,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "http://127.0.0.1:18181/backend-api/codex".to_string(),
            None,
            None,
            Some(9),
            None,
            Some(json!({"upstream_stream_policy": "force_non_stream"})),
            None,
            Some(json!({"url": "http://proxy.internal:8080"})),
        )
        .expect("endpoint transport should build");
        responses.updated_at_unix_secs = Some(100);

        let mut live = StoredProviderCatalogEndpoint::new(
            "endpoint-codex-live".to_string(),
            provider.id.clone(),
            "codex:live".to_string(),
            Some("codex".to_string()),
            Some("live".to_string()),
            false,
        )
        .expect("Live endpoint should build")
        .with_transport_fields(
            "https://voice-proxy.internal/v1".to_string(),
            None,
            None,
            Some(6),
            Some("/socket/live".to_string()),
            Some(json!({"custom_transport_option": true})),
            None,
            Some(json!({"url": "http://voice-proxy.internal:8080"})),
        )
        .expect("Live endpoint transport should build");
        live.updated_at_unix_secs = Some(100);

        let mut key = StoredProviderCatalogKey::new(
            "key-codex".to_string(),
            provider.id.clone(),
            "oauth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.api_formats = Some(json!(["openai:responses"]));

        let unrelated_provider = StoredProviderCatalogProvider::new(
            "provider-custom".to_string(),
            "Custom".to_string(),
            None,
            "custom".to_string(),
        )
        .expect("unrelated provider should build");
        let fresh_codex_provider = StoredProviderCatalogProvider::new(
            "provider-codex-fresh".to_string(),
            "Fresh Codex".to_string(),
            None,
            "codex".to_string(),
        )
        .expect("fresh Codex provider should build");

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider, fresh_codex_provider, unrelated_provider],
            vec![responses, live],
            vec![key],
        ));
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository.clone()),
            );

        assert!(perform_fixed_provider_reconciliation_once(&state)
            .await
            .expect("reconciliation should run"));
        let first_endpoints = repository
            .list_endpoints_by_provider_ids(&["provider-codex".to_string()])
            .await
            .expect("endpoints should list");
        assert_eq!(first_endpoints.len(), 5);
        let responses = first_endpoints
            .iter()
            .find(|endpoint| endpoint.api_format == "openai:responses")
            .expect("responses endpoint should exist");
        assert_eq!(
            responses.base_url,
            "http://127.0.0.1:18181/backend-api/codex"
        );
        assert!(!responses.is_active);
        assert_eq!(responses.max_retries, Some(9));
        assert_eq!(
            responses.proxy,
            Some(json!({"url": "http://proxy.internal:8080/"}))
        );
        assert_eq!(
            responses
                .config
                .as_ref()
                .and_then(|value| value.get("upstream_stream_policy")),
            Some(&json!("force_non_stream"))
        );
        assert!(first_endpoints
            .iter()
            .any(|endpoint| endpoint.api_format == "openai:search"));
        let live = first_endpoints
            .iter()
            .find(|endpoint| endpoint.api_format == "codex:live")
            .expect("Codex Live endpoint should be reconciled from the v2 template");
        assert_eq!(live.api_family.as_deref(), Some("codex"));
        assert_eq!(live.endpoint_kind.as_deref(), Some("live"));
        assert_eq!(live.base_url, "https://voice-proxy.internal/v1");
        assert_eq!(live.custom_path.as_deref(), Some("/socket/live"));
        assert_eq!(live.max_retries, Some(6));
        assert_eq!(
            live.proxy,
            Some(json!({"url": "http://voice-proxy.internal:8080/"}))
        );
        assert_eq!(
            live.config
                .as_ref()
                .and_then(|value| value.get("custom_transport_option")),
            Some(&json!(true))
        );
        assert!(!live.is_active);
        let keys = repository
            .list_keys_by_provider_ids(&["provider-codex".to_string()])
            .await
            .expect("keys should list");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].api_formats, Some(json!(["openai:responses"])));
        assert!(repository
            .list_endpoints_by_provider_ids(&["provider-custom".to_string()])
            .await
            .expect("unrelated endpoints should list")
            .is_empty());
        let fresh_endpoints = repository
            .list_endpoints_by_provider_ids(&["provider-codex-fresh".to_string()])
            .await
            .expect("fresh Codex endpoints should list");
        assert_eq!(fresh_endpoints.len(), 5);
        let fresh_live = fresh_endpoints
            .iter()
            .find(|endpoint| endpoint.api_format == "codex:live")
            .expect("v2 reconciliation should add a Codex Live endpoint");
        assert_eq!(fresh_live.base_url, "https://chatgpt.com/backend-api/codex");
        assert!(fresh_live.is_active);

        assert!(perform_fixed_provider_reconciliation_once(&state)
            .await
            .expect("second reconciliation should run"));
        let second_endpoints = repository
            .list_endpoints_by_provider_ids(&["provider-codex".to_string()])
            .await
            .expect("endpoints should list again");
        assert_eq!(second_endpoints, first_endpoints);
        let second_fresh_endpoints = repository
            .list_endpoints_by_provider_ids(&["provider-codex-fresh".to_string()])
            .await
            .expect("fresh Codex endpoints should list again");
        assert_eq!(second_fresh_endpoints, fresh_endpoints);
    }

    #[tokio::test]
    async fn fixed_provider_reconciliation_retires_removed_vertex_claude_endpoint() {
        let provider = StoredProviderCatalogProvider::new(
            "provider-vertex".to_string(),
            "Vertex AI".to_string(),
            None,
            "vertex_ai".to_string(),
        )
        .expect("provider should build");

        let gemini = StoredProviderCatalogEndpoint::new(
            "endpoint-vertex-gemini".to_string(),
            provider.id.clone(),
            "gemini:generate_content".to_string(),
            Some("gemini".to_string()),
            Some("generate_content".to_string()),
            true,
        )
        .expect("gemini endpoint should build")
        .with_transport_fields(
            "https://aiplatform.googleapis.com".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("gemini endpoint transport should build");

        let claude = StoredProviderCatalogEndpoint::new(
            "endpoint-vertex-claude".to_string(),
            provider.id.clone(),
            "claude:messages".to_string(),
            Some("claude".to_string()),
            Some("messages".to_string()),
            true,
        )
        .expect("claude endpoint should build")
        .with_transport_fields(
            "https://aiplatform.googleapis.com".to_string(),
            None,
            None,
            Some(2),
            None,
            Some(json!({
                "_aether_fixed_provider_template": {
                    "managed": true,
                    "provider_type": "vertex_ai",
                    "item_key": "claude:messages",
                    "version": 1,
                    "retired": false,
                    "overrides": [],
                    "config_keys": []
                }
            })),
            None,
            None,
        )
        .expect("claude endpoint transport should build");

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![gemini, claude],
            vec![],
        ));
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository.clone()),
            );

        assert!(perform_fixed_provider_reconciliation_once(&state)
            .await
            .expect("reconciliation should run"));

        let endpoints = repository
            .list_endpoints_by_provider_ids(&["provider-vertex".to_string()])
            .await
            .expect("vertex endpoints should list");
        let gemini = endpoints
            .iter()
            .find(|endpoint| endpoint.id == "endpoint-vertex-gemini")
            .expect("gemini endpoint should remain");
        assert!(gemini.is_active);

        let claude = endpoints
            .iter()
            .find(|endpoint| endpoint.id == "endpoint-vertex-claude")
            .expect("legacy claude endpoint should remain as retired history");
        assert!(!claude.is_active);
        assert_eq!(
            claude
                .config
                .as_ref()
                .and_then(|value| value.get("_aether_fixed_provider_template"))
                .and_then(|value| value.get("retired")),
            Some(&json!(true))
        );
    }
}
