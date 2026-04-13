use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_contracts::{ExecutionPlan, ExecutionResult, RequestBody};
use aether_provider_transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
    GatewayProviderTransportSnapshot,
};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::logic::{extract_error_message, parse_models_response_page, preset_models_for_provider};
use crate::transport::{
    build_antigravity_fetch_available_models_plan, build_gemini_cli_load_code_assist_plan,
    build_standard_models_fetch_execution_plan, build_vertex_models_fetch_execution_plan,
    ModelFetchTransportRuntime,
};

const ANTIGRAVITY_SANDBOX_BASE_URL: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
const ANTIGRAVITY_DAILY_BASE_URL: &str = "https://daily-cloudcode-pa.googleapis.com";
const ANTIGRAVITY_PROD_BASE_URL: &str = "https://cloudcode-pa.googleapis.com";
const ANTIGRAVITY_BLOCKED_MODELS: &[&str] = &["chat_23310", "chat_20706"];
const VERTEX_API_BASE_URL: &str = "https://aiplatform.googleapis.com";
const VERTEX_PAGE_SIZE: &str = "100";
const VERTEX_MAX_PAGES: usize = 20;
const GOOGLE_OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[derive(Debug, Clone, PartialEq)]
pub struct ModelsFetchOutcome {
    pub fetched_model_ids: Vec<String>,
    pub cached_models: Vec<Value>,
    pub errors: Vec<String>,
    pub has_success: bool,
    pub upstream_metadata: Option<Value>,
}

pub async fn fetch_models_from_transports(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
) -> Result<ModelsFetchOutcome, String> {
    let Some(first_transport) = transports.first() else {
        return Err("No transport snapshots available for models fetch".to_string());
    };

    let provider_type = first_transport
        .provider
        .provider_type
        .trim()
        .to_ascii_lowercase();
    if let Some(models) = preset_models_for_provider(&provider_type) {
        if provider_type == "gemini_cli" {
            return fetch_gemini_cli_models(runtime, first_transport, models).await;
        }
        return Ok(build_success_outcome(models, None, true));
    }

    match provider_type.as_str() {
        "antigravity" => fetch_antigravity_models(runtime, first_transport).await,
        "vertex_ai" => fetch_vertex_models(runtime, transports).await,
        _ => fetch_standard_models(runtime, transports).await,
    }
}

async fn fetch_standard_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transports: &[GatewayProviderTransportSnapshot],
) -> Result<ModelsFetchOutcome, String> {
    let mut all_models = Vec::new();
    let mut errors = Vec::new();
    let mut has_success = false;

    for transport in transports {
        match fetch_standard_models_for_transport(runtime, transport).await {
            Ok(outcome) => {
                all_models.extend(outcome.cached_models);
                has_success |= outcome.has_success;
            }
            Err(err) => errors.push(format!("{}: {err}", transport.endpoint.api_format.trim())),
        }
    }

    Ok(build_success_outcome(all_models, None, has_success).with_errors(errors))
}

async fn fetch_standard_models_for_transport(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<ModelsFetchOutcome, String> {
    let mut all_models = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let mut next_after_id = None;
    let mut has_success = false;

    for _ in 0..20 {
        let plan = build_standard_models_fetch_execution_plan(
            runtime,
            transport,
            next_after_id.as_deref(),
        )
        .await?;
        let result = runtime.execute_model_fetch_execution_plan(&plan).await?;
        let body_json = execution_result_json_body(&result)?;
        let parsed = parse_models_response_page(&transport.endpoint.api_format, &body_json)?;
        has_success = true;
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

    Ok(build_success_outcome(all_models, None, has_success))
}

async fn fetch_antigravity_models(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<ModelsFetchOutcome, String> {
    let auth_config = transport_auth_config(transport);
    let project_id = auth_config
        .as_ref()
        .and_then(|value| value.get("project_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "antigravity: missing auth_config.project_id (please re-auth)".to_string())?
        .to_string();

    let mut errors = Vec::new();
    for base_url in [
        ANTIGRAVITY_SANDBOX_BASE_URL,
        ANTIGRAVITY_DAILY_BASE_URL,
        ANTIGRAVITY_PROD_BASE_URL,
    ] {
        let plan = match build_antigravity_fetch_available_models_plan(
            runtime,
            transport,
            base_url,
            &project_id,
        )
        .await
        {
            Ok(plan) => plan,
            Err(err) => return Err(err),
        };

        let result = match runtime.execute_model_fetch_execution_plan(&plan).await {
            Ok(result) => result,
            Err(err) => {
                errors.push(format!("{base_url}: {err}"));
                continue;
            }
        };

        if (200..300).contains(&result.status_code) {
            let body_json = execution_result_json_body_allow_empty(&result)?;
            let (models, metadata) = parse_antigravity_models_response(&body_json)?;
            return Ok(build_success_outcome(models, metadata, true));
        }

        let error = execution_result_error_message(&result);
        if should_fallback_antigravity_status(result.status_code) {
            errors.push(format!("{base_url}: {error}"));
            continue;
        }
        return Err(error);
    }

    Ok(ModelsFetchOutcome {
        fetched_model_ids: Vec::new(),
        cached_models: Vec::new(),
        errors,
        has_success: false,
        upstream_metadata: None,
    })
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
                    if let Some(project_id) =
                        extract_gemini_cli_project_id(&body_json).or_else(|| {
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
            errors: vec!["vertex_ai(api_key): missing api key".to_string()],
            has_success: false,
            upstream_metadata: None,
        });
    }

    let mut all_models = Vec::new();
    let mut hard_errors = Vec::new();
    let mut soft_errors = Vec::new();
    let mut has_success = false;

    for base_url in iter_vertex_base_urls(transports) {
        let url = build_vertex_google_list_url(&base_url, api_key, None);
        let outcome = fetch_vertex_models_from_url(
            runtime,
            reference_transport,
            &url,
            auth_config,
            "google",
            "gemini:chat",
            None,
        )
        .await?;
        has_success |= outcome.has_success;
        if let Some(error) = outcome.error {
            if is_soft_not_found(&error) {
                soft_errors.push(format!("{base_url}: {error}"));
            } else {
                hard_errors.push(format!("{base_url}: {error}"));
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
        errors,
        has_success,
        upstream_metadata: None,
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
            errors: vec!["vertex_ai(service_account): missing auth_config".to_string()],
            has_success: false,
            upstream_metadata: None,
        });
    };
    let token = exchange_vertex_service_account_token(runtime, &transports[0], auth_config).await?;
    let project_id = json_string(auth_config.get("project_id"))
        .ok_or_else(|| "vertex_ai(service_account): missing project_id".to_string())?;
    let gemini_transport =
        select_transport_for_api_format(transports, "gemini:").unwrap_or(&transports[0]);
    let claude_transport =
        select_transport_for_api_format(transports, "claude:").unwrap_or(gemini_transport);

    let mut all_models = Vec::new();
    let mut hard_errors = Vec::new();
    let mut soft_errors = Vec::new();
    let mut has_success = false;

    for region in vertex_regions(auth_config) {
        let base = if region == "global" {
            VERTEX_API_BASE_URL.to_string()
        } else {
            format!("https://{region}-aiplatform.googleapis.com")
        };
        for (publisher, transport, api_format) in [
            ("google", gemini_transport, "gemini:chat"),
            ("anthropic", claude_transport, "claude:chat"),
        ] {
            let url =
                build_vertex_service_account_list_url(&base, &project_id, &region, publisher, None);
            let outcome = fetch_vertex_models_from_url(
                runtime,
                transport,
                &url,
                Some(auth_config),
                publisher,
                api_format,
                Some(("authorization".to_string(), format!("Bearer {token}"))),
            )
            .await?;
            has_success |= outcome.has_success;
            if let Some(error) = outcome.error {
                let labeled = format!("{url}: {error}");
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
        errors,
        has_success,
        upstream_metadata: None,
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
        .await?;
        let result = runtime.execute_model_fetch_execution_plan(&plan).await?;
        if result.status_code != 200 {
            return Ok(VertexFetchPageOutcome {
                models: Vec::new(),
                error: Some(execution_result_error_message(&result)),
                has_success,
            });
        }

        has_success = true;
        let body_json = execution_result_json_body_allow_empty(&result)?;
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
    let token_url = json_string(auth_config.get("token_uri"))
        .unwrap_or_else(|| GOOGLE_OAUTH_TOKEN_URL.to_string());
    let client_email = json_string(auth_config.get("client_email"))
        .ok_or_else(|| "vertex_ai(service_account): missing client_email".to_string())?;
    let private_key = json_string(auth_config.get("private_key"))
        .ok_or_else(|| "vertex_ai(service_account): missing private_key".to_string())?;
    let now = now_unix_secs();
    let assertion =
        build_vertex_service_account_assertion(&client_email, &private_key, &token_url, now)?;
    let body = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
    );
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
        client_api_format: "gemini:chat".to_string(),
        provider_api_format: "vertex_ai:service_account_token".to_string(),
        model_name: Some("token".to_string()),
        proxy: runtime.resolve_model_fetch_proxy(transport).await,
        tls_profile: resolve_transport_tls_profile(transport),
        timeouts: resolve_transport_execution_timeouts(transport),
    };
    let result = runtime.execute_model_fetch_execution_plan(&plan).await?;
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
    let private_key = RsaPrivateKey::from_pkcs8_pem(private_key_pem)
        .map_err(|err| format!("vertex_ai(service_account): private_key parse failed: {err}"))?;
    let signing_key = SigningKey::<Sha256>::new(private_key);
    let signature = signing_key.sign(message.as_bytes());
    Ok(format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
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

fn execution_result_error_message(result: &ExecutionResult) -> String {
    result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .and_then(extract_error_message)
        .or_else(|| {
            result.error.as_ref().and_then(|error| {
                let message = error.message.trim();
                (!message.is_empty()).then_some(message.to_string())
            })
        })
        .unwrap_or_else(|| format!("HTTP {}: upstream request failed", result.status_code))
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
        if model_id.is_empty() || ANTIGRAVITY_BLOCKED_MODELS.contains(&model_id) {
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
            "api_formats": ["gemini:chat"],
        }));

        let quota_payload = build_antigravity_quota_payload(model_object.get("quotaInfo"));
        quota_by_model.insert(model_id.to_string(), Value::Object(quota_payload));
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

fn build_antigravity_quota_payload(quota_info: Option<&Value>) -> serde_json::Map<String, Value> {
    let quota_info = quota_info.and_then(Value::as_object);
    let reset_time = quota_info
        .and_then(|value| value.get("resetTime"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let remaining_fraction = quota_info
        .and_then(|value| value.get("remainingFraction"))
        .and_then(Value::as_f64);

    let mut payload = serde_json::Map::new();
    match remaining_fraction {
        Some(remaining_fraction) => {
            let used_percent = ((1.0 - remaining_fraction) * 100.0).clamp(0.0, 100.0);
            payload.insert(
                "remaining_fraction".to_string(),
                Value::from(remaining_fraction),
            );
            payload.insert("used_percent".to_string(), Value::from(used_percent));
        }
        None => {
            payload.insert("remaining_fraction".to_string(), Value::from(0.0));
            payload.insert("used_percent".to_string(), Value::from(100.0));
        }
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

fn build_vertex_google_list_url(base_url: &str, api_key: &str, page_token: Option<&str>) -> String {
    let path = if base_url.trim_end_matches('/').ends_with("/v1")
        || base_url.trim_end_matches('/').ends_with("/v1beta")
    {
        "/publishers/google/models"
    } else {
        "/v1/publishers/google/models"
    };
    let url = build_simple_path_url(base_url, path);
    let mut url = append_query_param(url, "key", api_key);
    url = append_query_param(url, "pageSize", VERTEX_PAGE_SIZE);
    if let Some(page_token) = page_token {
        url = append_query_param(url, "pageToken", page_token);
    }
    url
}

fn build_vertex_service_account_list_url(
    base_url: &str,
    project_id: &str,
    region: &str,
    publisher: &str,
    page_token: Option<&str>,
) -> String {
    let path =
        format!("/v1/projects/{project_id}/locations/{region}/publishers/{publisher}/models");
    let mut url = build_simple_path_url(base_url, &path);
    url = append_query_param(url, "pageSize", VERTEX_PAGE_SIZE);
    if let Some(page_token) = page_token {
        url = append_query_param(url, "pageToken", page_token);
    }
    url
}

fn build_simple_path_url(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim().trim_end_matches('/'), path.trim())
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
                    return normalize_api_format(api_format.as_str().unwrap_or("gemini:chat"));
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
        "claude:chat".to_string()
    } else {
        "gemini:chat".to_string()
    }
}

fn vertex_regions(auth_config: &Value) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut regions = Vec::new();
    let auth_config = auth_config.as_object();

    let mut push_region = |region: Option<&str>| {
        let Some(region) = region.map(str::trim).filter(|value| !value.is_empty()) else {
            return;
        };
        if seen.insert(region.to_string()) {
            regions.push(region.to_string());
        }
    };

    push_region(
        auth_config
            .and_then(|value| value.get("region"))
            .and_then(Value::as_str),
    );
    if let Some(model_regions) = auth_config
        .and_then(|value| value.get("model_regions"))
        .and_then(Value::as_object)
    {
        for value in model_regions.values() {
            push_region(value.as_str());
        }
    }
    push_region(Some("global"));
    push_region(Some("us-central1"));
    regions
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
    ModelsFetchOutcome {
        fetched_model_ids: collect_model_ids(&cached_models),
        cached_models,
        errors: Vec::new(),
        has_success,
        upstream_metadata,
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
    url.push(separator);
    url.push_str(key.trim());
    url.push('=');
    url.push_str(value.trim());
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
        let tier = body.get(key)?;
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

fn extract_gemini_cli_project_id(body: &Value) -> Option<String> {
    let raw = body.get("cloudaicompanionProject")?;
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
}

impl OutcomeExt for ModelsFetchOutcome {
    fn with_errors(mut self, errors: Vec<String>) -> Self {
        self.errors = errors;
        self
    }
}
