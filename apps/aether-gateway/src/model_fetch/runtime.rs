use std::collections::{BTreeSet, HashMap};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogUpstreamMetadataNamespaceUpdate, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_model_fetch::{
    apply_model_filters, fetch_models_from_transports_for_management, json_string_list,
    model_catalog_upstream_metadata, model_fetch_interval_minutes,
    model_fetch_startup_delay_seconds, model_fetch_startup_enabled, preset_models_for_provider,
    selected_models_fetch_endpoints, sync_provider_model_whitelist_associations,
    upstream_metadata_namespace_updates, ModelFetchAssociationStore, ModelFetchRunSummary,
};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::{AppState, GatewayError};

pub(crate) mod state;

use self::state::ModelFetchRuntimeState;

#[derive(Debug, Clone)]
struct SelectedFetchTarget {
    provider: StoredProviderCatalogProvider,
    key: StoredProviderCatalogKey,
    endpoints: Vec<StoredProviderCatalogEndpoint>,
}

pub(crate) fn spawn_model_fetch_worker(state: AppState) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return None;
    }

    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_MODEL_FETCH_WORKER,
        |state| async move {
            if model_fetch_startup_enabled() {
                let startup_delay = model_fetch_startup_delay_seconds();
                if startup_delay > 0 {
                    tokio::time::sleep(Duration::from_secs(startup_delay)).await;
                }
                if let Err(err) = run_model_fetch_cycle(&state, "startup").await {
                    warn!(
                        error = %safe_model_fetch_error(&err.clone().into_message()),
                        "gateway model fetch startup failed"
                    );
                }
            } else {
                info!("gateway model fetch startup disabled");
            }

            let mut interval = tokio::time::interval(Duration::from_secs(
                model_fetch_interval_minutes().saturating_mul(60),
            ));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(err) = run_model_fetch_cycle(&state, "tick").await {
                    warn!(
                        error = %safe_model_fetch_error(&err.clone().into_message()),
                        "gateway model fetch tick failed"
                    );
                }
            }
        },
    ))
}

pub(crate) async fn perform_model_fetch_once(
    state: &AppState,
) -> Result<ModelFetchRunSummary, GatewayError> {
    perform_model_fetch_once_with_state(state).await
}

pub(crate) async fn perform_model_fetch_for_key(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
) -> Result<ModelFetchRunSummary, GatewayError> {
    let key_ids = BTreeSet::from([key_id.to_string()]);
    perform_model_fetch_for_keys_with_state(state, provider_id, &key_ids).await
}

pub(crate) async fn perform_model_fetch_for_keys(
    state: &AppState,
    provider_id: &str,
    key_ids: &BTreeSet<String>,
) -> Result<ModelFetchRunSummary, GatewayError> {
    perform_model_fetch_for_keys_with_state(state, provider_id, key_ids).await
}

async fn perform_model_fetch_once_with_state<S>(
    state: &S,
) -> Result<ModelFetchRunSummary, GatewayError>
where
    S: ModelFetchRuntimeState + ?Sized,
{
    let targets = collect_fetch_targets(state, None, None).await?;
    execute_fetch_targets(state, targets).await
}

async fn perform_model_fetch_for_keys_with_state<S>(
    state: &S,
    provider_id: &str,
    key_ids: &BTreeSet<String>,
) -> Result<ModelFetchRunSummary, GatewayError>
where
    S: ModelFetchRuntimeState + ?Sized,
{
    let targets = collect_fetch_targets(state, Some(provider_id), Some(key_ids)).await?;
    execute_fetch_targets(state, targets).await
}

async fn collect_fetch_targets<S>(
    state: &S,
    provider_id_filter: Option<&str>,
    key_id_filter: Option<&BTreeSet<String>>,
) -> Result<Vec<SelectedFetchTarget>, GatewayError>
where
    S: ModelFetchRuntimeState + ?Sized,
{
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(Vec::new());
    }

    let providers = state
        .list_provider_catalog_providers_for_model_fetch(true)
        .await?
        .into_iter()
        .filter(|provider| provider_id_filter.is_none_or(|provider_id| provider.id == provider_id))
        .collect::<Vec<_>>();
    if providers.is_empty() {
        return Ok(Vec::new());
    }

    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let mut endpoints_by_provider = HashMap::<String, Vec<StoredProviderCatalogEndpoint>>::new();
    for endpoint in state
        .list_provider_catalog_endpoints_for_model_fetch(&provider_ids)
        .await?
    {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }
    let mut keys_by_provider = HashMap::<String, Vec<StoredProviderCatalogKey>>::new();
    for key in state
        .list_provider_catalog_keys_for_model_fetch(&provider_ids)
        .await
        .map_err(GatewayError::Internal)?
    {
        keys_by_provider
            .entry(key.provider_id.clone())
            .or_default()
            .push(key);
    }

    let mut targets = Vec::new();
    for provider in providers {
        let endpoints = endpoints_by_provider
            .remove(&provider.id)
            .unwrap_or_default()
            .into_iter()
            .map(sanitize_model_fetch_endpoint)
            .collect::<Vec<_>>();
        let keys = keys_by_provider.remove(&provider.id).unwrap_or_default();
        let provider = sanitize_model_fetch_provider(provider);
        for key in keys {
            if key_id_filter.is_some_and(|key_ids| !key_ids.contains(&key.id)) {
                continue;
            }
            if !key.is_active || !key.auto_fetch_models {
                continue;
            }
            let selected_endpoints = selected_models_fetch_endpoints(&endpoints, &key);
            let key = sanitize_model_fetch_key(key);
            targets.push(SelectedFetchTarget {
                provider: provider.clone(),
                key,
                endpoints: selected_endpoints,
            });
        }
    }
    Ok(targets)
}

/// Keep only the key metadata needed after target collection. Raw catalog
/// rows contain encrypted credentials and transport secrets; model discovery
/// reopens a single snapshot by id when it actually needs to make a request.
fn sanitize_model_fetch_key(mut key: StoredProviderCatalogKey) -> StoredProviderCatalogKey {
    // `SelectedFetchTarget` lives across endpoint selection and the complete
    // fetch/persist operation.  Keep only fields consumed by that operation;
    // in particular, do not retain historical diagnostics, scheduling state,
    // usage counters, or transport configuration copied from a raw database
    // row.  The actual credential/proxy snapshot is reopened by id for one
    // endpoint at a time.
    key.capabilities = None;
    key.auth_type_by_format = None;
    key.allow_auth_channel_mismatch_formats = None;
    key.encrypted_api_key = None;
    key.encrypted_auth_config = None;
    key.note = None;
    key.internal_priority = 0;
    key.rate_multipliers = None;
    key.global_priority_by_format = None;
    key.expires_at_unix_secs = None;
    key.cache_ttl_minutes = 0;
    key.max_probe_interval_minutes = 0;
    key.proxy = None;
    key.fingerprint = None;
    key.rpm_limit = None;
    key.concurrent_limit = None;
    key.learned_rpm_limit = None;
    key.concurrent_429_count = None;
    key.rpm_429_count = None;
    key.last_429_at_unix_secs = None;
    key.last_429_type = None;
    key.adjustment_history = None;
    key.utilization_samples = None;
    key.last_probe_increase_at_unix_secs = None;
    key.last_rpm_peak = None;
    key.request_count = None;
    key.total_tokens = 0;
    key.total_cost_usd = 0.0;
    key.success_count = None;
    key.error_count = None;
    key.total_response_time_ms = None;
    key.last_used_at_unix_secs = None;
    key.last_models_fetch_at_unix_secs = None;
    key.last_models_fetch_error = None;
    key.oauth_invalid_at_unix_secs = None;
    key.oauth_invalid_reason = None;
    key.status_snapshot = None;
    key
}

/// Keep only non-secret provider metadata in a background fetch target.  The
/// authoritative transport snapshot is reopened by ID immediately before a
/// request, so carrying stored proxy/config JSON here would needlessly retain
/// credentials and could expose malformed historical values to later stages.
fn sanitize_model_fetch_provider(
    mut provider: StoredProviderCatalogProvider,
) -> StoredProviderCatalogProvider {
    provider.proxy = None;
    provider.config = None;
    provider
}

/// Endpoint selection needs only activity, format, and identity.  Clear
/// transport rules/proxy data because those are reloaded from the snapshot
/// just before execution.
fn sanitize_model_fetch_endpoint(
    mut endpoint: StoredProviderCatalogEndpoint,
) -> StoredProviderCatalogEndpoint {
    endpoint.header_rules = None;
    endpoint.body_rules = None;
    endpoint.config = None;
    endpoint.format_acceptance_config = None;
    endpoint.proxy = None;
    endpoint
}

async fn execute_fetch_targets<S>(
    state: &S,
    targets: Vec<SelectedFetchTarget>,
) -> Result<ModelFetchRunSummary, GatewayError>
where
    S: ModelFetchRuntimeState + ?Sized,
{
    let mut summary = ModelFetchRunSummary {
        attempted: targets.len(),
        succeeded: 0,
        failed: 0,
        skipped: 0,
    };
    for target in targets {
        match fetch_and_persist_key_models(state, &target).await? {
            KeyFetchDisposition::Succeeded => summary.succeeded += 1,
            KeyFetchDisposition::Failed => summary.failed += 1,
            KeyFetchDisposition::Skipped => summary.skipped += 1,
        }
    }
    Ok(summary)
}

async fn run_model_fetch_cycle<S>(state: &S, phase: &'static str) -> Result<(), GatewayError>
where
    S: ModelFetchRuntimeState + ?Sized,
{
    let summary = perform_model_fetch_once_with_state(state).await?;
    if summary.attempted == 0 {
        debug!(phase, "gateway model fetch found no eligible keys");
        return Ok(());
    }

    info!(
        phase,
        attempted = summary.attempted,
        succeeded = summary.succeeded,
        failed = summary.failed,
        skipped = summary.skipped,
        "gateway model fetch cycle completed"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyFetchDisposition {
    Succeeded,
    Failed,
    Skipped,
}

async fn fetch_and_persist_key_models(
    state: &(impl ModelFetchRuntimeState + ?Sized),
    target: &SelectedFetchTarget,
) -> Result<KeyFetchDisposition, GatewayError> {
    let now_unix_secs = now_unix_secs();
    if target.endpoints.is_empty() {
        if let Some(models) = preset_models_for_provider(&target.provider.provider_type) {
            let fetched_model_ids = models
                .iter()
                .filter_map(|model| model.get("id"))
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let filtered_models = apply_model_filters(
                &fetched_model_ids,
                json_string_list(target.key.locked_models.as_ref()),
                json_string_list(target.key.model_include_patterns.as_ref()),
                json_string_list(target.key.model_exclude_patterns.as_ref()),
            );
            let upstream_metadata =
                model_catalog_upstream_metadata(&target.provider.provider_type, &models);
            persist_key_fetch_success(
                state,
                &target.key,
                now_unix_secs,
                &filtered_models,
                upstream_metadata.as_ref(),
            )
            .await?;
            state
                .write_upstream_models_cache(&target.provider.id, &target.key.id, &models)
                .await;
            sync_provider_model_whitelist_associations(
                state,
                &target.provider.id,
                &filtered_models,
            )
            .await
            .map_err(GatewayError::Internal)?;
            return Ok(KeyFetchDisposition::Succeeded);
        }
        persist_key_fetch_failure(
            state,
            &target.key,
            now_unix_secs,
            "No supported endpoint for Rust models fetch".to_string(),
        )
        .await?;
        return Ok(KeyFetchDisposition::Skipped);
    }

    let mut transports = Vec::new();
    let mut skipped_invalid_credential = false;
    for endpoint in &target.endpoints {
        match state
            .read_provider_transport_snapshot(&target.provider.id, &endpoint.id, &target.key.id)
            .await
        {
            Ok(Some(transport)) => transports.push(transport),
            Ok(None) => {
                warn!(
                    provider_id = %target.provider.id,
                    endpoint_id = %endpoint.id,
                    key_id = %target.key.id,
                    "gateway model fetch transport snapshot unavailable"
                );
            }
            Err(error) if is_nonfatal_legacy_credential_error(&error) => {
                skipped_invalid_credential = true;
                warn!(
                    event_name = "model_fetch_skipped_invalid_credential",
                    log_type = "ops",
                    provider_id = %target.provider.id,
                    endpoint_id = %endpoint.id,
                    key_id = %target.key.id,
                    reason = "invalid_stored_credential",
                    "gateway skipped model fetch for an invalid stored credential"
                );
            }
            Err(error) => return Err(error),
        }
    }

    // A malformed legacy credential is isolated to its key. Do not turn it
    // into a cycle-wide failure (or rewrite the row merely to record a fetch
    // error), and let other eligible keys continue through the worker.
    if transports.is_empty() && skipped_invalid_credential {
        return Ok(KeyFetchDisposition::Skipped);
    }

    if transports.is_empty() {
        persist_key_fetch_failure(
            state,
            &target.key,
            now_unix_secs,
            "Provider transport snapshot unavailable".to_string(),
        )
        .await?;
        return Ok(KeyFetchDisposition::Skipped);
    }

    let codex_client_version = if target
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex")
    {
        state
            .read_recent_codex_catalog_client_version(&target.provider.id, &target.key.id)
            .await
    } else {
        None
    };
    let result = match fetch_models_from_transports_for_management(
        state,
        &transports,
        codex_client_version.as_deref(),
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            let safe_error = safe_model_fetch_error(&err);
            persist_key_fetch_failure(state, &target.key, now_unix_secs, safe_error.clone())
                .await?;
            warn!(
                provider_id = %target.provider.id,
                key_id = %target.key.id,
                error = %safe_error,
                "gateway model fetch failed"
            );
            return Ok(KeyFetchDisposition::Failed);
        }
    };

    if !result.has_success {
        let error = if result.errors.is_empty() {
            "Upstream models fetch failed".to_string()
        } else {
            result.errors.join("; ")
        };
        let safe_error = safe_model_fetch_error(&error);
        persist_key_fetch_failure(state, &target.key, now_unix_secs, safe_error.clone()).await?;
        warn!(
            provider_id = %target.provider.id,
            key_id = %target.key.id,
            error = %safe_error,
            "gateway model fetch failed"
        );
        return Ok(KeyFetchDisposition::Failed);
    }

    let filtered_models = apply_model_filters(
        &result.fetched_model_ids,
        json_string_list(target.key.locked_models.as_ref()),
        json_string_list(target.key.model_include_patterns.as_ref()),
        json_string_list(target.key.model_exclude_patterns.as_ref()),
    );
    persist_key_fetch_success(
        state,
        &target.key,
        now_unix_secs,
        &filtered_models,
        result.upstream_metadata.as_ref(),
    )
    .await?;
    state
        .write_upstream_models_cache(&target.provider.id, &target.key.id, &result.legacy_models)
        .await;
    sync_provider_model_whitelist_associations(state, &target.provider.id, &filtered_models)
        .await
        .map_err(GatewayError::Internal)?;
    Ok(KeyFetchDisposition::Succeeded)
}

async fn persist_key_fetch_failure(
    state: &(impl ModelFetchRuntimeState + ?Sized),
    key: &StoredProviderCatalogKey,
    now_unix_secs: u64,
    error: String,
) -> Result<(), GatewayError> {
    let safe_error = safe_model_fetch_error(&error);
    state
        .update_provider_catalog_key_model_fetch_state(
            &key.id,
            key.allowed_models.as_ref(),
            Some(now_unix_secs),
            Some(&safe_error),
            Some(now_unix_secs),
        )
        .await?;
    Ok(())
}

pub(crate) fn safe_model_fetch_error(error: &str) -> String {
    let trimmed = error.trim();
    match trimmed {
        "No supported endpoint for Rust models fetch"
        | "Provider transport snapshot unavailable" => return trimmed.to_string(),
        _ => {}
    }

    let lower = trimmed.to_ascii_lowercase();
    if let Some(status) = model_fetch_error_http_status(&lower) {
        return match status {
            401 => "Upstream models fetch authentication failed (status 401)".to_string(),
            403 => "Upstream models fetch authorization failed (status 403)".to_string(),
            404 => "Upstream models fetch endpoint not found (status 404)".to_string(),
            408 => "Upstream models fetch timed out (status 408)".to_string(),
            429 => "Upstream models fetch rate limited (status 429)".to_string(),
            _ => format!("Upstream models fetch failed (status {status})"),
        };
    }
    if lower.contains("unauthorized")
        || lower.contains("authentication failed")
        || lower.contains("invalid api key")
        || lower.contains("invalid token")
    {
        return "Upstream models fetch authentication failed".to_string();
    }
    if lower.contains("forbidden") || lower.contains("authorization failed") {
        return "Upstream models fetch authorization failed".to_string();
    }
    if lower.contains("rate limit") || lower.contains("too many requests") {
        return "Upstream models fetch rate limited".to_string();
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return "Upstream models fetch timed out".to_string();
    }
    if lower.contains("missing api key")
        || lower.contains("missing access token")
        || (lower.contains("requires") && lower.contains("auth"))
    {
        return "Provider credentials unavailable for models fetch".to_string();
    }
    if lower.contains("private_key")
        || lower.contains("auth_config")
        || lower.contains("configuration")
    {
        return "Provider models fetch configuration is invalid".to_string();
    }
    if lower.contains("response body")
        || lower.contains("json")
        || lower.contains("malformed")
        || lower.contains("parse")
        || lower.contains("no models")
        || lower.contains("invalid response")
    {
        return "Upstream models fetch response was invalid".to_string();
    }
    if lower.contains("connect")
        || lower.contains("connection")
        || lower.contains("network")
        || lower.contains("dns")
        || lower.contains("tls")
        || lower.contains("certificate")
    {
        return "Upstream models fetch connection failed".to_string();
    }
    "Upstream models fetch failed".to_string()
}

/// Credential decoding failures from old catalog rows are isolated by the
/// background model-fetch worker. Normal request/admin paths remain fail
/// closed; this predicate only controls whether one maintenance item may be
/// skipped without aborting the whole cycle.
fn is_nonfatal_legacy_credential_error(error: &GatewayError) -> bool {
    let GatewayError::Internal(message) = error else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    // Missing encryption configuration is an operational failure and must
    // remain fail-closed.  Only errors that identify a stored field or a
    // malformed legacy ciphertext are safe to isolate to one key.
    if message.contains("encryption key is not configured") {
        return false;
    }
    message.contains("provider_api_keys.api_key")
        || message.contains("provider_api_keys.auth_config")
        || message.contains("stored provider proxy credentials cannot be decrypted")
        || message.contains("stored endpoint proxy credentials cannot be decrypted")
        || message.contains("stored key proxy credentials cannot be decrypted")
        || message.contains("stored provider proxy changed during credential migration")
        || message.contains("stored endpoint proxy changed during credential migration")
        || message.contains("stored key changed during credential migration")
        || message.contains("stored provider proxy credential migration did not stabilize")
        || message.contains("stored endpoint proxy credential migration did not stabilize")
        || message.contains("stored key proxy credential migration did not stabilize")
        || message.contains("legacy provider catalog credential")
        || message.contains("provider catalog credential is not an authenticated ciphertext")
        || message.contains("provider catalog credential contains reserved framing")
        || message.contains("provider catalog credential authentication failed")
        || message.contains("provider catalog credential envelope")
}

fn model_fetch_error_http_status(error: &str) -> Option<u16> {
    [
        "http ",
        "status ",
        "status=",
        "status:",
        "status_code=",
        "status_code:",
    ]
    .into_iter()
    .find_map(|marker| {
        let suffix = error.split_once(marker)?.1.trim_start();
        let digits = suffix
            .bytes()
            .take_while(u8::is_ascii_digit)
            .take(3)
            .collect::<Vec<_>>();
        (digits.len() == 3)
            .then(|| std::str::from_utf8(&digits).ok()?.parse::<u16>().ok())
            .flatten()
            .filter(|status| (400..600).contains(status))
    })
}

async fn persist_key_fetch_success(
    state: &(impl ModelFetchRuntimeState + ?Sized),
    key: &StoredProviderCatalogKey,
    now_unix_secs: u64,
    allowed_models: &[String],
    upstream_metadata: Option<&Value>,
) -> Result<(), GatewayError> {
    let allowed_models = if allowed_models.is_empty() {
        None
    } else {
        Some(json!(allowed_models))
    };
    let upstream_metadata_updates = upstream_metadata
        .map(|upstream_metadata| {
            upstream_metadata_namespace_updates(key.upstream_metadata.as_ref(), upstream_metadata)
                .into_iter()
                .map(
                    |(namespace, value)| ProviderCatalogUpstreamMetadataNamespaceUpdate {
                        namespace,
                        value,
                    },
                )
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    state
        .update_provider_catalog_key_model_fetch_success(
            &key.id,
            allowed_models.as_ref(),
            now_unix_secs,
            &upstream_metadata_updates,
            Some(now_unix_secs),
        )
        .await?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{
        perform_model_fetch_once_with_state, safe_model_fetch_error, sanitize_model_fetch_key,
        state::ModelFetchRuntimeState,
    };
    use aether_contracts::{ExecutionPlan, ExecutionResult, ProxySnapshot};
    use aether_data_contracts::repository::global_models::{
        AdminGlobalModelListQuery, AdminProviderModelListQuery, StoredAdminGlobalModelPage,
        StoredAdminProviderModel, UpsertAdminProviderModelRecord,
    };
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogUpstreamMetadataNamespaceUpdate, StoredProviderCatalogEndpoint,
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use aether_model_fetch::{
        build_models_fetch_execution_plan, ModelFetchAssociationStore, ModelFetchTransportRuntime,
    };
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    use crate::provider_transport::LocalResolvedOAuthRequestAuth;
    use crate::GatewayError;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    #[derive(Clone, Default)]
    struct TestState {
        providers: Arc<Vec<StoredProviderCatalogProvider>>,
        endpoints: Arc<Vec<StoredProviderCatalogEndpoint>>,
        keys: Arc<Mutex<Vec<StoredProviderCatalogKey>>>,
        transports: Arc<HashMap<(String, String, String), GatewayProviderTransportSnapshot>>,
        transport_errors: Arc<HashMap<(String, String, String), String>>,
        execution_results: Arc<Mutex<VecDeque<ExecutionResult>>>,
        executed_plans: Arc<Mutex<Vec<ExecutionPlan>>>,
        cached_models: Arc<Mutex<HashMap<(String, String), Vec<Value>>>>,
        upstream_metadata_updates: Arc<Mutex<Vec<(String, String, Value, Option<u64>)>>>,
    }

    impl TestState {
        fn new(
            providers: Vec<StoredProviderCatalogProvider>,
            endpoints: Vec<StoredProviderCatalogEndpoint>,
            keys: Vec<StoredProviderCatalogKey>,
            transports: HashMap<(String, String, String), GatewayProviderTransportSnapshot>,
            execution_results: Vec<ExecutionResult>,
        ) -> Self {
            Self {
                providers: Arc::new(providers),
                endpoints: Arc::new(endpoints),
                keys: Arc::new(Mutex::new(keys)),
                transports: Arc::new(transports),
                transport_errors: Arc::new(HashMap::new()),
                execution_results: Arc::new(Mutex::new(VecDeque::from(execution_results))),
                executed_plans: Arc::new(Mutex::new(Vec::new())),
                cached_models: Arc::new(Mutex::new(HashMap::new())),
                upstream_metadata_updates: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_transport_errors(
            mut self,
            transport_errors: HashMap<(String, String, String), String>,
        ) -> Self {
            self.transport_errors = Arc::new(transport_errors);
            self
        }

        fn key(&self, key_id: &str) -> StoredProviderCatalogKey {
            self.keys
                .lock()
                .expect("keys mutex")
                .iter()
                .find(|key| key.id == key_id)
                .cloned()
                .expect("key should exist")
        }
    }

    #[async_trait]
    impl ModelFetchTransportRuntime for TestState {
        async fn resolve_local_oauth_request_auth(
            &self,
            transport: &GatewayProviderTransportSnapshot,
        ) -> Result<Option<LocalResolvedOAuthRequestAuth>, String> {
            if transport.key.auth_type.trim().eq_ignore_ascii_case("oauth") {
                return Ok(Some(LocalResolvedOAuthRequestAuth::Header {
                    name: "authorization".to_string(),
                    value: "Bearer oauth-token".to_string(),
                }));
            }
            Ok(None)
        }

        async fn resolve_model_fetch_proxy(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Option<ProxySnapshot> {
            None
        }

        async fn execute_model_fetch_execution_plan(
            &self,
            plan: &ExecutionPlan,
        ) -> Result<ExecutionResult, String> {
            self.executed_plans
                .lock()
                .expect("executed plans mutex")
                .push(plan.clone());
            self.execution_results
                .lock()
                .expect("execution result mutex")
                .pop_front()
                .ok_or_else(|| "missing execution result".to_string())
        }
    }

    #[async_trait]
    impl ModelFetchAssociationStore for TestState {
        type Error = String;

        fn has_global_model_reader(&self) -> bool {
            false
        }

        fn has_global_model_writer(&self) -> bool {
            false
        }

        fn model_fetch_internal_error(&self, message: String) -> Self::Error {
            message
        }

        async fn list_admin_provider_models(
            &self,
            _query: &AdminProviderModelListQuery,
        ) -> Result<Vec<StoredAdminProviderModel>, Self::Error> {
            Ok(Vec::new())
        }

        async fn list_admin_global_models(
            &self,
            _query: &AdminGlobalModelListQuery,
        ) -> Result<StoredAdminGlobalModelPage, Self::Error> {
            Ok(StoredAdminGlobalModelPage {
                items: Vec::new(),
                total: 0,
            })
        }

        async fn create_admin_provider_model(
            &self,
            _record: &UpsertAdminProviderModelRecord,
        ) -> Result<Option<StoredAdminProviderModel>, Self::Error> {
            Ok(None)
        }

        async fn list_provider_catalog_keys_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, Self::Error> {
            Ok(self
                .keys
                .lock()
                .expect("keys mutex")
                .iter()
                .filter(|key| {
                    provider_ids
                        .iter()
                        .any(|provider_id| provider_id == &key.provider_id)
                })
                .cloned()
                .collect())
        }
    }

    #[async_trait]
    impl ModelFetchRuntimeState for TestState {
        fn has_provider_catalog_data_reader(&self) -> bool {
            true
        }

        fn has_provider_catalog_data_writer(&self) -> bool {
            true
        }

        async fn list_provider_catalog_providers(
            &self,
            _active_only: bool,
        ) -> Result<Vec<StoredProviderCatalogProvider>, GatewayError> {
            Ok(self.providers.as_ref().clone())
        }

        async fn list_provider_catalog_endpoints_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogEndpoint>, GatewayError> {
            Ok(self
                .endpoints
                .iter()
                .filter(|endpoint| {
                    provider_ids
                        .iter()
                        .any(|provider_id| provider_id == &endpoint.provider_id)
                })
                .cloned()
                .collect())
        }

        async fn read_provider_transport_snapshot(
            &self,
            provider_id: &str,
            endpoint_id: &str,
            key_id: &str,
        ) -> Result<Option<GatewayProviderTransportSnapshot>, GatewayError> {
            if let Some(error) = self.transport_errors.get(&(
                provider_id.to_string(),
                endpoint_id.to_string(),
                key_id.to_string(),
            )) {
                return Err(GatewayError::Internal(error.clone()));
            }
            Ok(self
                .transports
                .get(&(
                    provider_id.to_string(),
                    endpoint_id.to_string(),
                    key_id.to_string(),
                ))
                .cloned())
        }

        async fn execute_execution_runtime_sync_plan(
            &self,
            _plan: &ExecutionPlan,
        ) -> Result<ExecutionResult, GatewayError> {
            Err(GatewayError::Internal(
                "execute_execution_runtime_sync_plan should not be called".to_string(),
            ))
        }

        async fn update_provider_catalog_key_model_fetch_state(
            &self,
            key_id: &str,
            allowed_models: Option<&Value>,
            last_models_fetch_at_unix_secs: Option<u64>,
            last_models_fetch_error: Option<&str>,
            updated_at_unix_secs: Option<u64>,
        ) -> Result<(), GatewayError> {
            let mut keys = self.keys.lock().expect("keys mutex");
            let Some(key) = keys.iter_mut().find(|item| item.id == key_id) else {
                return Err(GatewayError::Internal("key not found".to_string()));
            };
            key.allowed_models = allowed_models.cloned();
            key.last_models_fetch_at_unix_secs = last_models_fetch_at_unix_secs;
            key.last_models_fetch_error = last_models_fetch_error.map(str::to_string);
            key.updated_at_unix_secs = updated_at_unix_secs;
            Ok(())
        }

        async fn update_provider_catalog_key_model_fetch_success(
            &self,
            key_id: &str,
            allowed_models: Option<&Value>,
            last_models_fetch_at_unix_secs: u64,
            upstream_metadata_updates: &[ProviderCatalogUpstreamMetadataNamespaceUpdate],
            updated_at_unix_secs: Option<u64>,
        ) -> Result<(), GatewayError> {
            let mut keys = self.keys.lock().expect("keys mutex");
            let Some(key) = keys.iter_mut().find(|key| key.id == key_id) else {
                return Err(GatewayError::Internal("key not found".to_string()));
            };
            key.allowed_models = allowed_models.cloned();
            key.last_models_fetch_at_unix_secs = Some(last_models_fetch_at_unix_secs);
            key.last_models_fetch_error = None;
            if !upstream_metadata_updates.is_empty() {
                let metadata = key
                    .upstream_metadata
                    .get_or_insert_with(|| json!({}))
                    .as_object_mut()
                    .expect("upstream metadata object");
                for update in upstream_metadata_updates {
                    metadata.insert(update.namespace.clone(), update.value.clone());
                }
            }
            key.updated_at_unix_secs = updated_at_unix_secs;
            drop(keys);
            self.upstream_metadata_updates
                .lock()
                .expect("metadata updates mutex")
                .extend(upstream_metadata_updates.iter().map(|update| {
                    (
                        key_id.to_string(),
                        update.namespace.clone(),
                        update.value.clone(),
                        updated_at_unix_secs,
                    )
                }));
            Ok(())
        }

        async fn write_upstream_models_cache(
            &self,
            provider_id: &str,
            key_id: &str,
            cached_models: &[Value],
        ) {
            self.cached_models.lock().expect("cache mutex").insert(
                (provider_id.to_string(), key_id.to_string()),
                cached_models.to_vec(),
            );
        }
    }

    fn sample_provider(provider_id: &str, provider_type: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            provider_id.to_string(),
            provider_id.to_string(),
            None,
            provider_type.to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(true, false, false, None, None, None, None, None, None)
    }

    fn sample_endpoint(
        endpoint_id: &str,
        provider_id: &str,
        api_format: &str,
    ) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            endpoint_id.to_string(),
            provider_id.to_string(),
            api_format.to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://cloudcode-pa.googleapis.com".to_string(),
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

    fn sample_key(
        key_id: &str,
        provider_id: &str,
        auth_type: &str,
        api_formats: &[&str],
    ) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            "primary".to_string(),
            auth_type.to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(api_formats)),
            "encrypted".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");
        key.auto_fetch_models = true;
        key
    }

    fn sample_transport(
        provider_type: &str,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        api_format: &str,
        auth_type: &str,
        decrypted_auth_config: Option<&str>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: provider_id.to_string(),
                name: provider_id.to_string(),
                provider_type: provider_type.to_string(),
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
                id: endpoint_id.to_string(),
                provider_id: provider_id.to_string(),
                api_format: api_format.to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://cloudcode-pa.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: key_id.to_string(),
                provider_id: provider_id.to_string(),
                name: "primary".to_string(),
                auth_type: auth_type.to_string(),
                is_active: true,
                api_formats: Some(vec![api_format.to_string()]),
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
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: decrypted_auth_config.map(ToOwned::to_owned),
            },
        }
    }

    fn execution_result(body: Value) -> ExecutionResult {
        execution_result_with_status(200, body)
    }

    fn execution_result_with_status(status_code: u16, body: Value) -> ExecutionResult {
        ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code,
            headers: Default::default(),
            response_observation: None,
            body: Some(aether_contracts::ResponseBody {
                json_body: Some(body),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        }
    }

    #[test]
    fn model_fetch_error_projection_discards_transport_credentials_and_urls() {
        let error = "connection failed for https://user:password@example.test/v1/models?key=\
                     query-secret: Authorization: Bearer transport-secret-token-value";

        let safe_error = safe_model_fetch_error(error);

        assert_eq!(safe_error, "Upstream models fetch connection failed");
        for secret in [
            "user",
            "password",
            "query-secret",
            "transport-secret-token-value",
            "Bearer",
            "example.test",
        ] {
            assert!(!safe_error.contains(secret));
        }
    }

    #[test]
    fn model_fetch_error_projection_discards_unclassified_details() {
        let error =
            "opaque failure at https://user:password@example.test/private?key=query-secret; \
                     Authorization: Bearer transport-secret-token-value";

        let safe_error = safe_model_fetch_error(error);

        assert_eq!(safe_error, "Upstream models fetch failed");
        for secret in [
            "user",
            "password",
            "query-secret",
            "transport-secret-token-value",
            "Bearer",
            "example.test",
        ] {
            assert!(!safe_error.contains(secret));
        }
    }

    #[test]
    fn invalid_credential_classifier_does_not_swallow_missing_key_configuration() {
        assert!(!super::is_nonfatal_legacy_credential_error(
            &GatewayError::Internal(
                "provider catalog credential encryption key is not configured".to_string(),
            )
        ));
    }

    #[tokio::test]
    async fn gateway_runtime_state_supports_shared_models_fetch_plan_builder() {
        let state = TestState::default();
        let transport = sample_transport(
            "openai",
            "provider-openai",
            "endpoint-openai-chat",
            "key-openai-chat",
            "openai:chat",
            "api_key",
            None,
        );

        let plan = build_models_fetch_execution_plan(&state, &transport)
            .await
            .expect("shared models fetch plan should build");

        assert_eq!(plan.method, "GET");
        assert_eq!(plan.provider_id, "provider-openai");
        assert_eq!(plan.endpoint_id, "endpoint-openai-chat");
        assert_eq!(plan.key_id, "key-openai-chat");
        assert_eq!(plan.model_name.as_deref(), Some("models"));
    }

    #[tokio::test]
    async fn model_fetch_uses_preset_models_without_endpoint() {
        let provider = sample_provider("provider-codex", "codex");
        let mut key = sample_key(
            "key-codex",
            "provider-codex",
            "api_key",
            &["openai:responses"],
        );
        key.upstream_metadata = Some(json!({
            "codex": {
                "quota_by_model": {
                    "gpt-5.6-sol": {"remaining_fraction": 0.75}
                }
            }
        }));
        let state = TestState::new(vec![provider], vec![], vec![key], HashMap::new(), vec![]);

        let summary = perform_model_fetch_once_with_state(&state)
            .await
            .expect("fetch should succeed");

        assert_eq!(summary.attempted, 1);
        assert_eq!(summary.succeeded, 1);
        let updated = state.key("key-codex");
        let allowed_models = updated
            .allowed_models
            .as_ref()
            .and_then(|value| value.as_array().cloned())
            .expect("allowed_models should be set");
        assert!(allowed_models.iter().any(|model| model == "gpt-5.4"));
        let upstream_metadata = updated
            .upstream_metadata
            .as_ref()
            .expect("Codex model catalog should be persisted");
        assert_eq!(
            upstream_metadata["codex"]["quota_by_model"]["gpt-5.6-sol"]["remaining_fraction"],
            0.75
        );
        assert_eq!(
            upstream_metadata["codex_models"]["cards"]["gpt-5.6-sol"]["multi_agent_version"],
            "v2"
        );
        let capabilities = crate::ai_serving::resolve_codex_responses_model_capabilities(
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            Some(upstream_metadata),
        );
        assert!(capabilities.use_responses_lite);
        assert_eq!(
            capabilities.default_reasoning_effort.as_deref(),
            Some("low")
        );
        assert!(capabilities
            .supported_reasoning_efforts
            .iter()
            .any(|effort| effort == "ultra"));
        let metadata_updates = state
            .upstream_metadata_updates
            .lock()
            .expect("metadata updates mutex");
        assert_eq!(metadata_updates.len(), 1);
        assert_eq!(metadata_updates[0].0, "key-codex");
        assert_eq!(metadata_updates[0].1, "codex_models");
        assert_eq!(
            metadata_updates[0].2["cards"]["gpt-5.6-sol"]["multi_agent_version"],
            "v2"
        );
        assert!(state
            .cached_models
            .lock()
            .expect("cache mutex")
            .contains_key(&("provider-codex".to_string(), "key-codex".to_string())));
    }

    #[tokio::test]
    async fn model_fetch_merges_antigravity_metadata_and_preserves_reset_time() {
        let provider = sample_provider("provider-antigravity", "antigravity");
        let endpoint = sample_endpoint(
            "endpoint-antigravity",
            "provider-antigravity",
            "gemini:generate_content",
        );
        let mut key = sample_key(
            "key-antigravity",
            "provider-antigravity",
            "oauth",
            &["gemini:generate_content"],
        );
        key.upstream_metadata = Some(json!({
            "antigravity": {
                "quota_by_model": {
                    "gemini-2.5-pro": {
                        "reset_time": "2026-04-12T00:00:00Z"
                    }
                }
            }
        }));
        let transport = sample_transport(
            "antigravity",
            "provider-antigravity",
            "endpoint-antigravity",
            "key-antigravity",
            "gemini:generate_content",
            "oauth",
            Some(r#"{"project_id":"project-1","client_version":"1.2.3","session_id":"sess-1"}"#),
        );
        let state = TestState::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            HashMap::from([(
                (
                    "provider-antigravity".to_string(),
                    "endpoint-antigravity".to_string(),
                    "key-antigravity".to_string(),
                ),
                transport,
            )]),
            vec![execution_result(json!({
                "models": {
                    "gemini-2.5-pro": {
                        "displayName": "Gemini 2.5 Pro",
                        "quotaInfo": {
                            "remainingFraction": 0.25
                        }
                    }
                }
            }))],
        );

        let summary = perform_model_fetch_once_with_state(&state)
            .await
            .expect("fetch should succeed");

        assert_eq!(summary.succeeded, 1);
        let updated = state.key("key-antigravity");
        assert_eq!(updated.allowed_models, Some(json!(["gemini-2.5-pro"])));
        assert_eq!(
            updated
                .upstream_metadata
                .as_ref()
                .and_then(|value| value.get("antigravity"))
                .and_then(|value| value.get("quota_by_model"))
                .and_then(|value| value.get("gemini-2.5-pro"))
                .and_then(|value| value.get("reset_time")),
            Some(&json!("2026-04-12T00:00:00Z"))
        );
    }

    #[tokio::test]
    async fn model_fetch_fetches_windsurf_model_configs_and_persists_allowed_models() {
        let provider = sample_provider("provider-windsurf", "windsurf");
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-windsurf-chat".to_string(),
            "provider-windsurf".to_string(),
            "openai:chat".to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://server.codeium.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");
        let key = sample_key(
            "key-windsurf",
            "provider-windsurf",
            "api_key",
            &["openai:chat"],
        );
        let mut transport = sample_transport(
            "windsurf",
            "provider-windsurf",
            "endpoint-windsurf-chat",
            "key-windsurf",
            "openai:chat",
            "api_key",
            Some(r#"{"provider_type":"windsurf"}"#),
        );
        transport.endpoint.base_url = "https://server.codeium.com".to_string();
        transport.key.decrypted_api_key = "devin-session-token$abc".to_string();
        let state = TestState::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            HashMap::from([(
                (
                    "provider-windsurf".to_string(),
                    "endpoint-windsurf-chat".to_string(),
                    "key-windsurf".to_string(),
                ),
                transport,
            )]),
            vec![execution_result(json!({
                "clientModelConfigs": [
                    {
                        "modelUid": "claude-sonnet-4-6",
                        "label": "Claude Sonnet 4.6",
                        "provider": "anthropic",
                        "supportsImages": true,
                        "creditMultiplier": 4
                    },
                    {
                        "modelUid": "gpt-5.4",
                        "label": "GPT-5.4",
                        "provider": "openai"
                    }
                ],
                "defaultOverrideModelConfig": {
                    "modelUid": "claude-sonnet-4-6"
                }
            }))],
        );

        let summary = perform_model_fetch_once_with_state(&state)
            .await
            .expect("fetch should succeed");

        assert_eq!(summary.succeeded, 1);
        let plans = state.executed_plans.lock().expect("executed plans mutex");
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].url,
            "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetCascadeModelConfigs"
        );
        assert_eq!(plans[0].method, "POST");
        assert_eq!(plans[0].provider_api_format, "windsurf:model_configs");
        assert_eq!(
            plans[0]
                .body
                .json_body
                .as_ref()
                .and_then(|body| body.get("metadata"))
                .and_then(|metadata| metadata.get("apiKey")),
            Some(&json!("devin-session-token$abc"))
        );
        drop(plans);

        let updated = state.key("key-windsurf");
        assert_eq!(
            updated.allowed_models,
            Some(json!(["claude-sonnet-4-6", "gpt-5.4"]))
        );
        assert_eq!(
            updated
                .upstream_metadata
                .as_ref()
                .and_then(|value| value.get("windsurf"))
                .and_then(|value| value.get("allowed_models_count")),
            Some(&json!(2))
        );
        assert_eq!(
            updated
                .upstream_metadata
                .as_ref()
                .and_then(|value| value.get("windsurf"))
                .and_then(|value| value.get("default_model_uid")),
            Some(&json!("claude-sonnet-4-6"))
        );
        let cached = state.cached_models.lock().expect("cache mutex");
        let cached_models = cached
            .get(&("provider-windsurf".to_string(), "key-windsurf".to_string()))
            .expect("cached models should be written");
        assert_eq!(
            cached_models[0]["api_formats"],
            json!(["openai:chat", "openai:responses", "claude:messages"])
        );
    }

    #[tokio::test]
    async fn model_fetch_failure_keeps_existing_allowed_models() {
        let provider = sample_provider("provider-openai", "openai");
        let endpoint = sample_endpoint(
            "endpoint-openai-responses",
            "provider-openai",
            "openai:responses",
        );
        let mut key = sample_key(
            "key-openai-responses",
            "provider-openai",
            "api_key",
            &["openai:responses"],
        );
        key.allowed_models = Some(json!(["gpt-old"]));
        let state = TestState::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            HashMap::new(),
            vec![],
        );

        let summary = perform_model_fetch_once_with_state(&state)
            .await
            .expect("fetch should finish");

        assert_eq!(summary.succeeded, 0);
        assert_eq!(summary.skipped, 1);
        let updated = state.key("key-openai-responses");
        assert_eq!(updated.allowed_models, Some(json!(["gpt-old"])));
        assert_eq!(
            updated.last_models_fetch_error.as_deref(),
            Some("Provider transport snapshot unavailable")
        );
    }

    #[tokio::test]
    async fn model_fetch_isolates_malformed_legacy_key_from_healthy_key() {
        let provider = sample_provider("provider-openai", "openai");
        let endpoint = sample_endpoint(
            "endpoint-openai-responses",
            "provider-openai",
            "openai:responses",
        );
        let mut malformed = sample_key(
            "key-openai-malformed",
            "provider-openai",
            "api_key",
            &["openai:responses"],
        );
        malformed.encrypted_api_key = Some("legacy-plaintext-or-corrupt".to_string());
        malformed.allowed_models = Some(json!(["legacy-model"]));
        malformed.last_models_fetch_error = Some("previous error".to_string());
        let malformed_ciphertext = malformed.encrypted_api_key.clone();
        let healthy = sample_key(
            "key-openai-healthy",
            "provider-openai",
            "api_key",
            &["openai:responses"],
        );
        let healthy_transport = sample_transport(
            "openai",
            "provider-openai",
            "endpoint-openai-responses",
            "key-openai-healthy",
            "openai:responses",
            "api_key",
            None,
        );
        let state = TestState::new(
            vec![provider],
            vec![endpoint],
            vec![malformed, healthy],
            HashMap::from([(
                (
                    "provider-openai".to_string(),
                    "endpoint-openai-responses".to_string(),
                    "key-openai-healthy".to_string(),
                ),
                healthy_transport,
            )]),
            vec![execution_result(json!({
                "data": [{"id": "gpt-healthy"}]
            }))],
        )
        .with_transport_errors(HashMap::from([(
            (
                "provider-openai".to_string(),
                "endpoint-openai-responses".to_string(),
                "key-openai-malformed".to_string(),
            ),
            "provider_api_keys.api_key is not an authenticated ciphertext".to_string(),
        )]));

        let summary = perform_model_fetch_once_with_state(&state)
            .await
            .expect("one malformed key must not abort the cycle");

        assert_eq!(summary.attempted, 2);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.succeeded, 1);
        let malformed_after = state.key("key-openai-malformed");
        assert_eq!(malformed_after.encrypted_api_key, malformed_ciphertext);
        assert_eq!(
            malformed_after.allowed_models,
            Some(json!(["legacy-model"]))
        );
        assert_eq!(
            malformed_after.last_models_fetch_error.as_deref(),
            Some("previous error")
        );
        assert_eq!(
            state.key("key-openai-healthy").allowed_models,
            Some(json!(["gpt-healthy"]))
        );
    }

    #[test]
    fn sanitized_model_fetch_key_drops_raw_transport_and_diagnostic_state() {
        let mut key = sample_key("key-sanitize", "provider", "api_key", &["openai:responses"]);
        key.capabilities = Some(json!({"secret": "capability"}));
        key.note = Some("operator note".to_string());
        key.proxy = Some(json!({"url": "http://user:pass@example.test"}));
        key.last_models_fetch_error = Some("upstream detail".to_string());
        key.oauth_invalid_reason = Some("token detail".to_string());
        key.allowed_models = Some(json!(["keep-model-filter"]));
        key.upstream_metadata = Some(json!({"provider": {"quota": 1}}));

        let sanitized = sanitize_model_fetch_key(key);

        assert_eq!(sanitized.encrypted_api_key, None);
        assert_eq!(sanitized.encrypted_auth_config, None);
        assert_eq!(sanitized.proxy, None);
        assert_eq!(sanitized.fingerprint, None);
        assert_eq!(sanitized.note, None);
        assert_eq!(sanitized.last_models_fetch_error, None);
        assert_eq!(sanitized.oauth_invalid_reason, None);
        assert_eq!(sanitized.allowed_models, Some(json!(["keep-model-filter"])));
        assert_eq!(
            sanitized.upstream_metadata,
            Some(json!({"provider": {"quota": 1}}))
        );
    }

    #[test]
    fn sanitized_model_fetch_provider_and_endpoint_drop_transport_secrets() {
        let mut provider = sample_provider("provider-sanitize", "openai");
        provider.proxy = Some(json!({"url": "http://user:pass@example.test"}));
        provider.config = Some(json!({"api_key": "provider-secret"}));
        let sanitized_provider = super::sanitize_model_fetch_provider(provider);
        assert_eq!(sanitized_provider.proxy, None);
        assert_eq!(sanitized_provider.config, None);

        let mut endpoint =
            sample_endpoint("endpoint-sanitize", "provider-sanitize", "openai:responses");
        endpoint.header_rules = Some(json!({"authorization": "Bearer endpoint-secret"}));
        endpoint.body_rules = Some(json!({"token": "endpoint-secret"}));
        endpoint.config = Some(json!({"password": "endpoint-secret"}));
        endpoint.format_acceptance_config = Some(json!({"secret": "endpoint-secret"}));
        endpoint.proxy = Some(json!({"url": "http://user:pass@example.test"}));
        let sanitized_endpoint = super::sanitize_model_fetch_endpoint(endpoint);
        assert_eq!(sanitized_endpoint.header_rules, None);
        assert_eq!(sanitized_endpoint.body_rules, None);
        assert_eq!(sanitized_endpoint.config, None);
        assert_eq!(sanitized_endpoint.format_acceptance_config, None);
        assert_eq!(sanitized_endpoint.proxy, None);
        assert_eq!(sanitized_endpoint.id, "endpoint-sanitize");
        assert_eq!(sanitized_endpoint.api_format, "openai:responses");
    }

    #[tokio::test]
    async fn model_fetch_failure_does_not_persist_upstream_error_body_credentials() {
        const UPSTREAM_SECRET: &str = "upstream-secret-token-value";
        let provider = sample_provider("provider-openai", "openai");
        let endpoint = sample_endpoint(
            "endpoint-openai-responses",
            "provider-openai",
            "openai:responses",
        );
        let key = sample_key(
            "key-openai-responses",
            "provider-openai",
            "api_key",
            &["openai:responses"],
        );
        let transport = sample_transport(
            "openai",
            "provider-openai",
            "endpoint-openai-responses",
            "key-openai-responses",
            "openai:responses",
            "api_key",
            None,
        );
        let state = TestState::new(
            vec![provider],
            vec![endpoint],
            vec![key],
            HashMap::from([(
                (
                    "provider-openai".to_string(),
                    "endpoint-openai-responses".to_string(),
                    "key-openai-responses".to_string(),
                ),
                transport,
            )]),
            vec![execution_result_with_status(
                401,
                json!({
                    "error": {
                        "message": format!(
                            "Authorization: Bearer {UPSTREAM_SECRET}; api_key=query-secret; \
                             config=/srv/provider/private.json"
                        )
                    }
                }),
            )],
        );

        let summary = perform_model_fetch_once_with_state(&state)
            .await
            .expect("fetch should finish with a projected failure");

        assert_eq!(summary.failed, 1);
        let persisted = state
            .key("key-openai-responses")
            .last_models_fetch_error
            .expect("safe failure should be persisted");
        assert_eq!(
            persisted,
            "Upstream models fetch authentication failed (status 401)"
        );
        for secret in [
            UPSTREAM_SECRET,
            "query-secret",
            "/srv/provider/private.json",
            "Bearer",
            "api_key",
        ] {
            assert!(!persisted.contains(secret));
        }
    }
}
