use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_contracts::{
    ExecutionError, ExecutionErrorKind, ExecutionPlan, ExecutionResult, RequestBody,
};
use aether_crypto::{rsa_pkcs1_sha256_sign, RsaPkcs1Sha256Error};
use aether_provider_transport::antigravity::{
    resolve_local_antigravity_request_auth, AntigravityRequestAuthSupport,
};
use aether_provider_transport::vertex::{
    looks_like_vertex_ai_host, parse_vertex_service_account_auth_config,
};
use aether_provider_transport::{
    is_vertex_api_key_transport_context, resolve_transport_execution_timeouts,
    resolve_transport_profile, GatewayProviderTransportSnapshot,
};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use serde_json::{json, Value};

use crate::logic::{
    aggregate_models_for_cache, codex_model_identity, extract_error_message,
    merge_codex_models_preserving_cards, parse_codex_models_response_page,
    parse_models_response_page, parse_windsurf_model_configs_response, preset_models_for_provider,
    project_codex_models_for_legacy_cache,
};
use crate::transport::{
    build_antigravity_fetch_available_models_plan, build_antigravity_load_code_assist_plan,
    build_gemini_cli_load_code_assist_plan, build_kiro_list_available_models_plan,
    build_standard_models_fetch_execution_plan_for_client_version,
    build_vertex_models_fetch_execution_plan, build_windsurf_model_configs_execution_plan,
    ModelFetchTransportRuntime,
};

const ANTIGRAVITY_SANDBOX_BASE_URL: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
const ANTIGRAVITY_DAILY_BASE_URL: &str = "https://daily-cloudcode-pa.googleapis.com";
const ANTIGRAVITY_PROD_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";
const ANTIGRAVITY_BLOCKED_MODELS: &[&str] = &["chat_23310", "chat_20706"];
const VERTEX_API_BASE_URL: &str = "https://aiplatform.googleapis.com";
const VERTEX_MODEL_GARDEN_API_VERSION: &str = "v1beta1";
const VERTEX_PAGE_SIZE: &str = "100";
const VERTEX_MAX_PAGES: usize = 20;
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[derive(Debug, Clone, PartialEq)]
pub struct ModelsFetchOutcome {
    pub fetched_model_ids: Vec<String>,
    /// Provider response cards. Versioned Codex catalogs remain byte-for-byte opaque here.
    pub cached_models: Vec<Value>,
    /// Provider cards projected into Aether's legacy admin/runtime-cache shape.
    pub legacy_models: Vec<Value>,
    pub errors: Vec<String>,
    pub has_success: bool,
    /// Only native `models` responses may populate the opaque Codex client catalog.
    pub native_codex_catalog: bool,
    pub upstream_metadata: Option<Value>,
    pub etag: Option<String>,
    pub upstream_status: Option<u16>,
}

#[derive(Debug)]
struct ConsistentValue<T> {
    value: Option<T>,
    observed: bool,
    consistent: bool,
}

impl<T> Default for ConsistentValue<T> {
    fn default() -> Self {
        Self {
            value: None,
            observed: false,
            consistent: true,
        }
    }
}

impl<T: PartialEq> ConsistentValue<T> {
    fn observe(&mut self, candidate: Option<T>) {
        if !self.observed {
            self.consistent = candidate.is_some();
            self.value = candidate;
            self.observed = true;
            return;
        }
        if self.value.as_ref() != candidate.as_ref() {
            self.consistent = false;
            self.value = None;
        }
    }

    fn finish(self) -> Option<T> {
        (self.observed && self.consistent)
            .then_some(self.value)
            .flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFetchStrategyKind {
    PresetCatalog,
    StandardTransport,
    Vertex,
    Antigravity,
    GeminiCliPreset,
    Kiro,
    Windsurf,
}

pub trait ModelFetchStrategy {
    fn provider_id(&self) -> &str;

    fn kind(&self) -> ModelFetchStrategyKind;
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectedModelFetchStrategy {
    provider_type: String,
    kind: ModelFetchStrategyKind,
    preset_models: Option<Vec<Value>>,
}

impl ModelFetchStrategy for SelectedModelFetchStrategy {
    fn provider_id(&self) -> &str {
        self.provider_type.as_str()
    }

    fn kind(&self) -> ModelFetchStrategyKind {
        self.kind
    }
}

pub async fn fetch_models_from_transports(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
) -> Result<ModelsFetchOutcome, String> {
    fetch_models_from_transports_for_client_version(runtime, transports, None).await
}

pub async fn fetch_models_from_transports_for_client_version(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
    codex_client_version: Option<&str>,
) -> Result<ModelsFetchOutcome, String> {
    let strategy = select_model_fetch_strategy(transports)
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    execute_model_fetch_strategy(
        runtime,
        transports,
        strategy,
        codex_client_version,
        codex_client_version.is_none(),
    )
    .await
    .map_err(|error| sanitize_model_fetch_error(&error))
}

/// Management also supports Codex-compatible proxies returning OpenAI `data` arrays,
/// independently of the fingerprint sent upstream. Public client catalogs stay strict.
pub async fn fetch_models_from_transports_for_management(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
    codex_client_version: Option<&str>,
) -> Result<ModelsFetchOutcome, String> {
    let strategy = select_model_fetch_strategy(transports)
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    execute_model_fetch_strategy(runtime, transports, strategy, codex_client_version, true)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))
}

fn select_model_fetch_strategy(
    transports: &[GatewayProviderTransportSnapshot],
) -> Result<SelectedModelFetchStrategy, String> {
    let Some(first_transport) = transports.first() else {
        return Err("No transport snapshots available for models fetch".to_string());
    };

    let provider_type = first_transport
        .provider
        .provider_type
        .trim()
        .to_ascii_lowercase();
    if let Some(models) = preset_models_for_provider(&provider_type) {
        if provider_type == "kiro" {
            return Ok(SelectedModelFetchStrategy {
                provider_type,
                kind: ModelFetchStrategyKind::Kiro,
                preset_models: None,
            });
        }
        if provider_type == "codex" {
            return Ok(SelectedModelFetchStrategy {
                provider_type,
                kind: ModelFetchStrategyKind::StandardTransport,
                preset_models: None,
            });
        }
        if provider_type == "gemini_cli" {
            return Ok(SelectedModelFetchStrategy {
                provider_type,
                kind: ModelFetchStrategyKind::GeminiCliPreset,
                preset_models: Some(models),
            });
        }
        return Ok(SelectedModelFetchStrategy {
            provider_type,
            kind: ModelFetchStrategyKind::PresetCatalog,
            preset_models: Some(models),
        });
    }

    if transports.iter().any(is_vertex_api_key_transport_context) {
        return Ok(SelectedModelFetchStrategy {
            provider_type,
            kind: ModelFetchStrategyKind::Vertex,
            preset_models: None,
        });
    }

    let kind = match provider_type.as_str() {
        "antigravity" => ModelFetchStrategyKind::Antigravity,
        "vertex_ai" => ModelFetchStrategyKind::Vertex,
        "windsurf" => ModelFetchStrategyKind::Windsurf,
        _ => ModelFetchStrategyKind::StandardTransport,
    };
    Ok(SelectedModelFetchStrategy {
        provider_type,
        kind,
        preset_models: None,
    })
}

async fn execute_model_fetch_strategy(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
    strategy: SelectedModelFetchStrategy,
    codex_client_version: Option<&str>,
    allow_codex_legacy_response: bool,
) -> Result<ModelsFetchOutcome, String> {
    let Some(first_transport) = transports.first() else {
        return Err("No transport snapshots available for models fetch".to_string());
    };

    match strategy.kind() {
        ModelFetchStrategyKind::PresetCatalog => Ok(build_success_outcome(
            strategy.preset_models.unwrap_or_default(),
            None,
            true,
        )),
        ModelFetchStrategyKind::StandardTransport => {
            fetch_standard_models(
                runtime,
                transports,
                strategy.provider_id(),
                codex_client_version,
                allow_codex_legacy_response,
            )
            .await
        }
        ModelFetchStrategyKind::Vertex => fetch_vertex_models(runtime, transports).await,
        ModelFetchStrategyKind::Antigravity => {
            fetch_antigravity_models(runtime, first_transport).await
        }
        ModelFetchStrategyKind::GeminiCliPreset => {
            fetch_gemini_cli_models(
                runtime,
                first_transport,
                strategy.preset_models.unwrap_or_default(),
            )
            .await
        }
        ModelFetchStrategyKind::Kiro => fetch_kiro_models(runtime, first_transport).await,
        ModelFetchStrategyKind::Windsurf => fetch_windsurf_models(runtime, first_transport).await,
    }
}

async fn fetch_standard_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
    provider_type: &str,
    codex_client_version: Option<&str>,
    allow_codex_legacy_response: bool,
) -> Result<ModelsFetchOutcome, String> {
    let mut all_models = Vec::new();
    let mut successful_codex_catalogs = Vec::<(String, Vec<Value>)>::new();
    let mut errors = Vec::new();
    let mut has_success = false;
    let mut etag = ConsistentValue::default();
    let mut upstream_status = ConsistentValue::default();
    let is_codex = provider_type.trim().eq_ignore_ascii_case("codex");
    let mut native_codex_catalog = is_codex;

    for transport in transports {
        match fetch_standard_models_for_transport(
            runtime,
            transport,
            codex_client_version,
            allow_codex_legacy_response,
        )
        .await
        {
            Ok(outcome) => {
                native_codex_catalog &= outcome.native_codex_catalog;
                all_models.extend(outcome.cached_models.iter().cloned());
                if is_codex && outcome.has_success {
                    successful_codex_catalogs
                        .push((transport.endpoint.api_format.clone(), outcome.cached_models));
                }
                has_success |= outcome.has_success;
                if outcome.has_success {
                    etag.observe(outcome.etag);
                    upstream_status.observe(outcome.upstream_status);
                }
            }
            Err((err, status)) => {
                upstream_status.observe(status);
                let format_label = model_fetch_format_label(&transport.endpoint.api_format);
                errors.push(format!(
                    "{format_label}: {}",
                    sanitize_model_fetch_error(&err)
                ));
            }
        }
    }

    let merged_models = if is_codex {
        merge_codex_models_preserving_cards(&all_models)?
    } else {
        aggregate_models_for_cache(&all_models)
    };
    let codex_model_ids = is_codex.then(|| collect_codex_model_ids(&merged_models));
    let upstream_metadata =
        crate::logic::model_catalog_upstream_metadata(provider_type, &merged_models);
    let mut outcome = build_success_outcome(merged_models, upstream_metadata, has_success);
    outcome.native_codex_catalog = native_codex_catalog && has_success;
    if let Some(model_ids) = codex_model_ids {
        outcome.fetched_model_ids = model_ids;
        outcome.legacy_models = project_codex_models_for_legacy_cache(
            successful_codex_catalogs
                .iter()
                .map(|(api_format, models)| (api_format.as_str(), models.as_slice())),
        );
    }
    Ok(outcome
        .with_errors(errors)
        .with_etag(etag.finish())
        .with_upstream_status(upstream_status.finish()))
}

async fn fetch_standard_models_for_transport(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    codex_client_version: Option<&str>,
    allow_codex_legacy_response: bool,
) -> Result<ModelsFetchOutcome, (String, Option<u16>)> {
    let mut all_models = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let mut next_after_id = None;
    let mut has_success = false;
    let mut etag = ConsistentValue::default();
    let mut upstream_status = ConsistentValue::default();
    let is_codex = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex");
    let mut native_codex_catalog = is_codex;

    for _ in 0..20 {
        let plan = build_standard_models_fetch_execution_plan_for_client_version(
            runtime,
            transport,
            next_after_id.as_deref(),
            codex_client_version,
        )
        .await
        .map_err(|err| (err, None))?;
        let result = runtime
            .execute_model_fetch_execution_plan(&plan)
            .await
            .map_err(|err| (err, None))?;
        upstream_status.observe(Some(result.status_code));
        let body_json =
            execution_result_json_body(&result).map_err(|err| (err, Some(result.status_code)))?;
        native_codex_catalog &= body_json.get("models").and_then(Value::as_array).is_some();
        let parsed = if is_codex {
            parse_codex_models_response_for_request(
                &transport.endpoint.api_format,
                &body_json,
                if allow_codex_legacy_response {
                    None
                } else {
                    codex_client_version
                },
            )
        } else {
            parse_models_response_page(&transport.endpoint.api_format, &body_json)
        }
        .map_err(|err| (err, Some(result.status_code)))?;
        etag.observe(execution_result_header(&result, "etag"));
        has_success = true;
        if is_codex {
            // Preserve every opaque card until the catalog-wide merge can distinguish exact
            // duplicates from conflicting `id`/`slug` identities across endpoint transports.
            all_models.extend(parsed.cached_models);
        } else {
            for model in parsed.cached_models {
                let Some(model_id) = model
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                if !seen_ids.insert(model_id.to_string()) {
                    continue;
                }
                all_models.push(model);
            }
        }

        let Some(next_cursor) = parsed
            .has_more
            .then_some(parsed.next_after_id)
            .flatten()
            .filter(|value| next_after_id.as_deref() != Some(value.as_str()))
        else {
            break;
        };
        next_after_id = Some(next_cursor);
    }

    let mut outcome = build_success_outcome(all_models, None, has_success);
    outcome.native_codex_catalog = native_codex_catalog && has_success;
    Ok(outcome
        .with_etag(etag.finish())
        .with_upstream_status(upstream_status.finish()))
}

fn parse_codex_models_response_for_request(
    endpoint_api_format: &str,
    body: &Value,
    codex_client_version: Option<&str>,
) -> Result<crate::logic::ModelsFetchPage, String> {
    if codex_client_version.is_none()
        && (body.is_array() || body.get("data").and_then(Value::as_array).is_some())
    {
        return parse_models_response_page(endpoint_api_format, body);
    }
    parse_codex_models_response_page(body)
}

async fn fetch_antigravity_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<ModelsFetchOutcome, String> {
    let (project_id, hydrated_transport, project_metadata) =
        resolve_or_hydrate_antigravity_project(runtime, transport).await?;

    let mut errors = Vec::new();
    for base_url in [
        ANTIGRAVITY_DAILY_BASE_URL,
        ANTIGRAVITY_PROD_BASE_URL,
        ANTIGRAVITY_SANDBOX_BASE_URL,
    ] {
        let plan = match build_antigravity_fetch_available_models_plan(
            runtime,
            &hydrated_transport,
            base_url,
            &project_id,
        )
        .await
        {
            Ok(plan) => plan,
            Err(err) => return Err(sanitize_model_fetch_error(&err)),
        };

        let result = match runtime.execute_model_fetch_execution_plan(&plan).await {
            Ok(result) => result,
            Err(err) => {
                errors.push(format!(
                    "antigravity models fetch failed: {}",
                    sanitize_model_fetch_error(&err)
                ));
                continue;
            }
        };

        if (200..300).contains(&result.status_code) {
            let body_json = execution_result_json_body_allow_empty(&result)?;
            let (models, metadata) = parse_antigravity_models_response(&body_json)?;
            let metadata = metadata
                .map(|metadata| attach_antigravity_project_metadata(metadata, &project_id))
                .or(project_metadata.clone());
            return Ok(build_success_outcome(models, metadata, true));
        }

        let error = execution_result_error_message(&result);
        if should_fallback_antigravity_status(result.status_code) {
            errors.push(format!("antigravity models fetch failed: {error}"));
            continue;
        }
        return Err(sanitize_model_fetch_error(&error));
    }

    Ok(ModelsFetchOutcome {
        fetched_model_ids: Vec::new(),
        cached_models: Vec::new(),
        legacy_models: Vec::new(),
        errors,
        has_success: false,
        native_codex_catalog: false,
        upstream_metadata: None,
        etag: None,
        upstream_status: None,
    })
}

async fn resolve_or_hydrate_antigravity_project(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<(String, GatewayProviderTransportSnapshot, Option<Value>), String> {
    if let Some(project_id) = resolve_antigravity_project_id_from_transport(transport) {
        let metadata = Some(build_antigravity_project_metadata(&project_id));
        return Ok((project_id, transport.clone(), metadata));
    }

    let plan = build_antigravity_load_code_assist_plan(runtime, transport)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    let result = runtime
        .execute_model_fetch_execution_plan(&plan)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    if !(200..300).contains(&result.status_code) {
        return Err(format!(
            "antigravity: loadCodeAssist failed: {}",
            execution_result_error_message(&result)
        ));
    }
    let body_json = execution_result_json_body_allow_empty(&result)?;
    let project_id = extract_cloud_ai_companion_project_id(&body_json)
        .ok_or_else(|| "antigravity: loadCodeAssist response missing project_id".to_string())?;
    let metadata = build_antigravity_project_metadata(&project_id);
    let mut hydrated_transport = transport.clone();
    hydrated_transport.key.upstream_metadata = Some(metadata.clone());

    Ok((project_id, hydrated_transport, Some(metadata)))
}

fn resolve_antigravity_project_id_from_transport(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    match resolve_local_antigravity_request_auth(transport) {
        AntigravityRequestAuthSupport::Supported(auth) => Some(auth.project_id),
        AntigravityRequestAuthSupport::Unsupported(_) => None,
    }
}

fn build_antigravity_project_metadata(project_id: &str) -> Value {
    json!({
        "antigravity": {
            "project_id": project_id,
            "updated_at": now_unix_secs(),
        }
    })
}

fn attach_antigravity_project_metadata(mut metadata: Value, project_id: &str) -> Value {
    let Value::Object(root) = &mut metadata else {
        return build_antigravity_project_metadata(project_id);
    };
    let antigravity = root
        .entry("antigravity".to_string())
        .or_insert_with(|| json!({}));
    let Some(object) = antigravity.as_object_mut() else {
        *antigravity = json!({
            "project_id": project_id,
            "updated_at": now_unix_secs(),
        });
        return metadata;
    };
    object
        .entry("project_id".to_string())
        .or_insert_with(|| Value::String(project_id.to_string()));
    object
        .entry("updated_at".to_string())
        .or_insert_with(|| Value::from(now_unix_secs()));
    metadata
}

async fn fetch_gemini_cli_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    models: Vec<Value>,
) -> Result<ModelsFetchOutcome, String> {
    let mut provider_meta = serde_json::Map::new();
    provider_meta.insert("updated_at".to_string(), Value::from(now_unix_secs()));

    if let Ok(plan) = build_gemini_cli_load_code_assist_plan(runtime, transport).await {
        if let Ok(result) = runtime.execute_model_fetch_execution_plan(&plan).await {
            if (200..300).contains(&result.status_code) {
                if let Ok(body_json) = execution_result_json_body_allow_empty(&result) {
                    if let Some(plan_type) = extract_gemini_cli_plan_type(&body_json) {
                        provider_meta.insert("plan_type".to_string(), Value::String(plan_type));
                    }
                    for key in ["paidTier", "currentTier"] {
                        if let Some(value) = extract_gemini_cli_tier_metadata(&body_json, key) {
                            provider_meta.insert(key.to_string(), value);
                        }
                    }
                    if let Some(project_id) = extract_cloud_ai_companion_project_id(&body_json)
                        .or_else(|| {
                            transport_auth_config(transport)
                                .and_then(|value| value.get("project_id").cloned())
                                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                        })
                    {
                        provider_meta.insert("project_id".to_string(), Value::String(project_id));
                    }
                }
            }
        }
    }

    let upstream_metadata = (!provider_meta.is_empty()).then(|| {
        Value::Object(
            [("gemini_cli".to_string(), Value::Object(provider_meta))]
                .into_iter()
                .collect(),
        )
    });
    Ok(build_success_outcome(models, upstream_metadata, true))
}

async fn fetch_kiro_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<ModelsFetchOutcome, String> {
    let plan = build_kiro_list_available_models_plan(runtime, transport)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    let result = runtime
        .execute_model_fetch_execution_plan(&plan)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    if !(200..300).contains(&result.status_code) {
        return Err(execution_result_error_message(&result));
    }

    let body_json = execution_result_json_body_allow_empty(&result)?;
    let (models, metadata) = parse_kiro_available_models_response(&body_json)?;
    Ok(build_success_outcome(models, metadata, true))
}

async fn fetch_windsurf_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<ModelsFetchOutcome, String> {
    let plan = build_windsurf_model_configs_execution_plan(runtime, transport)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    let result = runtime
        .execute_model_fetch_execution_plan(&plan)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    if !(200..300).contains(&result.status_code) {
        return Err(execution_result_error_message(&result));
    }

    let body_json = execution_result_json_body_allow_empty(&result)?;
    let (models, metadata) = parse_windsurf_model_configs_response(&body_json, now_unix_secs())?;
    Ok(build_success_outcome(
        models.cached_models,
        Some(metadata),
        true,
    ))
}

async fn fetch_vertex_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
) -> Result<ModelsFetchOutcome, String> {
    let Some(first_transport) = transports.first() else {
        return Err("Vertex models fetch requires at least one transport".to_string());
    };
    let auth_config = transport_auth_config(first_transport);
    if looks_like_vertex_service_account(auth_config.as_ref()) {
        fetch_vertex_service_account_models(runtime, transports, auth_config.as_ref()).await
    } else {
        fetch_vertex_api_key_models(runtime, transports, auth_config.as_ref()).await
    }
}

async fn fetch_vertex_api_key_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
    auth_config: Option<&Value>,
) -> Result<ModelsFetchOutcome, String> {
    let Some(reference_transport) = select_transport_for_api_format(transports, "gemini:") else {
        return Err("vertex_ai(api_key): missing gemini endpoint".to_string());
    };
    let api_key = reference_transport.key.decrypted_api_key.trim();
    if api_key.is_empty() || api_key == "__placeholder__" {
        return Ok(ModelsFetchOutcome {
            fetched_model_ids: Vec::new(),
            cached_models: Vec::new(),
            legacy_models: Vec::new(),
            errors: vec!["vertex_ai(api_key): missing api key".to_string()],
            has_success: false,
            native_codex_catalog: false,
            upstream_metadata: None,
            etag: None,
            upstream_status: None,
        });
    }

    let mut all_models = Vec::new();
    let mut hard_errors = Vec::new();
    let mut soft_errors = Vec::new();
    let mut has_success = false;

    // The API key is a bearer-like cloud credential.  Endpoint records can be
    // imported or edited by administrators, so never send it to an arbitrary
    // custom host merely because it is listed alongside a Vertex transport.
    // Keep only the canonical Vertex host and its official regional variants.
    for base_url in iter_trusted_vertex_base_urls(transports) {
        let url = build_vertex_google_list_url(&base_url, api_key, None);
        let outcome = match fetch_vertex_models_from_url(
            runtime,
            reference_transport,
            &url,
            auth_config,
            "google",
            "gemini:generate_content",
            None,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                hard_errors.push(format!(
                    "vertex google models fetch failed: {}",
                    sanitize_model_fetch_error(&err)
                ));
                continue;
            }
        };
        has_success |= outcome.has_success;
        if let Some(error) = outcome.error {
            let error = sanitize_model_fetch_error(&error);
            if is_soft_not_found(&error) {
                soft_errors.push(format!("vertex google models fetch failed: {error}"));
            } else {
                hard_errors.push(format!("vertex google models fetch failed: {error}"));
            }
            continue;
        }
        all_models.extend(outcome.models);
    }

    let deduped = dedupe_models_by_id_and_format(all_models);
    if !deduped.is_empty() {
        return Ok(build_success_outcome(deduped, None, true).with_errors(hard_errors));
    }

    let errors = if !hard_errors.is_empty() {
        hard_errors
    } else if !soft_errors.is_empty() {
        vec![soft_errors.remove(0)]
    } else {
        Vec::new()
    };
    Ok(ModelsFetchOutcome {
        fetched_model_ids: Vec::new(),
        cached_models: Vec::new(),
        legacy_models: Vec::new(),
        errors,
        has_success,
        native_codex_catalog: false,
        upstream_metadata: None,
        etag: None,
        upstream_status: None,
    })
}

async fn fetch_vertex_service_account_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
    auth_config: Option<&Value>,
) -> Result<ModelsFetchOutcome, String> {
    let Some(auth_config) = auth_config else {
        return Ok(ModelsFetchOutcome {
            fetched_model_ids: Vec::new(),
            cached_models: Vec::new(),
            legacy_models: Vec::new(),
            errors: vec!["vertex_ai(service_account): missing auth_config".to_string()],
            has_success: false,
            native_codex_catalog: false,
            upstream_metadata: None,
            etag: None,
            upstream_status: None,
        });
    };
    let token = exchange_vertex_service_account_token(runtime, &transports[0], auth_config)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    let gemini_transport =
        select_transport_for_api_format(transports, "gemini:").unwrap_or(&transports[0]);
    let claude_transport =
        select_transport_for_api_format(transports, "claude:").unwrap_or(gemini_transport);

    let mut all_models = Vec::new();
    let mut hard_errors = Vec::new();
    let mut soft_errors = Vec::new();
    let mut has_success = false;

    // A service-account access token is a cloud credential.  Restrict its
    // model-garden requests to official Vertex hosts even when an endpoint
    // override exists under the provider.
    for base in iter_trusted_vertex_base_urls(transports) {
        for (publisher, transport, api_format) in [
            ("google", gemini_transport, "gemini:generate_content"),
            ("anthropic", claude_transport, "claude:messages"),
        ] {
            let url = build_vertex_service_account_list_url(&base, publisher, None);
            let outcome = match fetch_vertex_models_from_url(
                runtime,
                transport,
                &url,
                Some(auth_config),
                publisher,
                api_format,
                Some(("authorization".to_string(), format!("Bearer {token}"))),
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(err) => {
                    hard_errors.push(format!(
                        "vertex {publisher} models fetch failed: {}",
                        sanitize_model_fetch_error(&err)
                    ));
                    continue;
                }
            };
            has_success |= outcome.has_success;
            if let Some(error) = outcome.error {
                let error = sanitize_model_fetch_error(&error);
                let labeled = format!("vertex {publisher} models fetch failed: {error}");
                if is_soft_not_found(&error) {
                    soft_errors.push(labeled);
                } else {
                    hard_errors.push(labeled);
                }
                continue;
            }
            all_models.extend(outcome.models);
        }
    }

    let deduped = dedupe_models_by_id_and_format(all_models);
    if !deduped.is_empty() {
        return Ok(build_success_outcome(deduped, None, true).with_errors(hard_errors));
    }

    let errors = if !hard_errors.is_empty() {
        hard_errors
    } else if !soft_errors.is_empty() {
        vec![soft_errors.remove(0)]
    } else {
        Vec::new()
    };
    Ok(ModelsFetchOutcome {
        fetched_model_ids: Vec::new(),
        cached_models: Vec::new(),
        legacy_models: Vec::new(),
        errors,
        has_success,
        native_codex_catalog: false,
        upstream_metadata: None,
        etag: None,
        upstream_status: None,
    })
}

#[derive(Debug)]
struct VertexFetchPageOutcome {
    models: Vec<Value>,
    error: Option<String>,
    has_success: bool,
}

async fn fetch_vertex_models_from_url(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    initial_url: &str,
    auth_config: Option<&Value>,
    fallback_publisher: &str,
    api_format: &str,
    auth_header: Option<(String, String)>,
) -> Result<VertexFetchPageOutcome, String> {
    let mut all_models = Vec::new();
    let mut has_success = false;
    let mut next_page_token = None;

    for _ in 0..VERTEX_MAX_PAGES {
        let url = next_page_token
            .as_deref()
            .map(|token| append_query_param(initial_url.to_string(), "pageToken", token))
            .unwrap_or_else(|| initial_url.to_string());
        let plan = build_vertex_models_fetch_execution_plan(
            runtime,
            transport,
            &url,
            api_format,
            auth_header.clone(),
        )
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
        let result = runtime
            .execute_model_fetch_execution_plan(&plan)
            .await
            .map_err(|error| sanitize_model_fetch_error(&error))?;
        if result.status_code != 200 {
            return Ok(VertexFetchPageOutcome {
                models: Vec::new(),
                error: Some(execution_result_error_message(&result)),
                has_success,
            });
        }

        has_success = true;
        let body_json = execution_result_json_body_allow_empty(&result)
            .map_err(|error| sanitize_model_fetch_error(&error))?;
        all_models.extend(parse_vertex_models_payload(
            &body_json,
            auth_config,
            fallback_publisher,
        ));
        next_page_token = body_json
            .get("nextPageToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if next_page_token.is_none() {
            break;
        }
    }

    Ok(VertexFetchPageOutcome {
        models: all_models,
        error: None,
        has_success,
    })
}

async fn exchange_vertex_service_account_token(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    auth_config: &Value,
) -> Result<String, String> {
    // Reuse the provider transport parser here instead of trusting token_uri
    // from the raw credential JSON.  The parser pins the token endpoint to
    // Google's HTTPS OAuth endpoint and rejects credentials, ports, queries,
    // fragments, and lookalike hosts before a signed assertion is produced.
    let auth_config_json = serde_json::to_string(auth_config)
        .map_err(|_| "vertex_ai(service_account): invalid auth_config".to_string())?;
    let auth_config = parse_vertex_service_account_auth_config(Some(&auth_config_json))
        .ok_or_else(|| "vertex_ai(service_account): invalid auth_config".to_string())?;
    let token_url = auth_config.token_uri;
    let client_email = auth_config.client_email;
    let private_key = auth_config.private_key;
    let now = now_unix_secs();
    let assertion =
        build_vertex_service_account_assertion(&client_email, &private_key, &token_url, now)
            .map_err(|error| sanitize_model_fetch_error(&error))?;
    let body = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
    );
    let transport_profile = resolve_transport_profile(transport);

    let plan = ExecutionPlan {
        request_id: format!("req-model-fetch-{}-vertex-sa-token", transport.key.id),
        candidate_id: None,
        provider_name: Some(transport.provider.name.clone()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "POST".to_string(),
        url: token_url,
        headers: BTreeMap::from([(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )]),
        content_type: Some("application/x-www-form-urlencoded".to_string()),
        content_encoding: None,
        body: RequestBody {
            json_body: None,
            body_bytes_b64: Some(STANDARD.encode(body.as_bytes())),
            body_ref: None,
        },
        stream: false,
        client_api_format: "gemini:generate_content".to_string(),
        provider_api_format: "vertex_ai:service_account_token".to_string(),
        model_name: Some("token".to_string()),
        proxy: runtime.resolve_model_fetch_proxy(transport).await,
        transport_profile,
        timeouts: resolve_transport_execution_timeouts(transport),
    };
    let result = runtime
        .execute_model_fetch_execution_plan(&plan)
        .await
        .map_err(|error| sanitize_model_fetch_error(&error))?;
    let body_json = execution_result_json_body(&result)?;
    body_json
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "vertex_ai(service_account): auth failed: missing access_token".to_string())
}

fn build_vertex_service_account_assertion(
    client_email: &str,
    private_key_pem: &str,
    token_url: &str,
    now_unix_secs: u64,
) -> Result<String, String> {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_string(&json!({
            "iss": client_email,
            "scope": GOOGLE_CLOUD_PLATFORM_SCOPE,
            "aud": token_url,
            "iat": now_unix_secs,
            "exp": now_unix_secs.saturating_add(3600),
        }))
        .map_err(|err| format!("vertex_ai(service_account): jwt payload encode failed: {err}"))?,
    );
    let message = format!("{header}.{payload}");
    let signature =
        rsa_pkcs1_sha256_sign(private_key_pem.as_bytes(), message.as_bytes()).map_err(|error| {
            match error {
                RsaPkcs1Sha256Error::InvalidPrivateKey => {
                    "vertex_ai(service_account): private_key parse failed".to_string()
                }
                _ => "vertex_ai(service_account): signing failed".to_string(),
            }
        })?;
    Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

fn execution_result_json_body(result: &ExecutionResult) -> Result<Value, String> {
    if result.status_code != 200 {
        return Err(execution_result_error_message(result));
    }
    execution_result_json_body_allow_empty(result)
}

fn execution_result_json_body_allow_empty(result: &ExecutionResult) -> Result<Value, String> {
    result
        .body
        .as_ref()
        .and_then(|body| body.json_body.clone())
        .ok_or_else(|| "models fetch response body is missing JSON payload".to_string())
}

fn execution_result_header(result: &ExecutionResult, name: &str) -> Option<String> {
    result
        .headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

const MODEL_FETCH_ERROR_DETAIL_MAX_BYTES: usize = 256;

fn execution_result_error_message(result: &ExecutionResult) -> String {
    let detail = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .and_then(extract_error_message)
        .or_else(|| {
            result.error.as_ref().and_then(|error| {
                let message = error.message.trim();
                (!message.is_empty()).then_some(message.to_string())
            })
        });
    let status = if !(200..300).contains(&result.status_code) {
        Some(result.status_code)
    } else {
        result
            .error
            .as_ref()
            .and_then(|error| error.upstream_status)
            .filter(|status| (400..600).contains(status))
    };
    let summary = model_fetch_error_summary(detail.as_deref(), result.error.as_ref(), status);
    match status {
        Some(status) => format!("HTTP {status}: {summary}"),
        None if detail.is_some() => summary,
        None => format!("HTTP {}: {summary}", result.status_code),
    }
}

/// Projects transport/upstream diagnostics into a bounded message that can cross the
/// model-fetch API boundary.  Upstream error bodies and HTTP client errors are untrusted: they
/// commonly contain authorization headers, credential-bearing URLs, or local file paths.
fn sanitize_model_fetch_error(error: &str) -> String {
    let trimmed = error.trim();
    if trimmed.is_empty() {
        return "upstream request failed".to_string();
    }
    match trimmed {
        "No transport snapshots available for models fetch"
        | "No supported endpoint for Rust models fetch"
        | "Provider transport snapshot unavailable" => return trimmed.to_string(),
        _ => {}
    }

    let status = model_fetch_status_from_text(trimmed);
    if let Some(detail) = sanitize_model_fetch_error_detail(trimmed) {
        if let Some(status) = status {
            let lower = trimmed.to_ascii_lowercase();
            if lower.starts_with("http ")
                || lower.starts_with("status ")
                || lower.starts_with("status=")
                || lower.starts_with("status:")
                || lower.starts_with("status_code=")
                || lower.starts_with("status_code:")
            {
                return format!("HTTP {status}: {detail}");
            }
        }
        return detail;
    }

    let summary = model_fetch_error_summary(Some(trimmed), None, status);
    status
        .map(|status| format!("HTTP {status}: {summary}"))
        .unwrap_or_else(|| summary.to_string())
}

fn model_fetch_error_summary(
    detail: Option<&str>,
    execution_error: Option<&ExecutionError>,
    status: Option<u16>,
) -> String {
    // Authentication, authorization, not-found, timeout, and rate-limit statuses are stable
    // public classifications.  Never let an upstream body replace them with free-form text.
    if matches!(status, Some(401 | 403 | 404 | 408 | 429)) {
        return model_fetch_error_category(detail, execution_error, status).to_string();
    }
    if let Some(detail) = detail.and_then(sanitize_model_fetch_error_detail) {
        return detail;
    }
    model_fetch_error_category(detail, execution_error, status).to_string()
}

fn model_fetch_error_category(
    detail: Option<&str>,
    execution_error: Option<&ExecutionError>,
    status: Option<u16>,
) -> &'static str {
    let lower = detail.unwrap_or_default().to_ascii_lowercase();

    match status {
        Some(401) => return "upstream authentication failed",
        Some(403) => return "upstream authorization failed",
        Some(404) => return "upstream endpoint not found",
        Some(408) => return "upstream request timed out",
        Some(429) => return "upstream rate limited",
        _ => {}
    }

    if let Some(error) = execution_error {
        match &error.kind {
            ExecutionErrorKind::ConnectTimeout
            | ExecutionErrorKind::FirstByteTimeout
            | ExecutionErrorKind::ReadTimeout => return "upstream request timed out",
            ExecutionErrorKind::TlsError => return "upstream TLS connection failed",
            ExecutionErrorKind::ProxyError => return "upstream proxy request failed",
            ExecutionErrorKind::ProtocolError => return "upstream response invalid",
            ExecutionErrorKind::Cancelled => return "upstream request cancelled",
            ExecutionErrorKind::Upstream4xx => return "upstream request rejected",
            ExecutionErrorKind::Upstream5xx => return "upstream service failed",
            ExecutionErrorKind::Internal => {}
        }
    }

    if lower.contains("timeout") || lower.contains("timed out") {
        return "upstream request timed out";
    }
    if lower.contains("unauthorized")
        || lower.contains("authentication")
        || lower.contains("invalid api key")
        || lower.contains("invalid token")
    {
        return "upstream authentication failed";
    }
    if lower.contains("forbidden") || lower.contains("authorization") {
        return "upstream authorization failed";
    }
    if lower.contains("rate limit") || lower.contains("too many requests") {
        return "upstream rate limited";
    }
    if lower.contains("response body")
        || lower.contains("invalid response")
        || lower.contains("malformed")
        || lower.contains("missing models")
        || lower.contains("missing data")
        || lower.contains("conflicting cards")
        || lower.contains("json")
        || lower.contains("parse")
    {
        return "upstream response invalid";
    }
    if lower.contains("connect")
        || lower.contains("connection")
        || lower.contains("network")
        || lower.contains("dns")
        || lower.contains("certificate")
    {
        return "upstream connection failed";
    }

    match status {
        Some(status) if (400..500).contains(&status) => "upstream request rejected",
        Some(status) if (500..600).contains(&status) => "upstream service failed",
        _ => "upstream request failed",
    }
}

fn sanitize_model_fetch_error_detail(detail: &str) -> Option<String> {
    let normalized = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty()
        || normalized.len() > MODEL_FETCH_ERROR_DETAIL_MAX_BYTES
        || model_fetch_error_contains_sensitive_data(&normalized)
    {
        return None;
    }

    let lower = normalized.to_ascii_lowercase();
    if lower.contains("conflicting cards") {
        // The identity is upstream-controlled and the accepted model-ID alphabet also accepts
        // common bearer/API-key encodings. A malicious upstream can reflect the credential it
        // just received as a conflicting model ID, so the API boundary must omit it entirely.
        return Some("conflicting cards".to_string());
    }

    const SAFE_ERROR_PHRASES: &[&str] = &[
        "connection reset",
        "connect timeout",
        "connection timeout",
        "compact endpoint unavailable",
        "temporarily unavailable",
        "missing models array",
        "missing data array",
        "missing project_id",
        "missing clientmodelconfigs",
        "response body is missing json payload",
        "invalid response",
        "no models",
        "endpoint unavailable",
    ];
    SAFE_ERROR_PHRASES
        .iter()
        .find(|phrase| lower.contains(**phrase))
        .map(|phrase| (*phrase).to_string())
}

fn model_fetch_error_contains_sensitive_data(value: &str) -> bool {
    if value.chars().any(|character| character.is_control()) {
        return true;
    }

    let lower = value.to_ascii_lowercase();
    const SENSITIVE_MARKERS: &[&str] = &[
        "authorization",
        "proxy-authorization",
        "bearer",
        "basic ",
        "api_key",
        "api-key",
        "apikey",
        "access_token",
        "access-token",
        "refresh_token",
        "refresh-token",
        "session_token",
        "session-token",
        "sessionkey",
        "password",
        "passwd",
        "private_key",
        "private-key",
        "secret",
        "credential",
        "cookie",
        "set-cookie",
        "token=",
        "token:",
        "key=",
        "key:",
        "url",
        "uri",
        "path=",
    ];
    if SENSITIVE_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return true;
    }

    if lower
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|word| {
            matches!(
                word,
                "token" | "apikey" | "password" | "passwd" | "secret" | "credential"
            )
        })
    {
        return true;
    }

    if lower
        .chars()
        .any(|character| matches!(character, '/' | '\\' | '?' | '#' | '@' | '%'))
    {
        return true;
    }

    // A host, IP literal, or version-like opaque identifier should not cross the boundary as
    // part of an error.  Model IDs do not need to be included in transport diagnostics.
    lower.split_whitespace().any(|token| {
        let token = token.trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && character != '.' && character != '-'
        });
        if token.len() > 64 {
            return true;
        }
        let parts = token.split('.').collect::<Vec<_>>();
        parts.len() >= 2
            && parts.last().is_some_and(|suffix| suffix.len() >= 2)
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            })
    })
}

fn model_fetch_status_from_text(error: &str) -> Option<u16> {
    error
        .split(|character: char| !character.is_ascii_digit())
        .filter(|token| token.len() == 3)
        .find_map(|token| {
            let status = token.parse::<u16>().ok()?;
            (400..600).contains(&status).then_some(status)
        })
}

fn model_fetch_format_label(api_format: &str) -> &'static str {
    let normalized = api_format.trim().to_ascii_lowercase();
    if normalized.starts_with("openai:") {
        "openai"
    } else if normalized.starts_with("claude:") {
        "claude"
    } else if normalized.starts_with("gemini:") {
        "gemini"
    } else {
        "models"
    }
}

fn parse_antigravity_models_response(body: &Value) -> Result<(Vec<Value>, Option<Value>), String> {
    let models_object = body
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| "antigravity: invalid response (missing models)".to_string())?;

    let mut models = Vec::new();
    let mut quota_by_model = serde_json::Map::new();
    for (model_id, model_data) in models_object {
        let model_id = model_id.trim();
        if !antigravity_model_id_is_routable(model_id) {
            continue;
        }
        let model_object = model_data.as_object().cloned().unwrap_or_default();
        let display_name = model_object
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(model_id);
        models.push(json!({
            "id": model_id,
            "object": "model",
            "owned_by": "antigravity",
            "display_name": display_name,
            "api_formats": ["gemini:generate_content"],
        }));

        let quota_payload = build_antigravity_quota_payload(model_object.get("quotaInfo"));
        if !quota_payload.is_empty() {
            quota_by_model.insert(model_id.to_string(), Value::Object(quota_payload));
        }
    }

    let upstream_metadata = (!quota_by_model.is_empty()).then(|| {
        json!({
            "antigravity": {
                "updated_at": now_unix_secs(),
                "quota_by_model": quota_by_model,
            }
        })
    });

    Ok((models, upstream_metadata))
}

pub fn antigravity_model_id_is_routable(model_id: &str) -> bool {
    let model_id = model_id.trim();
    !model_id.is_empty()
        && !ANTIGRAVITY_BLOCKED_MODELS
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(model_id))
}

fn parse_kiro_available_models_response(
    body: &Value,
) -> Result<(Vec<Value>, Option<Value>), String> {
    let items = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "kiro: invalid response (missing models)".to_string())?;

    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for item in items {
        let Some(model_id) = item
            .get("modelId")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if !seen.insert(model_id.to_string()) {
            continue;
        }

        let display_name = item
            .get("modelName")
            .or_else(|| item.get("display_name"))
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(model_id);
        let mut model = item.as_object().cloned().unwrap_or_default();
        model.insert("id".to_string(), Value::String(model_id.to_string()));
        model.insert("object".to_string(), Value::String("model".to_string()));
        model.insert(
            "owned_by".to_string(),
            Value::String(infer_kiro_model_owner(model_id).to_string()),
        );
        model.insert(
            "display_name".to_string(),
            Value::String(display_name.to_string()),
        );
        model.insert(
            "api_formats".to_string(),
            Value::Array(vec![Value::String("claude:messages".to_string())]),
        );
        model.remove("api_format");
        models.push(Value::Object(model));
    }

    let default_model = body.get("defaultModel").and_then(|value| {
        json_string(value.get("modelId")).map(|model_id| {
            json!({
                "model_id": model_id,
                "model_name": json_string(value.get("modelName")),
            })
        })
    });
    let upstream_metadata = default_model.map(|default_model| {
        json!({
            "kiro": {
                "updated_at": now_unix_secs(),
                "default_model": default_model,
            }
        })
    });

    Ok((models, upstream_metadata))
}

fn infer_kiro_model_owner(model_id: &str) -> &'static str {
    let normalized = model_id.trim().to_ascii_lowercase();
    if normalized.starts_with("claude-") {
        "anthropic"
    } else if normalized.starts_with("deepseek-") {
        "deepseek"
    } else if normalized.starts_with("minimax-") {
        "minimax"
    } else if normalized.starts_with("glm-") {
        "zhipu"
    } else if normalized.starts_with("qwen") {
        "alibaba"
    } else {
        "kiro"
    }
}

fn build_antigravity_quota_payload(quota_info: Option<&Value>) -> serde_json::Map<String, Value> {
    let Some(quota_info) = quota_info.and_then(Value::as_object) else {
        return serde_json::Map::new();
    };
    let reset_time = quota_info
        .get("resetTime")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let remaining_fraction = quota_info
        .get("remainingFraction")
        .and_then(|value| {
            value.as_f64().or_else(|| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .and_then(|value| value.parse::<f64>().ok())
            })
        })
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 1.0));

    let mut payload = serde_json::Map::new();
    if let Some(remaining_fraction) = remaining_fraction {
        let used_percent = (1.0 - remaining_fraction) * 100.0;
        payload.insert(
            "remaining_fraction".to_string(),
            Value::from(remaining_fraction),
        );
        payload.insert("used_percent".to_string(), Value::from(used_percent));
    }
    if let Some(reset_time) = reset_time {
        payload.insert("reset_time".to_string(), Value::String(reset_time));
    }
    payload
}

fn should_fallback_antigravity_status(status_code: u16) -> bool {
    matches!(status_code, 404 | 408 | 429) || (500..600).contains(&status_code)
}

fn looks_like_vertex_service_account(auth_config: Option<&Value>) -> bool {
    let Some(auth_config) = auth_config.and_then(Value::as_object) else {
        return false;
    };
    ["client_email", "private_key", "project_id"]
        .into_iter()
        .all(|field| {
            auth_config
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
        })
}

fn iter_vertex_base_urls(transports: &[GatewayProviderTransportSnapshot]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut urls = Vec::new();
    for transport in transports {
        let base_url = transport.endpoint.base_url.trim().trim_end_matches('/');
        if base_url.is_empty() || !seen.insert(base_url.to_string()) {
            continue;
        }
        urls.push(base_url.to_string());
    }
    if seen.insert(VERTEX_API_BASE_URL.to_string()) {
        urls.push(VERTEX_API_BASE_URL.to_string());
    }
    urls
}

fn iter_trusted_vertex_base_urls(transports: &[GatewayProviderTransportSnapshot]) -> Vec<String> {
    iter_vertex_base_urls(transports)
        .into_iter()
        .filter(|base_url| looks_like_vertex_ai_host(base_url))
        .collect()
}

fn build_vertex_google_list_url(base_url: &str, api_key: &str, page_token: Option<&str>) -> String {
    let url = build_vertex_publisher_models_list_base_url(base_url, "google");
    let mut url = append_query_param(url, "key", api_key);
    url = append_query_param(url, "pageSize", VERTEX_PAGE_SIZE);
    if let Some(page_token) = page_token {
        url = append_query_param(url, "pageToken", page_token);
    }
    url
}

fn build_vertex_service_account_list_url(
    base_url: &str,
    publisher: &str,
    page_token: Option<&str>,
) -> String {
    let mut url = build_vertex_publisher_models_list_base_url(base_url, publisher);
    url = append_query_param(url, "pageSize", VERTEX_PAGE_SIZE);
    if let Some(page_token) = page_token {
        url = append_query_param(url, "pageToken", page_token);
    }
    url
}

fn build_vertex_publisher_models_list_base_url(base_url: &str, publisher: &str) -> String {
    let path = format!("/{VERTEX_MODEL_GARDEN_API_VERSION}/publishers/{publisher}/models");
    build_vertex_model_garden_path_url(base_url, &path)
}

fn build_vertex_model_garden_path_url(base_url: &str, path: &str) -> String {
    let base = base_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/v1beta1")
        .trim_end_matches("/v1beta")
        .trim_end_matches("/v1");
    format!("{}{}", base, path.trim())
}

fn parse_vertex_models_payload(
    body: &Value,
    auth_config: Option<&Value>,
    fallback_publisher: &str,
) -> Vec<Value> {
    vertex_payload_items(body)
        .into_iter()
        .filter_map(|item| build_vertex_model(item, auth_config, fallback_publisher))
        .collect()
}

fn vertex_payload_items(body: &Value) -> Vec<&serde_json::Map<String, Value>> {
    if let Some(items) = body.as_array() {
        return items.iter().filter_map(Value::as_object).collect();
    }
    ["publisherModels", "models", "data", "items"]
        .iter()
        .find_map(|key| body.get(*key).and_then(Value::as_array))
        .map(|items| items.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn build_vertex_model(
    item: &serde_json::Map<String, Value>,
    auth_config: Option<&Value>,
    fallback_publisher: &str,
) -> Option<Value> {
    let raw_name = item
        .get("id")
        .or_else(|| item.get("name"))
        .or_else(|| item.get("model"))
        .and_then(Value::as_str)?;
    let model_id = extract_vertex_model_id(raw_name);
    if model_id.is_empty() {
        return None;
    }
    let display_name = item
        .get("displayName")
        .or_else(|| item.get("display_name"))
        .or_else(|| item.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(model_id.as_str())
        .to_string();
    Some(json!({
        "id": model_id,
        "object": "model",
        "owned_by": extract_vertex_publisher(item, fallback_publisher),
        "display_name": display_name,
        "api_formats": [vertex_effective_format(&model_id, auth_config)],
    }))
}

fn extract_vertex_model_id(raw_name: &str) -> String {
    let trimmed = raw_name.trim();
    if let Some((_, suffix)) = trimmed.split_once("/models/") {
        return suffix.trim().to_string();
    }
    trimmed
        .strip_prefix("models/")
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

fn extract_vertex_publisher(
    item: &serde_json::Map<String, Value>,
    fallback_publisher: &str,
) -> String {
    item.get("publisher")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            item.get("name")
                .and_then(Value::as_str)
                .and_then(|name| name.split("/publishers/").nth(1))
                .and_then(|rest| rest.split('/').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| fallback_publisher.to_string())
}

fn vertex_effective_format(model_id: &str, auth_config: Option<&Value>) -> String {
    if let Some(config) = auth_config.and_then(Value::as_object) {
        if let Some(mapping) = config
            .get("model_format_mapping")
            .and_then(Value::as_object)
        {
            if let Some(api_format) = mapping.get(model_id).and_then(Value::as_str) {
                return normalize_api_format(api_format);
            }
            for (prefix, api_format) in mapping {
                if prefix.ends_with('-')
                    && model_id.starts_with(prefix)
                    && api_format.as_str().is_some()
                {
                    return normalize_api_format(
                        api_format.as_str().unwrap_or("gemini:generate_content"),
                    );
                }
            }
        }
        if let Some(default_format) = config.get("default_format").and_then(Value::as_str) {
            let normalized = normalize_api_format(default_format);
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }
    if model_id.starts_with("claude-") {
        "claude:messages".to_string()
    } else {
        "gemini:generate_content".to_string()
    }
}

fn is_soft_not_found(error: &str) -> bool {
    error.trim().starts_with("HTTP 404:")
}

fn dedupe_models_by_id_and_format(models: Vec<Value>) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for model in models {
        let Some(model_id) = model
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let api_format = model
            .get("api_formats")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_str)
            .unwrap_or_default();
        let dedupe_key = format!("{model_id}:{api_format}");
        if !seen.insert(dedupe_key) {
            continue;
        }
        deduped.push(model);
    }
    deduped
}

fn build_success_outcome(
    cached_models: Vec<Value>,
    upstream_metadata: Option<Value>,
    has_success: bool,
) -> ModelsFetchOutcome {
    let legacy_models = cached_models.clone();
    ModelsFetchOutcome {
        fetched_model_ids: collect_model_ids(&cached_models),
        cached_models,
        legacy_models,
        errors: Vec::new(),
        has_success,
        native_codex_catalog: false,
        upstream_metadata,
        etag: None,
        upstream_status: None,
    }
}

fn collect_model_ids(models: &[Value]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    for model in models {
        let Some(model_id) = model
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if seen.insert(model_id.to_string()) {
            ids.push(model_id.to_string());
        }
    }
    ids
}

fn collect_codex_model_ids(models: &[Value]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    models
        .iter()
        .filter_map(codex_model_identity)
        .filter(|model_id| seen.insert((*model_id).to_string()))
        .map(ToOwned::to_owned)
        .collect()
}

fn transport_auth_config(transport: &GatewayProviderTransportSnapshot) -> Option<Value> {
    transport
        .key
        .decrypted_auth_config
        .as_deref()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
}

fn select_transport_for_api_format<'a>(
    transports: &'a [GatewayProviderTransportSnapshot],
    prefix: &str,
) -> Option<&'a GatewayProviderTransportSnapshot> {
    transports.iter().find(|transport| {
        transport
            .endpoint
            .api_format
            .trim()
            .to_ascii_lowercase()
            .starts_with(prefix)
    })
}

fn append_query_param(mut url: String, key: &str, value: &str) -> String {
    if key.trim().is_empty() || value.trim().is_empty() {
        return url;
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    let encoded_key =
        url::form_urlencoded::byte_serialize(key.trim().as_bytes()).collect::<String>();
    let encoded_value =
        url::form_urlencoded::byte_serialize(value.trim().as_bytes()).collect::<String>();
    url.push(separator);
    url.push_str(&encoded_key);
    url.push('=');
    url.push_str(&encoded_value);
    url
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_api_format(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn extract_gemini_cli_plan_type(body: &Value) -> Option<String> {
    for key in ["paidTier", "currentTier"] {
        let Some(tier) = body.get(key) else {
            continue;
        };
        let raw = if let Some(value) = tier.as_str() {
            value.trim().to_string()
        } else if let Some(value) = tier
            .as_object()
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str)
        {
            value.trim().to_string()
        } else if let Some(value) = tier
            .as_object()
            .and_then(|object| object.get("tierType"))
            .and_then(Value::as_str)
        {
            value.trim().to_string()
        } else {
            continue;
        };
        let normalized = raw.trim().to_ascii_lowercase();
        if !normalized.is_empty() {
            return Some(normalized);
        }
    }
    None
}

fn extract_gemini_cli_tier_metadata(body: &Value, key: &str) -> Option<Value> {
    let tier = body.get(key)?;
    if let Some(text) = tier
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(Value::String(text.to_string()));
    }

    let object = tier.as_object()?;
    let mut out = serde_json::Map::new();
    for field in [
        "id",
        "tierType",
        "name",
        "displayName",
        "availableCredits",
        "remainingCredits",
        "consumedCredits",
        "totalCredits",
        "unlimited",
        "hasCredits",
    ] {
        let Some(value) = object.get(field) else {
            continue;
        };
        if value.is_string() || value.is_number() || value.is_boolean() || value.is_null() {
            out.insert(field.to_string(), value.clone());
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

fn extract_cloud_ai_companion_project_id(body: &Value) -> Option<String> {
    let raw = body
        .get("cloudaicompanionProject")
        .or_else(|| body.get("cloudAiCompanionProject"))?;
    if let Some(value) = raw.as_str() {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    raw.as_object()
        .and_then(|object| object.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

trait OutcomeExt {
    fn with_errors(self, errors: Vec<String>) -> Self;
    fn with_etag(self, etag: Option<String>) -> Self;
    fn with_upstream_status(self, upstream_status: Option<u16>) -> Self;
}

impl OutcomeExt for ModelsFetchOutcome {
    fn with_errors(mut self, errors: Vec<String>) -> Self {
        self.errors = errors;
        self
    }

    fn with_etag(mut self, etag: Option<String>) -> Self {
        self.etag = etag;
        self
    }

    fn with_upstream_status(mut self, upstream_status: Option<u16>) -> Self {
        self.upstream_status = upstream_status;
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use aether_contracts::{
        redact_url_for_debug, ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionResult,
        ResponseBody,
    };
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use async_trait::async_trait;
    use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};
    use aws_lc_rs::rsa::{KeyPair as AwsRsaKeyPair, KeySize};
    use aws_lc_rs::signature::{KeyPair as _, UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use base64::Engine as _;
    use serde_json::{json, Value};

    use super::{
        build_vertex_google_list_url, build_vertex_service_account_assertion,
        build_vertex_service_account_list_url, parse_antigravity_models_response,
        parse_codex_models_response_for_request, select_model_fetch_strategy, ModelFetchStrategy,
        ModelFetchStrategyKind,
    };
    use crate::transport::ModelFetchTransportRuntime;
    use crate::{fetch_models_from_transports, fetch_models_from_transports_for_client_version};

    type RouteResult = Result<(u16, Value), String>;
    type ModelFetchRoute = (String, RouteResult);

    #[test]
    fn vertex_assertion_accepts_bare_base64_pkcs8_and_verifies() {
        let key_pair = AwsRsaKeyPair::generate(KeySize::Rsa2048)
            .expect("2048-bit test RSA private key should generate");
        let pkcs8 = AsDer::<Pkcs8V1Der<'static>>::as_der(&key_pair)
            .expect("test RSA private key should encode as PKCS#8");
        let assertion = build_vertex_service_account_assertion(
            "svc@example.iam.gserviceaccount.com",
            &STANDARD.encode(pkcs8.as_ref()),
            "https://oauth2.googleapis.com/token",
            1_700_000_000,
        )
        .expect("bare-base64 PKCS#8 key should sign");
        let parts = assertion.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 3);
        let message = format!("{}.{}", parts[0], parts[1]);
        let signature = URL_SAFE_NO_PAD
            .decode(parts[2])
            .expect("JWT signature should decode");
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key_pair.public_key().as_ref())
            .verify(message.as_bytes(), &signature)
            .expect("AWS-LC signature should verify");
    }

    struct TestRuntime {
        executed_urls: Arc<Mutex<Vec<String>>>,
        response_body: Value,
        status_code: u16,
        response_headers: BTreeMap<String, String>,
    }

    struct RoutingTestRuntime {
        executed_urls: Arc<Mutex<Vec<String>>>,
        routes: Vec<ModelFetchRoute>,
    }

    struct OAuthRoutingTestRuntime {
        executed_urls: Arc<Mutex<Vec<String>>>,
        routes: Vec<ModelFetchRoute>,
    }

    #[async_trait]
    impl ModelFetchTransportRuntime for TestRuntime {
        async fn resolve_local_oauth_request_auth(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Result<Option<aether_provider_transport::LocalResolvedOAuthRequestAuth>, String>
        {
            Ok(None)
        }

        async fn resolve_model_fetch_proxy(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Option<aether_contracts::ProxySnapshot> {
            None
        }

        async fn execute_model_fetch_execution_plan(
            &self,
            plan: &aether_contracts::ExecutionPlan,
        ) -> Result<ExecutionResult, String> {
            self.executed_urls
                .lock()
                .expect("executed_urls lock")
                .push(plan.url.clone());
            Ok(ExecutionResult {
                request_id: plan.request_id.clone(),
                candidate_id: plan.candidate_id.clone(),
                status_code: self.status_code,
                headers: self.response_headers.clone(),
                response_observation: None,
                body: Some(ResponseBody {
                    json_body: Some(self.response_body.clone()),
                    body_bytes_b64: None,
                }),
                telemetry: None,
                error: None,
            })
        }
    }

    #[async_trait]
    impl ModelFetchTransportRuntime for RoutingTestRuntime {
        async fn resolve_local_oauth_request_auth(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Result<Option<aether_provider_transport::LocalResolvedOAuthRequestAuth>, String>
        {
            Ok(None)
        }

        async fn resolve_model_fetch_proxy(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Option<aether_contracts::ProxySnapshot> {
            None
        }

        async fn execute_model_fetch_execution_plan(
            &self,
            plan: &aether_contracts::ExecutionPlan,
        ) -> Result<ExecutionResult, String> {
            self.executed_urls
                .lock()
                .expect("executed_urls lock")
                .push(plan.url.clone());
            let Some((_, route_result)) = self
                .routes
                .iter()
                .find(|(url_part, _)| plan.url.contains(url_part))
            else {
                return Err(format!(
                    "unexpected models fetch URL {}",
                    redact_url_for_debug(&plan.url)
                ));
            };
            let (status_code, response_body) = match route_result {
                Ok((status_code, response_body)) => (*status_code, response_body.clone()),
                Err(err) => return Err(err.clone()),
            };
            Ok(ExecutionResult {
                request_id: plan.request_id.clone(),
                candidate_id: plan.candidate_id.clone(),
                status_code,
                headers: BTreeMap::new(),
                response_observation: None,
                body: Some(ResponseBody {
                    json_body: Some(response_body),
                    body_bytes_b64: None,
                }),
                telemetry: None,
                error: None,
            })
        }
    }

    #[async_trait]
    impl ModelFetchTransportRuntime for OAuthRoutingTestRuntime {
        async fn resolve_local_oauth_request_auth(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Result<Option<aether_provider_transport::LocalResolvedOAuthRequestAuth>, String>
        {
            Ok(Some(
                aether_provider_transport::LocalResolvedOAuthRequestAuth::Header {
                    name: "authorization".to_string(),
                    value: "Bearer oauth-token".to_string(),
                },
            ))
        }

        async fn resolve_model_fetch_proxy(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Option<aether_contracts::ProxySnapshot> {
            None
        }

        async fn execute_model_fetch_execution_plan(
            &self,
            plan: &aether_contracts::ExecutionPlan,
        ) -> Result<ExecutionResult, String> {
            self.executed_urls
                .lock()
                .expect("executed_urls lock")
                .push(plan.url.clone());
            let Some((_, route_result)) = self
                .routes
                .iter()
                .find(|(url_part, _)| plan.url.contains(url_part))
            else {
                return Err(format!(
                    "unexpected models fetch URL {}",
                    redact_url_for_debug(&plan.url)
                ));
            };
            let (status_code, response_body) = match route_result {
                Ok((status_code, response_body)) => (*status_code, response_body.clone()),
                Err(err) => return Err(err.clone()),
            };
            Ok(ExecutionResult {
                request_id: plan.request_id.clone(),
                candidate_id: plan.candidate_id.clone(),
                status_code,
                headers: BTreeMap::new(),
                response_observation: None,
                body: Some(ResponseBody {
                    json_body: Some(response_body),
                    body_bytes_b64: None,
                }),
                telemetry: None,
                error: None,
            })
        }
    }

    fn sample_custom_aiplatform_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Vertex".to_string(),
                provider_type: "custom".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
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
                api_format: "gemini:generate_content".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("generate_content".to_string()),
                is_active: true,
                base_url: "https://aiplatform.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1/publishers/google/models/{model}:{action}".to_string()),
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["gemini:generate_content".to_string()]),
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
                decrypted_api_key: "vertex-secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn sample_codex_transport() -> GatewayProviderTransportSnapshot {
        let mut transport = sample_custom_aiplatform_transport();
        transport.provider.provider_type = "codex".to_string();
        transport.provider.name = "Codex".to_string();
        transport.endpoint.api_format = "openai:responses".to_string();
        transport.endpoint.api_family = Some("openai".to_string());
        transport.endpoint.endpoint_kind = Some("responses".to_string());
        transport.endpoint.base_url = "https://chatgpt.com/backend-api/codex".to_string();
        transport.endpoint.custom_path = Some("/responses".to_string());
        transport.key.api_formats = Some(vec!["openai:responses".to_string()]);
        transport.key.decrypted_api_key = "access-token".to_string();
        transport
    }

    fn sample_codex_transport_for_base(
        endpoint_id: &str,
        base_url: &str,
    ) -> GatewayProviderTransportSnapshot {
        let mut transport = sample_codex_transport();
        transport.endpoint.id = endpoint_id.to_string();
        transport.endpoint.base_url = base_url.to_string();
        transport
    }

    fn sample_kiro_transport() -> GatewayProviderTransportSnapshot {
        let mut transport = sample_custom_aiplatform_transport();
        transport.provider.provider_type = "kiro".to_string();
        transport.provider.name = "Kiro".to_string();
        transport.endpoint.api_format = "claude:messages".to_string();
        transport.endpoint.api_family = Some("claude".to_string());
        transport.endpoint.endpoint_kind = Some("messages".to_string());
        transport.endpoint.base_url = "https://q.{region}.amazonaws.com".to_string();
        transport.endpoint.custom_path = None;
        transport.key.auth_type = "oauth".to_string();
        transport.key.api_formats = Some(vec!["claude:messages".to_string()]);
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config = Some(
            r#"{
                "access_token":"cached-token",
                "expires_at":4102444800,
                "profile_arn":"arn:aws:codewhisperer:us-east-1:123456789012:profile/demo",
                "api_region":"us-east-1",
                "machine_id":"123e4567-e89b-12d3-a456-426614174000"
            }"#
            .to_string(),
        );
        transport
    }

    fn sample_gemini_cli_transport() -> GatewayProviderTransportSnapshot {
        let mut transport = sample_custom_aiplatform_transport();
        transport.provider.provider_type = "gemini_cli".to_string();
        transport.provider.name = "Gemini CLI".to_string();
        transport.endpoint.base_url = "https://cloudcode-pa.googleapis.com".to_string();
        transport.key.auth_type = "bearer".to_string();
        transport.key.decrypted_api_key = "gemini-cli-access-token".to_string();
        transport
    }

    fn sample_antigravity_transport_without_project() -> GatewayProviderTransportSnapshot {
        let mut transport = sample_custom_aiplatform_transport();
        transport.provider.provider_type = "antigravity".to_string();
        transport.provider.name = "Antigravity".to_string();
        transport.endpoint.base_url = "https://daily-cloudcode-pa.googleapis.com".to_string();
        transport.key.auth_type = "oauth".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.decrypted_auth_config =
            Some(r#"{"provider_type":"antigravity","refresh_token":"rt"}"#.to_string());
        transport
    }

    fn sample_windsurf_transport() -> GatewayProviderTransportSnapshot {
        let mut transport = sample_custom_aiplatform_transport();
        transport.provider.provider_type = "windsurf".to_string();
        transport.provider.name = "Windsurf".to_string();
        transport.endpoint.api_format = "openai:chat".to_string();
        transport.endpoint.api_family = Some("openai".to_string());
        transport.endpoint.endpoint_kind = Some("chat".to_string());
        transport.endpoint.base_url = "https://server.codeium.com".to_string();
        transport.endpoint.custom_path = None;
        transport.key.auth_type = "oauth".to_string();
        transport.key.api_formats = Some(vec!["openai:chat".to_string()]);
        transport.key.decrypted_api_key = "devin-session-token$abc".to_string();
        transport.key.decrypted_auth_config = Some(r#"{"provider_type":"windsurf"}"#.to_string());
        transport
    }

    fn sample_openai_transport(
        endpoint_id: &str,
        api_format: &str,
        base_url: &str,
    ) -> GatewayProviderTransportSnapshot {
        let mut transport = sample_custom_aiplatform_transport();
        transport.provider.provider_type = "custom".to_string();
        transport.provider.name = "OpenAI Compat".to_string();
        transport.endpoint.id = endpoint_id.to_string();
        transport.endpoint.api_format = api_format.to_string();
        transport.endpoint.api_family = Some("openai".to_string());
        transport.endpoint.endpoint_kind = api_format
            .split_once(':')
            .map(|(_, endpoint_kind)| endpoint_kind.to_string());
        transport.endpoint.base_url = base_url.to_string();
        transport.endpoint.custom_path = None;
        transport.key.api_formats = Some(vec![api_format.to_string()]);
        transport.key.decrypted_api_key = "openai-secret".to_string();
        transport
    }

    #[test]
    fn strategy_selection_keeps_codex_on_standard_transport_fetch() {
        let strategy = select_model_fetch_strategy(&[sample_codex_transport()])
            .expect("strategy should select");

        assert_eq!(strategy.provider_id(), "codex");
        assert_eq!(strategy.kind(), ModelFetchStrategyKind::StandardTransport);
    }

    #[test]
    fn unversioned_codex_parser_accepts_top_level_openai_compatible_array() {
        let parsed = parse_codex_models_response_for_request(
            "openai:responses",
            &json!([{"id": "gpt-array-compatible"}]),
            None,
        )
        .expect("unversioned admin fetch should retain the top-level array fallback");

        assert_eq!(parsed.fetched_model_ids, vec!["gpt-array-compatible"]);
        assert_eq!(
            parsed.cached_models[0]["api_formats"],
            json!(["openai:responses"])
        );

        let error = parse_codex_models_response_for_request(
            "openai:responses",
            &json!([{"id": "gpt-array-compatible"}]),
            Some("0.145.2"),
        )
        .expect_err("versioned catalogs must use the opaque models-array schema");
        assert!(error.contains("missing models array"));
    }

    #[test]
    fn strategy_selection_uses_preset_catalog_for_claude_code() {
        let mut transport = sample_custom_aiplatform_transport();
        transport.provider.provider_type = "claude_code".to_string();
        transport.endpoint.api_format = "claude:messages".to_string();

        let strategy = select_model_fetch_strategy(&[transport]).expect("strategy should select");

        assert_eq!(strategy.provider_id(), "claude_code");
        assert_eq!(strategy.kind(), ModelFetchStrategyKind::PresetCatalog);
    }

    #[test]
    fn strategy_selection_uses_kiro_upstream_fetch() {
        let strategy = select_model_fetch_strategy(&[sample_kiro_transport()])
            .expect("strategy should select");

        assert_eq!(strategy.provider_id(), "kiro");
        assert_eq!(strategy.kind(), ModelFetchStrategyKind::Kiro);
    }

    #[test]
    fn strategy_selection_uses_windsurf_model_configs_fetch() {
        let strategy = select_model_fetch_strategy(&[sample_windsurf_transport()])
            .expect("strategy should select");

        assert_eq!(strategy.provider_id(), "windsurf");
        assert_eq!(strategy.kind(), ModelFetchStrategyKind::Windsurf);
    }

    #[tokio::test]
    async fn custom_aiplatform_transport_uses_vertex_models_fetch_path_and_normalizes_chat_format()
    {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            response_body: json!({
                "models": [{
                    "name": "publishers/google/models/gemini-3.1-pro-preview"
                }]
            }),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };
        let outcome =
            fetch_models_from_transports(&runtime, &[sample_custom_aiplatform_transport()])
                .await
                .expect("models fetch should succeed");

        let urls = executed_urls.lock().expect("executed_urls lock");
        assert_eq!(
            urls.as_slice(),
            &["https://aiplatform.googleapis.com/v1beta1/publishers/google/models?key=vertex-secret&pageSize=100"]
        );
        assert_eq!(outcome.fetched_model_ids, vec!["gemini-3.1-pro-preview"]);
        assert_eq!(outcome.cached_models.len(), 1);
        assert_eq!(
            outcome.cached_models[0]["api_formats"][0].as_str(),
            Some("gemini:generate_content")
        );
    }

    #[tokio::test]
    async fn standard_transport_merges_successful_endpoint_models_when_one_endpoint_fails() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = RoutingTestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            routes: vec![
                (
                    "https://bad.example.com/models".to_string(),
                    Err("connection reset".to_string()),
                ),
                (
                    "https://chat.example.com/models".to_string(),
                    Ok((
                        200,
                        json!({
                            "data": [{ "id": "shared-model" }]
                        }),
                    )),
                ),
                (
                    "https://responses.example.com/models".to_string(),
                    Ok((
                        200,
                        json!({
                            "data": [
                                { "id": "shared-model" },
                                { "id": "responses-only" }
                            ]
                        }),
                    )),
                ),
            ],
        };
        let transports = vec![
            sample_openai_transport("endpoint-bad", "openai:chat", "https://bad.example.com"),
            sample_openai_transport("endpoint-chat", "openai:chat", "https://chat.example.com"),
            sample_openai_transport(
                "endpoint-responses",
                "openai:responses",
                "https://responses.example.com",
            ),
        ];

        let outcome = fetch_models_from_transports(&runtime, &transports)
            .await
            .expect("models fetch should keep successful endpoint results");

        assert!(outcome.has_success);
        assert_eq!(
            outcome.fetched_model_ids,
            vec!["responses-only", "shared-model"]
        );
        assert_eq!(outcome.cached_models.len(), 2);
        assert_eq!(outcome.legacy_models, outcome.cached_models);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("connection reset"));
        let shared_model = outcome
            .cached_models
            .iter()
            .find(|model| model.get("id").and_then(Value::as_str) == Some("shared-model"))
            .expect("shared model should be cached once");
        assert_eq!(
            shared_model.get("api_formats"),
            Some(&json!(["openai:chat", "openai:responses"]))
        );
    }

    #[tokio::test]
    async fn vertex_models_fetch_continues_when_one_base_url_errors() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = RoutingTestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            routes: vec![
                (
                    "https://us-central1-aiplatform.googleapis.com/v1beta1/publishers/google/models"
                        .to_string(),
                    Err("connect timeout".to_string()),
                ),
                (
                    "https://aiplatform.googleapis.com/v1beta1/publishers/google/models".to_string(),
                    Ok((
                        200,
                        json!({
                            "models": [{
                                "name": "publishers/google/models/gemini-3.1-pro-preview"
                            }]
                        }),
                    )),
                ),
            ],
        };
        let mut failing_transport = sample_custom_aiplatform_transport();
        failing_transport.endpoint.base_url =
            "https://us-central1-aiplatform.googleapis.com".to_string();
        let mut successful_transport = sample_custom_aiplatform_transport();
        successful_transport.endpoint.id = "endpoint-2".to_string();
        successful_transport.endpoint.base_url = "https://aiplatform.googleapis.com".to_string();

        let outcome =
            fetch_models_from_transports(&runtime, &[failing_transport, successful_transport])
                .await
                .expect("vertex models fetch should keep successful base URL results");

        assert!(outcome.has_success);
        assert_eq!(outcome.fetched_model_ids, vec!["gemini-3.1-pro-preview"]);
        assert_eq!(outcome.cached_models.len(), 1);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("connect timeout"));
    }

    #[test]
    fn execution_result_error_projection_discards_body_credentials_and_urls() {
        const BEARER_SECRET: &str = "super-secret-bearer";
        const QUERY_SECRET: &str = "query-secret";
        let result = ExecutionResult {
            request_id: "req-model-fetch-security-body".to_string(),
            candidate_id: None,
            status_code: 401,
            headers: BTreeMap::new(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: Some(json!({
                    "error": {
                        "message": format!(
                            "Authorization: Bearer {BEARER_SECRET}; https://user:pass@example.test/v1/models?key={QUERY_SECRET}"
                        )
                    }
                })),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        };

        let projected = super::execution_result_error_message(&result);
        assert_eq!(projected, "HTTP 401: upstream authentication failed");
        for secret in [
            BEARER_SECRET,
            QUERY_SECRET,
            "Authorization",
            "Bearer",
            "user",
            "pass",
            "example.test",
            "/v1/models",
        ] {
            assert!(
                !projected.contains(secret),
                "error projection leaked {secret}"
            );
        }
    }

    #[test]
    fn execution_result_error_projection_discards_execution_error_message() {
        const SECRET: &str = "transport-secret";
        let result = ExecutionResult {
            request_id: "req-model-fetch-security-error".to_string(),
            candidate_id: None,
            status_code: 502,
            headers: BTreeMap::new(),
            response_observation: None,
            body: None,
            telemetry: None,
            error: Some(ExecutionError {
                kind: ExecutionErrorKind::Upstream5xx,
                phase: ExecutionPhase::Connect,
                message: format!(
                    "connection failed for https://user:pass@example.test/v1/models?token={SECRET}"
                ),
                upstream_status: Some(502),
                retryable: true,
                failover_recommended: true,
            }),
        };

        let projected = super::execution_result_error_message(&result);
        assert_eq!(projected, "HTTP 502: upstream service failed");
        for secret in [SECRET, "https://", "user", "pass", "example.test", "token"] {
            assert!(
                !projected.contains(secret),
                "error projection leaked {secret}"
            );
        }
    }

    #[test]
    fn model_fetch_transport_error_projection_discards_urls_and_credentials() {
        let projected = super::sanitize_model_fetch_error(
            "connection failed for https://user:password@example.test/v1/models?key=query-secret; Authorization: Bearer transport-secret",
        );
        assert_eq!(projected, "upstream authorization failed");
        for secret in [
            "user",
            "password",
            "query-secret",
            "transport-secret",
            "Bearer",
            "example.test",
        ] {
            assert!(
                !projected.contains(secret),
                "error projection leaked {secret}"
            );
        }
    }

    #[test]
    fn model_fetch_error_projection_drops_reflected_secret_like_model_identity() {
        const REFLECTED_API_KEY: &str = "sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
        let projected = super::sanitize_model_fetch_error(&format!(
            "Codex models response contains conflicting cards for identity '{REFLECTED_API_KEY}'"
        ));

        assert_eq!(projected, "conflicting cards");
        assert!(!projected.contains(REFLECTED_API_KEY));
    }

    #[tokio::test]
    async fn vertex_service_account_rejects_untrusted_token_uri_before_signing() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            response_body: json!({}),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };
        let transport = sample_custom_aiplatform_transport();
        let auth_config = json!({
            "client_email": "svc@example.iam.gserviceaccount.com",
            "private_key": "not-a-real-key",
            "project_id": "project-1",
            "token_uri": "https://attacker.example/token"
        });

        let error =
            super::exchange_vertex_service_account_token(&runtime, &transport, &auth_config)
                .await
                .expect_err("untrusted token URI must be rejected");
        assert_eq!(error, "vertex_ai(service_account): invalid auth_config");
        assert!(executed_urls.lock().expect("executed_urls lock").is_empty());
    }

    #[test]
    fn vertex_service_account_model_fetch_ignores_untrusted_endpoint_bases() {
        let mut untrusted = sample_custom_aiplatform_transport();
        untrusted.endpoint.base_url = "https://attacker.example".to_string();
        assert_eq!(
            super::iter_trusted_vertex_base_urls(&[untrusted]),
            vec!["https://aiplatform.googleapis.com".to_string()]
        );
    }

    #[tokio::test]
    async fn vertex_api_key_model_fetch_uses_only_trusted_endpoint_bases() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            response_body: json!({"models": []}),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };
        let mut untrusted = sample_custom_aiplatform_transport();
        untrusted.provider.provider_type = "vertex_ai".to_string();
        untrusted.endpoint.base_url = "https://attacker.example".to_string();

        fetch_models_from_transports(&runtime, &[untrusted])
            .await
            .expect("Vertex API-key model fetch should use the canonical fallback");

        let urls = executed_urls.lock().expect("executed_urls lock");
        assert_eq!(urls.len(), 1);
        assert!(urls[0].starts_with("https://aiplatform.googleapis.com/"));
        assert!(urls[0].contains("key=vertex-secret"));
        assert!(!urls[0].contains("attacker.example"));
    }

    #[test]
    fn pagination_query_values_are_percent_encoded() {
        assert_eq!(
            super::append_query_param(
                "https://aiplatform.googleapis.com/v1beta1/models?key=secret".to_string(),
                "pageToken",
                "cursor&next=1#fragment",
            ),
            "https://aiplatform.googleapis.com/v1beta1/models?key=secret&pageToken=cursor%26next%3D1%23fragment"
        );
    }

    #[test]
    fn vertex_model_fetch_uses_model_garden_list_endpoint() {
        assert_eq!(
            build_vertex_google_list_url(
                "https://aiplatform.googleapis.com",
                "vertex-secret",
                None,
            ),
            "https://aiplatform.googleapis.com/v1beta1/publishers/google/models?key=vertex-secret&pageSize=100"
        );
        assert_eq!(
            build_vertex_service_account_list_url(
                "https://aiplatform.googleapis.com",
                "google",
                Some("page-2"),
            ),
            "https://aiplatform.googleapis.com/v1beta1/publishers/google/models?pageSize=100&pageToken=page-2"
        );
    }

    #[test]
    fn vertex_publisher_models_list_url_uses_model_garden_resource_not_runtime_resource() {
        let url = super::build_vertex_service_account_list_url(
            "https://aiplatform.googleapis.com",
            "google",
            None,
        );

        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1beta1/publishers/google/models?pageSize=100"
        );
        assert!(
            !url.contains("/projects/") && !url.contains("/locations/"),
            "Model Garden publisher list must not use Vertex runtime project/location path"
        );
    }

    #[test]
    fn vertex_service_account_fetches_model_garden_publishers_without_project_prefix() {
        assert_eq!(
            super::build_vertex_service_account_list_url(
                "https://us-central1-aiplatform.googleapis.com",
                "google",
                None
            ),
            "https://us-central1-aiplatform.googleapis.com/v1beta1/publishers/google/models?pageSize=100"
        );
        assert_eq!(
            super::build_vertex_service_account_list_url(
                "https://aiplatform.googleapis.com/v1",
                "anthropic",
                Some("next")
            ),
            "https://aiplatform.googleapis.com/v1beta1/publishers/anthropic/models?pageSize=100&pageToken=next"
        );
        assert_eq!(
            super::build_vertex_google_list_url(
                "https://aiplatform.googleapis.com/v1beta1",
                "vertex-secret",
                Some("next")
            ),
            "https://aiplatform.googleapis.com/v1beta1/publishers/google/models?key=vertex-secret&pageSize=100&pageToken=next"
        );
    }

    #[tokio::test]
    async fn codex_transport_fetches_upstream_models_instead_of_preset_catalog() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            response_body: json!({
                "models": [{
                    "id": "gpt-5.6-future",
                    "slug": "gpt-5.6-future",
                    "api_format": "opaque-future-field",
                    "default_reasoning_level": "high",
                    "supported_reasoning_levels": [{"effort": "high"}],
                    "future_capability": {"mode": "preserve-me"}
                }]
            }),
            status_code: 200,
            response_headers: BTreeMap::from([(
                "ETag".to_string(),
                "\"codex-models-0.145.2\"".to_string(),
            )]),
        };
        let outcome = fetch_models_from_transports_for_client_version(
            &runtime,
            &[sample_codex_transport()],
            Some("0.145.2"),
        )
        .await
        .expect("models fetch should succeed");

        let urls = executed_urls.lock().expect("executed_urls lock");
        assert_eq!(
            urls.as_slice(),
            &["https://chatgpt.com/backend-api/codex/models?client_version=0.145.2"]
        );
        assert_eq!(outcome.etag.as_deref(), Some("\"codex-models-0.145.2\""));
        assert_eq!(outcome.upstream_status, Some(200));
        assert_eq!(outcome.fetched_model_ids, vec!["gpt-5.6-future"]);
        assert_eq!(outcome.cached_models.len(), 1);
        assert_eq!(
            outcome.cached_models[0]["api_format"],
            "opaque-future-field"
        );
        assert!(outcome.cached_models[0].get("api_formats").is_none());
        assert_eq!(outcome.legacy_models.len(), 1);
        assert_eq!(outcome.legacy_models[0]["id"], "gpt-5.6-future");
        assert_eq!(
            outcome.legacy_models[0]["api_formats"],
            json!(["openai:responses"])
        );
        assert_eq!(
            outcome.legacy_models[0]["api_format"],
            "opaque-future-field"
        );
        let card = &outcome
            .upstream_metadata
            .as_ref()
            .expect("Codex model catalog metadata")["codex_models"]["cards"]["gpt-5.6-future"];
        assert_eq!(card["default_reasoning_level"], "high");
        assert_eq!(card["future_capability"]["mode"], "preserve-me");
    }

    #[tokio::test]
    async fn codex_transport_reports_slug_only_ids_without_rewriting_opaque_cards() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let card = json!({
            "slug": "gpt-slug-only-future",
            "model_messages": {"instructions_template": "Slug-only instructions"},
            "future_capability": {"opaque": true}
        });
        let runtime = TestRuntime {
            executed_urls,
            response_body: json!({"models": [card.clone()]}),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };

        let outcome = fetch_models_from_transports_for_client_version(
            &runtime,
            &[sample_codex_transport()],
            Some("0.145.2"),
        )
        .await
        .expect("slug-only Codex card should fetch");

        assert_eq!(outcome.fetched_model_ids, vec!["gpt-slug-only-future"]);
        assert_eq!(outcome.cached_models, vec![card]);
        assert!(outcome.cached_models[0].get("id").is_none());
        assert_eq!(outcome.legacy_models[0]["id"], "gpt-slug-only-future");
        assert_eq!(
            outcome.legacy_models[0]["api_formats"],
            json!(["openai:responses"])
        );
        assert_eq!(
            outcome.legacy_models[0]["future_capability"]["opaque"],
            true
        );
    }

    #[tokio::test]
    async fn codex_legacy_projection_merges_only_formats_from_successful_transports() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let card = json!({
            "slug": "gpt-multi-format-future",
            "api_format": "opaque-upstream-protocol",
            "future_capability": {"opaque": true}
        });
        let runtime = RoutingTestRuntime {
            executed_urls,
            routes: vec![
                (
                    "chat.example.com/backend-api/codex/models".to_string(),
                    Ok((200, json!({"models": [card.clone()]}))),
                ),
                (
                    "responses.example.com/backend-api/codex/models".to_string(),
                    Ok((200, json!({"models": [card.clone()]}))),
                ),
                (
                    "compact.example.com/backend-api/codex/models".to_string(),
                    Err("compact endpoint unavailable".to_string()),
                ),
            ],
        };
        let mut chat = sample_codex_transport_for_base(
            "endpoint-chat",
            "https://chat.example.com/backend-api/codex",
        );
        chat.endpoint.api_format = "openai:chat".to_string();
        let responses = sample_codex_transport_for_base(
            "endpoint-responses",
            "https://responses.example.com/backend-api/codex",
        );
        let mut compact = sample_codex_transport_for_base(
            "endpoint-compact",
            "https://compact.example.com/backend-api/codex",
        );
        compact.endpoint.api_format = "openai:responses:compact".to_string();

        let outcome = fetch_models_from_transports_for_client_version(
            &runtime,
            &[chat, responses, compact],
            Some("0.145.2"),
        )
        .await
        .expect("successful endpoint catalogs should survive a sibling failure");

        assert_eq!(outcome.cached_models, vec![card]);
        assert_eq!(outcome.legacy_models.len(), 1);
        assert_eq!(
            outcome.legacy_models[0]["api_formats"],
            json!(["openai:chat", "openai:responses"])
        );
        assert_eq!(
            outcome.legacy_models[0]["api_format"],
            "opaque-upstream-protocol"
        );
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("compact endpoint unavailable"));
    }

    #[tokio::test]
    async fn unversioned_codex_admin_fetch_keeps_openai_compatible_data_fallback() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls,
            response_body: json!({
                "data": [{
                    "id": "gpt-legacy-compatible",
                    "future_capability": {"preserved": true}
                }]
            }),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };

        let outcome = fetch_models_from_transports(&runtime, &[sample_codex_transport()])
            .await
            .expect("unversioned admin fetch should retain the generic parser fallback");

        assert!(outcome.has_success);
        assert_eq!(outcome.fetched_model_ids, vec!["gpt-legacy-compatible"]);
        assert_eq!(outcome.cached_models[0]["id"], "gpt-legacy-compatible");
        assert!(!outcome.native_codex_catalog);
        let versioned = crate::fetch_models_from_transports_for_management(
            &runtime,
            &[sample_codex_transport()],
            Some("0.153.3"),
        )
        .await
        .expect("management retains data-array compatibility with a new fingerprint");
        assert!(versioned.has_success);
        assert!(
            !versioned.native_codex_catalog,
            "generic responses cannot replace opaque catalogs"
        );
        assert_eq!(versioned.fetched_model_ids, outcome.fetched_model_ids);
        assert_eq!(
            outcome.legacy_models[0]["api_formats"],
            json!(["openai:responses"])
        );
    }

    #[tokio::test]
    async fn versioned_codex_catalog_does_not_accept_openai_compatible_data_fallback() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls,
            response_body: json!({"data": [{"id": "gpt-not-an-opaque-card"}]}),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };

        let outcome = fetch_models_from_transports_for_client_version(
            &runtime,
            &[sample_codex_transport()],
            Some("0.145.2"),
        )
        .await
        .expect("transport failures are returned as observable outcomes");

        assert!(!outcome.has_success);
        assert!(outcome.cached_models.is_empty());
        assert!(outcome.legacy_models.is_empty());
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("missing models array"));
    }

    #[tokio::test]
    async fn codex_transport_merges_exact_duplicate_cards_across_endpoints() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let card = json!({
            "id": "gpt-exact-duplicate",
            "slug": "gpt-exact-duplicate",
            "model_messages": {"instructions_template": "Opaque instructions"},
            "future_capability": {"opaque": true}
        });
        let runtime = RoutingTestRuntime {
            executed_urls,
            routes: vec![
                (
                    "first.example.com/backend-api/codex/models".to_string(),
                    Ok((200, json!({"models": [card.clone()]}))),
                ),
                (
                    "second.example.com/backend-api/codex/models".to_string(),
                    Ok((200, json!({"models": [card.clone()]}))),
                ),
            ],
        };
        let transports = vec![
            sample_codex_transport_for_base(
                "endpoint-first",
                "https://first.example.com/backend-api/codex",
            ),
            sample_codex_transport_for_base(
                "endpoint-second",
                "https://second.example.com/backend-api/codex",
            ),
        ];

        let outcome =
            fetch_models_from_transports_for_client_version(&runtime, &transports, Some("0.145.2"))
                .await
                .expect("exact duplicate endpoint catalogs should merge");

        assert!(outcome.has_success);
        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.fetched_model_ids, vec!["gpt-exact-duplicate"]);
        assert_eq!(outcome.cached_models, vec![card]);
    }

    #[tokio::test]
    async fn codex_transport_rejects_cross_identity_conflicts_across_endpoints() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = RoutingTestRuntime {
            executed_urls,
            routes: vec![
                (
                    "first.example.com/backend-api/codex/models".to_string(),
                    Ok((
                        200,
                        json!({
                            "models": [{
                                "id": "gpt-id-one",
                                "slug": "gpt-cross-identity",
                                "future_capability": {"source": "first"}
                            }]
                        }),
                    )),
                ),
                (
                    "second.example.com/backend-api/codex/models".to_string(),
                    Ok((
                        200,
                        json!({
                            "models": [{
                                "id": "gpt-cross-identity",
                                "slug": "gpt-slug-two",
                                "future_capability": {"source": "second"}
                            }]
                        }),
                    )),
                ),
            ],
        };
        let transports = vec![
            sample_codex_transport_for_base(
                "endpoint-first",
                "https://first.example.com/backend-api/codex",
            ),
            sample_codex_transport_for_base(
                "endpoint-second",
                "https://second.example.com/backend-api/codex",
            ),
        ];

        let error =
            fetch_models_from_transports_for_client_version(&runtime, &transports, Some("0.145.2"))
                .await
                .expect_err("conflicting endpoint catalogs must fail");

        assert_eq!(error, "conflicting cards");
        assert!(!error.contains("gpt-cross-identity"));
    }

    #[tokio::test]
    async fn codex_transport_reports_non_success_upstream_status() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls,
            response_body: json!({
                "error": { "message": "temporarily unavailable" }
            }),
            status_code: 503,
            response_headers: BTreeMap::new(),
        };

        let outcome = fetch_models_from_transports_for_client_version(
            &runtime,
            &[sample_codex_transport()],
            Some("0.145.2"),
        )
        .await
        .expect("models fetch should return an observable failed outcome");

        assert!(!outcome.has_success);
        assert_eq!(outcome.upstream_status, Some(503));
        assert_eq!(outcome.etag, None);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("HTTP 503: temporarily unavailable"));
    }

    #[tokio::test]
    async fn gemini_cli_load_code_assist_preserves_paid_tier_credits() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            response_body: json!({
                "cloudaicompanionProject": {
                    "id": "project-from-load-code-assist"
                },
                "currentTier": {
                    "id": "free-tier"
                },
                "paidTier": {
                    "id": "g1-pro-tier",
                    "availableCredits": 123.5,
                    "consumedCredits": 7,
                    "totalCredits": 200,
                    "privateField": {
                        "ignored": true
                    }
                }
            }),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };
        let outcome = fetch_models_from_transports(&runtime, &[sample_gemini_cli_transport()])
            .await
            .expect("models fetch should succeed");

        let urls = executed_urls.lock().expect("executed_urls lock");
        assert_eq!(
            urls.as_slice(),
            &["https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist"]
        );
        assert_eq!(
            outcome
                .upstream_metadata
                .as_ref()
                .and_then(|value| value.pointer("/gemini_cli/project_id")),
            Some(&json!("project-from-load-code-assist"))
        );
        assert_eq!(
            outcome
                .upstream_metadata
                .as_ref()
                .and_then(|value| value.pointer("/gemini_cli/plan_type")),
            Some(&json!("g1-pro-tier"))
        );
        assert_eq!(
            outcome
                .upstream_metadata
                .as_ref()
                .and_then(|value| value.pointer("/gemini_cli/paidTier/availableCredits")),
            Some(&json!(123.5))
        );
        assert!(outcome
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.pointer("/gemini_cli/paidTier/privateField"))
            .is_none());
    }

    #[tokio::test]
    async fn antigravity_model_fetch_hydrates_project_from_prod_load_code_assist() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = OAuthRoutingTestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            routes: vec![
                (
                    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist".to_string(),
                    Ok((
                        200,
                        json!({
                            "cloudaicompanionProject": {
                                "id": "project-from-antigravity-load"
                            }
                        }),
                    )),
                ),
                (
                    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels"
                        .to_string(),
                    Ok((
                        200,
                        json!({
                            "models": {
                                "chat_12345": {
                                    "displayName": "Antigravity Chat",
                                    "quotaInfo": {
                                        "remainingFraction": 0.75
                                    }
                                }
                            }
                        }),
                    )),
                ),
            ],
        };

        let outcome = fetch_models_from_transports(
            &runtime,
            &[sample_antigravity_transport_without_project()],
        )
        .await
        .expect("antigravity models fetch should hydrate project and succeed");

        let urls = executed_urls.lock().expect("executed_urls lock");
        assert_eq!(
            urls.as_slice(),
            &[
                "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
                "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
            ]
        );
        assert_eq!(outcome.fetched_model_ids, vec!["chat_12345"]);
        assert_eq!(
            outcome
                .upstream_metadata
                .as_ref()
                .and_then(|value| value.pointer("/antigravity/project_id")),
            Some(&json!("project-from-antigravity-load"))
        );
        assert_eq!(
            outcome
                .upstream_metadata
                .as_ref()
                .and_then(|value| value
                    .pointer("/antigravity/quota_by_model/chat_12345/remaining_fraction")),
            Some(&json!(0.75))
        );
    }

    #[test]
    fn antigravity_models_without_explicit_quota_are_not_marked_exhausted() {
        let (models, metadata) = parse_antigravity_models_response(&json!({
            "models": {
                "gemini-3.7-flash-tiered": {
                    "displayName": "Gemini 3.7 Flash"
                },
                "gemini-3.7-flash-high": {
                    "displayName": "Gemini 3.7 Flash High",
                    "quotaInfo": {
                        "remainingFraction": "0.75",
                        "resetTime": "2030-01-01T00:00:00Z"
                    }
                },
                "gemini-3.7-flash-low": {
                    "displayName": "Gemini 3.7 Flash Low",
                    "quotaInfo": {
                        "remainingFraction": 0.0
                    }
                }
            }
        }))
        .expect("Antigravity models should parse");

        assert_eq!(models.len(), 3);
        let metadata = metadata.expect("explicit quota should produce metadata");
        let antigravity = &metadata["antigravity"];
        assert!(antigravity["quota_by_model"]
            .get("gemini-3.7-flash-tiered")
            .is_none());
        assert_eq!(
            antigravity["quota_by_model"]["gemini-3.7-flash-high"]["remaining_fraction"],
            json!(0.75)
        );
        assert_eq!(
            antigravity["quota_by_model"]["gemini-3.7-flash-high"]["used_percent"],
            json!(25.0)
        );
        assert_eq!(
            antigravity["quota_by_model"]["gemini-3.7-flash-low"]["used_percent"],
            json!(100.0)
        );
    }

    #[tokio::test]
    async fn kiro_transport_fetches_list_available_models() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            response_body: json!({
                "defaultModel": {
                    "modelId": "auto",
                    "modelName": "Auto"
                },
                "models": [
                    {
                        "modelId": "auto",
                        "modelName": "Auto",
                        "tokenLimits": {
                            "maxInputTokens": 1000000,
                            "maxOutputTokens": 64000
                        }
                    },
                    {
                        "modelId": "claude-opus-4.7",
                        "modelName": "Claude Opus 4.7",
                        "description": "Experimental preview"
                    }
                ]
            }),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };
        let outcome = fetch_models_from_transports(&runtime, &[sample_kiro_transport()])
            .await
            .expect("models fetch should succeed");

        let urls = executed_urls.lock().expect("executed_urls lock");
        assert_eq!(
            urls.as_slice(),
            &["https://q.us-east-1.amazonaws.com/ListAvailableModels?origin=AI_EDITOR"]
        );
        assert_eq!(
            outcome.fetched_model_ids,
            vec!["auto".to_string(), "claude-opus-4.7".to_string()]
        );
        assert_eq!(outcome.cached_models.len(), 2);
        assert_eq!(
            outcome.cached_models[1]["display_name"].as_str(),
            Some("Claude Opus 4.7")
        );
        assert_eq!(
            outcome.cached_models[1]["owned_by"].as_str(),
            Some("anthropic")
        );
        assert_eq!(
            outcome.cached_models[1]["api_formats"],
            json!(["claude:messages"])
        );
        assert_eq!(
            outcome.upstream_metadata.as_ref().and_then(|value| {
                value
                    .get("kiro")
                    .and_then(|value| value.get("default_model"))
                    .and_then(|value| value.get("model_id"))
            }),
            Some(&json!("auto"))
        );
    }

    #[tokio::test]
    async fn windsurf_transport_fetches_cascade_model_configs() {
        let executed_urls = Arc::new(Mutex::new(Vec::new()));
        let runtime = TestRuntime {
            executed_urls: Arc::clone(&executed_urls),
            response_body: json!({
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
            }),
            status_code: 200,
            response_headers: BTreeMap::new(),
        };
        let outcome = fetch_models_from_transports(&runtime, &[sample_windsurf_transport()])
            .await
            .expect("models fetch should succeed");

        let urls = executed_urls.lock().expect("executed_urls lock");
        assert_eq!(
            urls.as_slice(),
            &["https://server.codeium.com/exa.api_server_pb.ApiServerService/GetCascadeModelConfigs"]
        );
        assert_eq!(
            outcome.fetched_model_ids,
            vec!["claude-sonnet-4-6".to_string(), "gpt-5.4".to_string()]
        );
        assert_eq!(outcome.cached_models.len(), 2);
        assert_eq!(
            outcome.cached_models[0]["api_formats"],
            json!(["openai:chat", "openai:responses", "claude:messages"])
        );
        assert_eq!(
            outcome.upstream_metadata.as_ref().and_then(|value| {
                value
                    .get("windsurf")
                    .and_then(|value| value.get("allowed_models_count"))
            }),
            Some(&json!(2))
        );
    }
}
