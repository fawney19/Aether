use std::collections::BTreeMap;
use std::io::Error as IoError;

use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTelemetry, RequestBody, StreamFrame,
    StreamFramePayload, StreamFrameType,
};
use axum::body::Bytes;
use base64::Engine as _;
use futures_util::stream::{self, BoxStream};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::execution_runtime::ndjson::encode_stream_frame_ndjson;
use crate::execution_runtime::transport::{
    DirectSyncExecutionRuntime, ExecutionRuntimeTransportError,
};
use crate::AppState;

const WEB_SEARCH_TOOL_NAME: &str = "web_search";
const WEB_SEARCH_TOOL_TYPE_PREFIX: &str = "web_search";
const WEB_SEARCH_QUERY_PREFIX: &str = "Perform a web search for the query: ";

pub(crate) struct KiroWebSearchStream {
    pub(crate) frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
    pub(crate) report_context: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KiroWebSearchRequest {
    query: String,
    model: String,
    input_tokens: u64,
}

#[derive(Debug, Serialize)]
struct McpRequest {
    jsonrpc: &'static str,
    id: String,
    method: &'static str,
    params: McpParams,
}

#[derive(Debug, Serialize)]
struct McpParams {
    name: &'static str,
    arguments: McpArguments,
}

#[derive(Debug, Serialize)]
struct McpArguments {
    query: String,
}

#[derive(Debug, Deserialize)]
struct McpResponse {
    #[serde(default)]
    error: Option<McpError>,
    #[serde(default)]
    result: Option<McpResult>,
}

#[derive(Debug, Deserialize)]
struct McpError {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpResult {
    #[serde(default)]
    content: Vec<McpContent>,
    #[serde(default, rename = "isError")]
    is_error: bool,
}

#[derive(Debug, Deserialize)]
struct McpContent {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct WebSearchResults {
    #[serde(default)]
    results: Vec<WebSearchResult>,
    #[serde(default, rename = "totalResults")]
    total_results: Option<i64>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct WebSearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<Value>,
}

struct KiroMcpExecution {
    result: ExecutionResult,
    url: String,
    profile_arn_present: bool,
}

struct KiroMcpRequestContext {
    headers: BTreeMap<String, String>,
    profile_arn_present: bool,
    auth_config: Option<aether_provider_transport::kiro::KiroAuthConfig>,
}

pub(crate) async fn maybe_execute_kiro_web_search_stream(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Result<Option<KiroWebSearchStream>, ExecutionRuntimeTransportError> {
    let Some(request) = detect_kiro_web_search_request(plan, report_context) else {
        return Ok(None);
    };

    let (tool_use_id, mcp_request) = create_mcp_request(request.query.as_str());
    let mcp_execution = execute_mcp_request(state, plan, &mcp_request).await?;

    debug!(
        event_name = "kiro_web_search_mcp_executed",
        log_type = "debug",
        request_id = %plan.request_id,
        candidate_id = ?plan.candidate_id,
        status_code = mcp_execution.result.status_code,
        mcp_url = %mcp_execution.url,
        profile_arn_present = mcp_execution.profile_arn_present,
        "gateway executed Kiro web_search through MCP endpoint"
    );

    if !(200..300).contains(&mcp_execution.result.status_code) {
        return Ok(Some(KiroWebSearchStream {
            frame_stream: execution_result_frame_stream(&mcp_execution.result),
            report_context: report_context.cloned(),
        }));
    }

    let search_results = parse_mcp_search_results(&mcp_execution.result);
    let sse_body = build_web_search_sse_body(
        request.model.as_str(),
        request.query.as_str(),
        tool_use_id.as_str(),
        search_results,
        request.input_tokens,
    )
    .map_err(ExecutionRuntimeTransportError::BodyEncode)?;
    let mut synthetic_context = synthetic_report_context(report_context, mcp_execution.url);
    if let Some(context) = synthetic_context.as_mut().and_then(Value::as_object_mut) {
        context.insert("kiro_web_search_mcp".to_string(), Value::Bool(true));
    }

    Ok(Some(KiroWebSearchStream {
        frame_stream: sse_frame_stream(Bytes::from(sse_body)),
        report_context: synthetic_context,
    }))
}

fn execute_result_body_bytes(result: &ExecutionResult) -> Vec<u8> {
    let Some(body) = result.body.as_ref() else {
        return Vec::new();
    };
    if let Some(json_body) = body.json_body.as_ref() {
        return serde_json::to_vec(json_body).unwrap_or_default();
    }
    body.body_bytes_b64
        .as_deref()
        .and_then(|body| base64::engine::general_purpose::STANDARD.decode(body).ok())
        .unwrap_or_default()
}

fn execution_result_frame_stream(
    result: &ExecutionResult,
) -> BoxStream<'static, Result<Bytes, IoError>> {
    raw_response_frame_stream(
        result.status_code,
        result.headers.clone(),
        Bytes::from(execute_result_body_bytes(result)),
        result.telemetry.clone(),
    )
}

fn sse_frame_stream(body: Bytes) -> BoxStream<'static, Result<Bytes, IoError>> {
    raw_response_frame_stream(
        200,
        BTreeMap::from([
            ("cache-control".to_string(), "no-cache".to_string()),
            ("content-type".to_string(), "text/event-stream".to_string()),
        ]),
        body,
        None,
    )
}

fn raw_response_frame_stream(
    status_code: u16,
    headers: BTreeMap<String, String>,
    body: Bytes,
    telemetry: Option<ExecutionTelemetry>,
) -> BoxStream<'static, Result<Bytes, IoError>> {
    let ttfb_ms = telemetry.as_ref().and_then(|value| value.ttfb_ms);
    let elapsed_ms = telemetry.as_ref().and_then(|value| value.elapsed_ms);
    let upstream_bytes = body.len() as u64;
    let mut frames = vec![
        StreamFrame {
            frame_type: StreamFrameType::Headers,
            payload: StreamFramePayload::Headers {
                status_code,
                headers,
            },
        },
        StreamFrame {
            frame_type: StreamFrameType::Telemetry,
            payload: StreamFramePayload::Telemetry {
                telemetry: ExecutionTelemetry {
                    ttfb_ms,
                    elapsed_ms: ttfb_ms,
                    upstream_bytes: Some(0),
                },
            },
        },
    ];
    if !body.is_empty() {
        frames.push(StreamFrame {
            frame_type: StreamFrameType::Data,
            payload: StreamFramePayload::Data {
                chunk_b64: Some(base64::engine::general_purpose::STANDARD.encode(body.as_ref())),
                text: None,
            },
        });
    }
    frames.push(StreamFrame {
        frame_type: StreamFrameType::Telemetry,
        payload: StreamFramePayload::Telemetry {
            telemetry: ExecutionTelemetry {
                ttfb_ms,
                elapsed_ms,
                upstream_bytes: Some(upstream_bytes),
            },
        },
    });
    frames.push(StreamFrame::eof());

    stream::iter(
        frames
            .into_iter()
            .map(|frame| encode_stream_frame_ndjson(&frame)),
    )
    .boxed()
}

async fn execute_mcp_request(
    state: &AppState,
    plan: &ExecutionPlan,
    request: &McpRequest,
) -> Result<KiroMcpExecution, ExecutionRuntimeTransportError> {
    let mcp_url = aether_provider_transport::kiro::build_kiro_mcp_url_from_resolved_url(&plan.url)
        .ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "failed to build Kiro MCP url from {}",
                plan.url
            ))
        })?;
    let mut request_context = build_mcp_request_context(state, plan).await;
    if !request_context.profile_arn_present {
        if let Some(profile_arn) = discover_kiro_profile_arn(state, plan, &request_context).await? {
            request_context.headers.insert(
                aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER.to_string(),
                profile_arn,
            );
            request_context.profile_arn_present = true;
        }
    }
    let body_json =
        serde_json::to_value(request).map_err(ExecutionRuntimeTransportError::BodyEncode)?;
    let mcp_plan = ExecutionPlan {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        provider_name: plan.provider_name.clone(),
        provider_id: plan.provider_id.clone(),
        endpoint_id: plan.endpoint_id.clone(),
        key_id: plan.key_id.clone(),
        method: "POST".to_string(),
        url: mcp_url.clone(),
        headers: request_context.headers,
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(body_json),
        stream: false,
        client_api_format: plan.client_api_format.clone(),
        provider_api_format: plan.provider_api_format.clone(),
        model_name: plan.model_name.clone(),
        proxy: plan.proxy.clone(),
        tls_profile: plan.tls_profile.clone(),
        timeouts: plan.timeouts.clone(),
    };
    let result = DirectSyncExecutionRuntime::new()
        .execute_sync(&mcp_plan)
        .await?;
    Ok(KiroMcpExecution {
        result,
        url: mcp_url,
        profile_arn_present: request_context.profile_arn_present,
    })
}

async fn build_mcp_request_context(
    state: &AppState,
    plan: &ExecutionPlan,
) -> KiroMcpRequestContext {
    let body_profile_arn = profile_arn_from_plan_body(plan);
    let fallback = || build_mcp_headers_from_plan(&plan.headers, body_profile_arn.as_deref());

    let transport = match state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return fallback(),
        Err(err) => {
            warn!(
                event_name = "kiro_web_search_transport_snapshot_unavailable",
                log_type = "ops",
                request_id = %plan.request_id,
                candidate_id = ?plan.candidate_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                error = ?err,
                "gateway could not read Kiro transport snapshot for web_search MCP"
            );
            return fallback();
        }
    };

    build_mcp_headers_from_transport(&transport, plan, body_profile_arn.as_deref())
        .unwrap_or_else(fallback)
}

fn build_mcp_headers_from_transport(
    transport: &aether_provider_transport::GatewayProviderTransportSnapshot,
    plan: &ExecutionPlan,
    body_profile_arn: Option<&str>,
) -> Option<KiroMcpRequestContext> {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(aether_provider_transport::kiro::PROVIDER_TYPE)
    {
        return None;
    }

    let resolved_auth = aether_provider_transport::kiro::resolve_local_kiro_request_auth(transport);
    let auth_config = resolved_auth
        .as_ref()
        .map(|auth| auth.auth_config.clone())
        .or_else(|| {
            aether_provider_transport::kiro::KiroAuthConfig::from_raw_json(
                transport.key.decrypted_auth_config.as_deref(),
            )
        })?;
    let fallback_secret = header_value_case_insensitive(
        &plan.headers,
        aether_provider_transport::kiro::KIRO_AUTH_HEADER,
    )
    .and_then(stripped_bearer_token)
    .or_else(|| {
        transport
            .key
            .decrypted_api_key
            .trim()
            .strip_prefix("__placeholder__")
            .map(|_| "")
            .or_else(|| Some(transport.key.decrypted_api_key.trim()))
    });
    let machine_id = resolved_auth
        .as_ref()
        .map(|auth| auth.machine_id.clone())
        .or_else(|| {
            aether_provider_transport::kiro::generate_machine_id(&auth_config, fallback_secret)
        })?;
    let mut headers = aether_provider_transport::kiro::build_mcp_headers(&auth_config, &machine_id);
    if !headers.contains_key(aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER) {
        if let Some(profile_arn) = body_profile_arn {
            headers.insert(
                aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER.to_string(),
                profile_arn.to_string(),
            );
        }
    }

    if let Some(plan_auth_value) = header_value_case_insensitive(
        &plan.headers,
        aether_provider_transport::kiro::KIRO_AUTH_HEADER,
    ) {
        headers.insert(
            aether_provider_transport::kiro::KIRO_AUTH_HEADER.to_string(),
            plan_auth_value.to_string(),
        );
    } else if let Some(auth) = resolved_auth {
        headers.insert(auth.name.to_string(), auth.value);
    }
    if header_value_case_insensitive(
        &plan.headers,
        aether_provider_transport::kiro::KIRO_TOKEN_TYPE_HEADER,
    )
    .is_some_and(|value| value.eq_ignore_ascii_case("API_KEY"))
    {
        headers.insert("tokentype".to_string(), "API_KEY".to_string());
    }

    headers.remove("x-amzn-kiro-agent-mode");
    headers.remove("content-length");
    let profile_arn_present = headers.keys().any(|name| {
        name.eq_ignore_ascii_case(aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER)
    });
    Some(KiroMcpRequestContext {
        headers,
        profile_arn_present,
        auth_config: Some(auth_config),
    })
}

fn profile_arn_from_plan_body(plan: &ExecutionPlan) -> Option<String> {
    plan.body
        .json_body
        .as_ref()
        .and_then(|body| body.get("profileArn"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn header_value_case_insensitive<'a>(
    headers: &'a BTreeMap<String, String>,
    target: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(target))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn stripped_bearer_token(value: &str) -> Option<&str> {
    value
        .trim()
        .strip_prefix("Bearer ")
        .or_else(|| value.trim().strip_prefix("bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

async fn discover_kiro_profile_arn(
    _state: &AppState,
    plan: &ExecutionPlan,
    context: &KiroMcpRequestContext,
) -> Result<Option<String>, ExecutionRuntimeTransportError> {
    let Some(auth_config) = context.auth_config.as_ref() else {
        return Ok(None);
    };
    for region in profile_discovery_regions(auth_config) {
        match discover_kiro_profile_arn_in_region(plan, context, region.as_str()).await? {
            Some(profile_arn) => {
                debug!(
                    event_name = "kiro_web_search_profile_arn_discovered",
                    log_type = "debug",
                    request_id = %plan.request_id,
                    candidate_id = ?plan.candidate_id,
                    region = region.as_str(),
                    "gateway discovered Kiro profileArn for web_search MCP"
                );
                return Ok(Some(profile_arn));
            }
            None => continue,
        }
    }
    warn!(
        event_name = "kiro_web_search_profile_arn_unavailable",
        log_type = "ops",
        request_id = %plan.request_id,
        candidate_id = ?plan.candidate_id,
        provider_id = %plan.provider_id,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        "gateway could not resolve Kiro profileArn before web_search MCP"
    );
    Ok(None)
}

async fn discover_kiro_profile_arn_in_region(
    plan: &ExecutionPlan,
    context: &KiroMcpRequestContext,
    region: &str,
) -> Result<Option<String>, ExecutionRuntimeTransportError> {
    let mut next_token: Option<String> = None;
    for _ in 0..4 {
        let mut body = serde_json::Map::new();
        if let Some(token) = next_token.as_deref() {
            body.insert("nextToken".to_string(), Value::String(token.to_string()));
        }
        let list_plan = ExecutionPlan {
            request_id: plan.request_id.clone(),
            candidate_id: plan.candidate_id.clone(),
            provider_name: plan.provider_name.clone(),
            provider_id: plan.provider_id.clone(),
            endpoint_id: plan.endpoint_id.clone(),
            key_id: plan.key_id.clone(),
            method: "POST".to_string(),
            url: format!(
                "{}/ListAvailableProfiles",
                kiro_runtime_base_url_for_region(region).trim_end_matches('/')
            ),
            headers: build_profile_discovery_headers(&context.headers, region),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(Value::Object(body)),
            stream: false,
            client_api_format: plan.client_api_format.clone(),
            provider_api_format: "kiro:profiles".to_string(),
            model_name: Some("kiro-list-available-profiles".to_string()),
            proxy: plan.proxy.clone(),
            tls_profile: plan.tls_profile.clone(),
            timeouts: plan.timeouts.clone(),
        };
        let result = DirectSyncExecutionRuntime::new()
            .execute_sync(&list_plan)
            .await?;
        if !(200..300).contains(&result.status_code) {
            debug!(
                event_name = "kiro_web_search_profile_arn_discovery_status",
                log_type = "debug",
                request_id = %plan.request_id,
                candidate_id = ?plan.candidate_id,
                region,
                status_code = result.status_code,
                "Kiro ListAvailableProfiles did not return success"
            );
            return Ok(None);
        }
        let Some(body_json) = execution_result_body_json(&result) else {
            return Ok(None);
        };
        if let Some(profile_arn) = first_profile_arn(&body_json) {
            return Ok(Some(profile_arn));
        }
        next_token = body_json
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if next_token.is_none() {
            return Ok(None);
        }
    }
    Ok(None)
}

fn build_profile_discovery_headers(
    mcp_headers: &BTreeMap<String, String>,
    region: &str,
) -> BTreeMap<String, String> {
    let mut headers = mcp_headers.clone();
    remove_header_case_insensitive(
        &mut headers,
        aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER,
    );
    headers.insert("accept".to_string(), "application/json".to_string());
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert(
        "host".to_string(),
        kiro_runtime_host_for_region(region).to_string(),
    );
    headers
}

fn profile_discovery_regions(
    auth_config: &aether_provider_transport::kiro::KiroAuthConfig,
) -> Vec<String> {
    let mut regions = Vec::new();
    push_unique_region(&mut regions, auth_config.effective_api_region());
    push_unique_region(&mut regions, auth_config.effective_auth_region());
    if auth_config.is_idc_auth() {
        push_unique_region(&mut regions, "us-east-1");
        push_unique_region(&mut regions, "eu-central-1");
    }
    regions
}

fn push_unique_region(regions: &mut Vec<String>, region: &str) {
    let region = region.trim();
    if region.is_empty() || regions.iter().any(|value| value == region) {
        return;
    }
    regions.push(region.to_string());
}

fn kiro_runtime_base_url_for_region(region: &str) -> String {
    format!("https://{}", kiro_runtime_host_for_region(region))
}

fn kiro_runtime_host_for_region(region: &str) -> String {
    match region {
        "us-gov-east-1" | "us-gov-west-1" => format!("q-fips.{region}.amazonaws.com"),
        "us-iso-east-1" => "q.us-iso-east-1.c2s.ic.gov".to_string(),
        "us-isob-east-1" => "q.us-isob-east-1.sc2s.sgov.gov".to_string(),
        "us-isof-south-1" => "q.us-isof-south-1.csp.hci.ic.gov".to_string(),
        "us-isof-east-1" => "q.us-isof-east-1.csp.hci.ic.gov".to_string(),
        _ => format!("q.{region}.amazonaws.com"),
    }
}

fn first_profile_arn(body: &Value) -> Option<String> {
    body.get("profiles")?
        .as_array()?
        .iter()
        .find_map(|profile| {
            profile
                .get("arn")
                .or_else(|| profile.get("profileArn"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn build_mcp_headers_from_plan(
    plan_headers: &BTreeMap<String, String>,
    profile_arn: Option<&str>,
) -> KiroMcpRequestContext {
    let mut headers = BTreeMap::new();
    for (name, value) in plan_headers {
        let normalized = name.trim().to_ascii_lowercase();
        if normalized.is_empty()
            || matches!(
                normalized.as_str(),
                "accept" | "content-length" | "connection" | "host" | "x-amzn-kiro-agent-mode"
            )
        {
            continue;
        }
        headers.insert(normalized, value.trim().to_string());
    }
    remove_header_case_insensitive(
        &mut headers,
        aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER,
    );
    headers.insert("accept".to_string(), "application/json".to_string());
    headers.insert("content-type".to_string(), "application/json".to_string());
    if let Some(profile_arn) = profile_arn {
        headers.insert(
            aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER.to_string(),
            profile_arn.to_string(),
        );
    }
    let profile_arn_present = headers.keys().any(|name| {
        name.eq_ignore_ascii_case(aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER)
    });
    KiroMcpRequestContext {
        headers,
        profile_arn_present,
        auth_config: None,
    }
}

fn remove_header_case_insensitive(headers: &mut BTreeMap<String, String>, target: &str) {
    let matching_keys = headers
        .keys()
        .filter(|name| name.eq_ignore_ascii_case(target))
        .cloned()
        .collect::<Vec<_>>();
    for key in matching_keys {
        headers.remove(&key);
    }
}

fn detect_kiro_web_search_request(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Option<KiroWebSearchRequest> {
    if !plan
        .provider_name
        .as_deref()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("kiro")
        && !report_context
            .and_then(|context| context.get("envelope_name"))
            .and_then(Value::as_str)
            .is_some_and(|value| {
                value.eq_ignore_ascii_case(aether_provider_transport::kiro::KIRO_ENVELOPE_NAME)
            })
    {
        return None;
    }
    if !plan.stream
        || !plan
            .provider_api_format
            .eq_ignore_ascii_case("claude:messages")
    {
        return None;
    }
    if report_context
        .and_then(|context| context.get("client_api_format"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.eq_ignore_ascii_case("claude:messages"))
    {
        return None;
    }

    if let Some(original) = report_context
        .and_then(|context| context.get("original_request_body"))
        .filter(|body| body.is_object())
    {
        if has_only_builtin_web_search_tool(original) {
            let query = extract_search_query_from_claude_request(original)?;
            let model = original
                .get("model")
                .and_then(Value::as_str)
                .or(plan.model_name.as_deref())
                .unwrap_or("claude")
                .to_string();
            return Some(KiroWebSearchRequest {
                query,
                model,
                input_tokens: estimate_input_tokens(original),
            });
        }
    }

    detect_kiro_web_search_from_envelope(plan)
}

fn detect_kiro_web_search_from_envelope(plan: &ExecutionPlan) -> Option<KiroWebSearchRequest> {
    let body = plan.body.json_body.as_ref()?;
    let current_message = body
        .get("conversationState")?
        .get("currentMessage")?
        .get("userInputMessage")?;
    let tools = current_message
        .get("userInputMessageContext")
        .and_then(|context| context.get("tools"))
        .and_then(Value::as_array)?;
    if tools.len() != 1 {
        return None;
    }
    let spec = tools[0].get("toolSpecification")?;
    if spec.get("name").and_then(Value::as_str) != Some(WEB_SEARCH_TOOL_NAME) {
        return None;
    }
    let description = spec
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let input_schema = spec.get("inputSchema").and_then(|value| value.get("json"));
    if !description.is_empty() || !schema_is_empty_object(input_schema) {
        return None;
    }
    let query = current_message
        .get("content")
        .and_then(Value::as_str)
        .and_then(strip_search_query_prefix)?;
    let model = current_message
        .get("modelId")
        .and_then(Value::as_str)
        .or(plan.model_name.as_deref())
        .unwrap_or("claude")
        .to_string();
    Some(KiroWebSearchRequest {
        query,
        model,
        input_tokens: estimate_input_tokens(body),
    })
}

fn schema_is_empty_object(value: Option<&Value>) -> bool {
    let Some(value) = value else {
        return true;
    };
    value.as_object().is_some_and(|object| {
        object
            .get("properties")
            .is_none_or(|props| props.as_object().is_none_or(serde_json::Map::is_empty))
    })
}

fn has_only_builtin_web_search_tool(body: &Value) -> bool {
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return false;
    };
    if tools.len() != 1 {
        return false;
    }
    let tool = &tools[0];
    if tool.get("name").and_then(Value::as_str) != Some(WEB_SEARCH_TOOL_NAME) {
        return false;
    }
    tool.get("type")
        .or_else(|| tool.get("tool_type"))
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().starts_with(WEB_SEARCH_TOOL_TYPE_PREFIX))
}

fn extract_search_query_from_claude_request(body: &Value) -> Option<String> {
    let first_message = body.get("messages")?.as_array()?.first()?;
    let text = extract_text_content(first_message.get("content")?)?;
    strip_search_query_prefix(text.as_str())
}

fn extract_text_content(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => blocks.iter().find_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .flatten()
        }),
        _ => None,
    }
}

fn strip_search_query_prefix(text: &str) -> Option<String> {
    let text = text.trim();
    let query = text
        .strip_prefix(WEB_SEARCH_QUERY_PREFIX)
        .unwrap_or(text)
        .trim();
    (!query.is_empty()).then(|| query.to_string())
}

fn estimate_input_tokens(body: &Value) -> u64 {
    let size = serde_json::to_string(body)
        .map(|body| body.len() as u64)
        .unwrap_or_default();
    (size / 4).max(1)
}

fn create_mcp_request(query: &str) -> (String, McpRequest) {
    let random = Uuid::new_v4().simple().to_string();
    let timestamp = chrono::Utc::now().timestamp_millis();
    let suffix = Uuid::new_v4().simple().to_string();
    let request_id = format!(
        "web_search_tooluse_{}_{}_{}",
        &random[..22],
        timestamp,
        &suffix[..8]
    );
    let tool_use_id = format!("srvtoolu_{}", Uuid::new_v4().simple());
    (
        tool_use_id,
        McpRequest {
            jsonrpc: "2.0",
            id: request_id,
            method: "tools/call",
            params: McpParams {
                name: WEB_SEARCH_TOOL_NAME,
                arguments: McpArguments {
                    query: query.to_string(),
                },
            },
        },
    )
}

fn parse_mcp_search_results(result: &ExecutionResult) -> Option<WebSearchResults> {
    let body_json = execution_result_body_json(result)?;
    let response: McpResponse = serde_json::from_value(body_json).ok()?;
    if let Some(error) = response.error {
        warn!(
            event_name = "kiro_web_search_mcp_error",
            log_type = "event",
            code = error.code.unwrap_or_default(),
            message = error.message.as_deref().unwrap_or("unknown"),
            "Kiro MCP web_search returned JSON-RPC error"
        );
        return None;
    }
    let mcp_result = response.result?;
    if mcp_result.is_error {
        warn!(
            event_name = "kiro_web_search_mcp_tool_error",
            log_type = "event",
            "Kiro MCP web_search returned tool error"
        );
        return None;
    }
    let content = mcp_result
        .content
        .iter()
        .find(|content| content.content_type == "text")?;
    serde_json::from_str::<WebSearchResults>(content.text.as_str()).ok()
}

fn execution_result_body_json(result: &ExecutionResult) -> Option<Value> {
    let body = result.body.as_ref()?;
    if let Some(json_body) = body.json_body.as_ref() {
        return Some(json_body.clone());
    }
    let body = body.body_bytes_b64.as_deref()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn build_web_search_sse_body(
    model: &str,
    query: &str,
    tool_use_id: &str,
    search_results: Option<WebSearchResults>,
    input_tokens: u64,
) -> Result<Vec<u8>, serde_json::Error> {
    let events = build_web_search_events(model, query, tool_use_id, search_results, input_tokens);
    crate::ai_serving::api::encode_kiro_sse_events(events)
}

fn build_web_search_events(
    model: &str,
    query: &str,
    tool_use_id: &str,
    search_results: Option<WebSearchResults>,
    input_tokens: u64,
) -> Vec<Value> {
    let message_id = format!("msg_{}", &Uuid::new_v4().simple().to_string()[..24]);
    let mut events = vec![
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": 0,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0
                }
            }
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "text",
                "text": ""
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "text_delta",
                "text": format!("I'll search for \"{}\".", query)
            }
        }),
        json!({
            "type": "content_block_stop",
            "index": 0
        }),
        json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {
                "id": tool_use_id,
                "type": "server_tool_use",
                "name": WEB_SEARCH_TOOL_NAME,
                "input": {"query": query}
            }
        }),
        json!({
            "type": "content_block_stop",
            "index": 1
        }),
        json!({
            "type": "content_block_start",
            "index": 2,
            "content_block": {
                "type": "web_search_tool_result",
                "content": search_result_content(search_results.as_ref())
            }
        }),
        json!({
            "type": "content_block_stop",
            "index": 2
        }),
        json!({
            "type": "content_block_start",
            "index": 3,
            "content_block": {
                "type": "text",
                "text": ""
            }
        }),
    ];

    let summary = generate_search_summary(query, search_results.as_ref());
    for chunk in chunk_text(summary.as_str(), 100) {
        events.push(json!({
            "type": "content_block_delta",
            "index": 3,
            "delta": {
                "type": "text_delta",
                "text": chunk
            }
        }));
    }
    events.extend([
        json!({
            "type": "content_block_stop",
            "index": 3
        }),
        json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": "end_turn",
                "stop_sequence": Value::Null
            },
            "usage": {
                "input_tokens": input_tokens,
                "output_tokens": estimate_text_tokens(summary.as_str()),
                "server_tool_use": {
                    "web_search_requests": 1
                }
            }
        }),
        json!({
            "type": "message_stop"
        }),
    ]);
    events
}

fn search_result_content(results: Option<&WebSearchResults>) -> Vec<Value> {
    results
        .into_iter()
        .flat_map(|results| results.results.iter())
        .filter(|result| !result.title.trim().is_empty() && !result.url.trim().is_empty())
        .map(|result| {
            json!({
                "type": "web_search_result",
                "title": result.title,
                "url": result.url,
                "encrypted_content": result.snippet.clone().unwrap_or_default(),
                "page_age": page_age(result.published_date.as_ref())
            })
        })
        .collect()
}

fn page_age(value: Option<&Value>) -> Option<String> {
    let timestamp_ms = value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
    })?;
    chrono::DateTime::from_timestamp_millis(timestamp_ms)
        .map(|value| value.format("%B %-d, %Y").to_string())
}

fn generate_search_summary(query: &str, results: Option<&WebSearchResults>) -> String {
    let mut summary = format!("Here are the search results for \"{}\":\n\n", query);
    match results {
        Some(results) if !results.results.is_empty() => {
            if let Some(source_query) = results
                .query
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty() && *value != query)
            {
                summary.push_str(&format!("Search query used: {source_query}\n\n"));
            }
            if let Some(total) = results.total_results {
                summary.push_str(&format!("Total results reported: {total}\n\n"));
            }
            for (idx, result) in results.results.iter().enumerate() {
                if result.title.trim().is_empty() || result.url.trim().is_empty() {
                    continue;
                }
                summary.push_str(&format!("{}. **{}**\n", idx + 1, result.title));
                if let Some(snippet) = result
                    .snippet
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    summary.push_str("   ");
                    summary.push_str(truncate_chars(snippet, 200).as_str());
                    summary.push('\n');
                }
                summary.push_str(&format!("   Source: {}\n\n", result.url));
            }
            if let Some(error) = results
                .error
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                summary.push_str(&format!("Search warning: {error}\n\n"));
            }
        }
        _ => summary.push_str("No results found.\n"),
    }
    summary.push_str(
        "\nPlease note that these are web search results and may not be fully accurate or up-to-date.",
    );
    summary
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    match value.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}...", &value[..idx]),
        None => value.to_string(),
    }
}

fn chunk_text(value: &str, chunk_size: usize) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let chars = value.chars().collect::<Vec<_>>();
    chars
        .chunks(chunk_size.max(1))
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn estimate_text_tokens(text: &str) -> u64 {
    ((text.len() as u64 + 3) / 4).max(1)
}

fn synthetic_report_context(report_context: Option<&Value>, mcp_url: String) -> Option<Value> {
    let mut context = report_context.cloned()?;
    if let Some(object) = context.as_object_mut() {
        object.insert("has_envelope".to_string(), Value::Bool(false));
        object.insert("needs_conversion".to_string(), Value::Bool(false));
        object.insert("upstream_url".to_string(), Value::String(mcp_url));
        object.remove("envelope_name");
    }
    Some(context)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_contracts::{ExecutionPlan, RequestBody};
    use serde_json::json;

    use super::{
        build_mcp_headers_from_plan, build_web_search_sse_body, detect_kiro_web_search_request,
        parse_mcp_search_results, strip_search_query_prefix,
    };

    fn sample_plan(body: serde_json::Value) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: Some("cand-1".to_string()),
            provider_name: Some("Kiro".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true"
                .to_string(),
            headers: BTreeMap::from([
                ("authorization".to_string(), "Bearer token".to_string()),
                (
                    "accept".to_string(),
                    "application/vnd.amazon.eventstream".to_string(),
                ),
                ("x-amzn-kiro-agent-mode".to_string(), "vibe".to_string()),
            ]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(body),
            stream: true,
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "claude:messages".to_string(),
            model_name: Some("claude-sonnet-4.6".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        }
    }

    #[test]
    fn strips_web_search_query_prefix() {
        assert_eq!(
            strip_search_query_prefix("Perform a web search for the query: Shanghai weather")
                .as_deref(),
            Some("Shanghai weather")
        );
        assert_eq!(
            strip_search_query_prefix("Shanghai weather").as_deref(),
            Some("Shanghai weather")
        );
    }

    #[test]
    fn detects_builtin_web_search_from_original_report_context() {
        let plan = sample_plan(json!({"conversationState": {}}));
        let report_context = json!({
            "envelope_name": "kiro:generateAssistantResponse",
            "client_api_format": "claude:messages",
            "original_request_body": {
                "model": "claude-haiku-4-5-20251001",
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "Perform a web search for the query: Shanghai weather today"
                    }]
                }],
                "tools": [{
                    "type": "web_search_20250305",
                    "name": "web_search",
                    "max_uses": 8
                }]
            }
        });

        let detected = detect_kiro_web_search_request(&plan, Some(&report_context))
            .expect("web_search should be detected");
        assert_eq!(detected.query, "Shanghai weather today");
        assert_eq!(detected.model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn detects_builtin_web_search_from_kiro_envelope_fallback() {
        let plan = sample_plan(json!({
            "conversationState": {
                "currentMessage": {
                    "userInputMessage": {
                        "content": "Perform a web search for the query: Rust 2026",
                        "modelId": "claude-sonnet-4.6",
                        "userInputMessageContext": {
                            "tools": [{
                                "toolSpecification": {
                                    "name": "web_search",
                                    "description": "",
                                    "inputSchema": {"json": {"type": "object", "properties": {}}}
                                }
                            }]
                        }
                    }
                }
            }
        }));
        let report_context = json!({
            "envelope_name": "kiro:generateAssistantResponse",
            "client_api_format": "claude:messages"
        });

        let detected = detect_kiro_web_search_request(&plan, Some(&report_context))
            .expect("web_search should be detected");
        assert_eq!(detected.query, "Rust 2026");
        assert_eq!(detected.model, "claude-sonnet-4.6");
    }

    #[test]
    fn builds_mcp_headers_for_profile_and_external_idp_modes() {
        let headers = BTreeMap::from([
            ("authorization".to_string(), "Bearer token".to_string()),
            (
                "accept".to_string(),
                "application/vnd.amazon.eventstream".to_string(),
            ),
            ("host".to_string(), "q.us-east-1.amazonaws.com".to_string()),
            ("x-amzn-kiro-agent-mode".to_string(), "vibe".to_string()),
        ]);
        let social = build_mcp_headers_from_plan(&headers, Some("arn:profile"));
        assert_eq!(
            social.headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            social
                .headers
                .get(aether_provider_transport::kiro::KIRO_PROFILE_ARN_HEADER)
                .map(String::as_str),
            Some("arn:profile")
        );
        assert!(!social.headers.contains_key("x-amzn-kiro-agent-mode"));
        assert!(social.profile_arn_present);

        let idc = build_mcp_headers_from_plan(&headers, None);
        assert_eq!(
            idc.headers
                .get(aether_provider_transport::kiro::KIRO_TOKEN_TYPE_HEADER),
            None
        );
        assert!(!idc.profile_arn_present);
    }

    #[test]
    fn mcp_plan_header_fallback_does_not_invent_external_idp_token_type() {
        let headers = BTreeMap::from([("authorization".to_string(), "Bearer token".to_string())]);

        let context = build_mcp_headers_from_plan(&headers, None);

        assert_eq!(
            context
                .headers
                .get(aether_provider_transport::kiro::KIRO_TOKEN_TYPE_HEADER),
            None
        );
    }

    #[test]
    fn mcp_plan_header_fallback_preserves_api_key_token_type() {
        let headers = BTreeMap::from([
            ("authorization".to_string(), "Bearer token".to_string()),
            ("tokentype".to_string(), "API_KEY".to_string()),
        ]);

        let context = build_mcp_headers_from_plan(&headers, None);

        assert_eq!(
            context.headers.get("tokentype").map(String::as_str),
            Some("API_KEY")
        );
    }

    #[test]
    fn parses_mcp_search_result_text_payload() {
        let result = aether_contracts::ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 200,
            headers: BTreeMap::new(),
            body: Some(aether_contracts::ResponseBody {
                json_body: Some(json!({
                    "jsonrpc": "2.0",
                    "id": "web_search_tooluse_1",
                    "result": {
                        "isError": false,
                        "content": [{
                            "type": "text",
                            "text": "{\"results\":[{\"title\":\"Example\",\"url\":\"https://example.com\",\"snippet\":\"Snippet\"}],\"totalResults\":1}"
                        }]
                    }
                })),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: None,
        };

        let parsed = parse_mcp_search_results(&result).expect("results should parse");
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].title, "Example");
    }

    #[test]
    fn builds_anthropic_web_search_sse() {
        let sse = build_web_search_sse_body(
            "claude-sonnet-4.6",
            "Shanghai weather",
            "srvtoolu_123",
            None,
            12,
        )
        .expect("sse should encode");
        let text = String::from_utf8(sse).expect("sse should be utf8");
        assert!(text.contains("\"type\":\"server_tool_use\""));
        assert!(text.contains("\"type\":\"web_search_tool_result\""));
        assert!(text.contains("\"web_search_requests\":1"));
    }
}
