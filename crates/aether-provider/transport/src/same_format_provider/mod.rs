use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::anthropic_compat::{
    resolve_anthropic_compatibility_profile, AnthropicCompatibilityProfile,
};
use crate::antigravity::is_antigravity_provider_transport;
use crate::auth::{
    build_complete_passthrough_headers, build_complete_passthrough_headers_with_auth,
    replace_upstream_auth_headers, resolve_local_gemini_auth, resolve_local_openai_bearer_auth,
    resolve_local_standard_auth,
};
use crate::claude_code::{
    build_claude_code_passthrough_headers, current_claude_code_transport_identity_profile,
    local_claude_code_transport_unsupported_reason_with_network,
};
use crate::gemini_cli::is_gemini_cli_provider_transport;
use crate::grok::{is_grok_provider_transport, resolve_grok_session_auth};
use crate::headers::{force_identity_accept_encoding, upstream_credential_header_names};
use crate::kiro::{
    build_kiro_provider_headers, build_kiro_provider_request_body, is_kiro_provider_transport,
    local_kiro_request_transport_unsupported_reason_with_network, KiroAuthConfig,
    KiroProviderHeadersInput,
};
use crate::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_standard_transport_unsupported_reason_with_network,
};
use crate::rules::{
    apply_local_body_rules_with_request_headers, apply_local_header_rules_with_request_headers,
};
use crate::snapshot::GatewayProviderTransportSnapshot;
use crate::vertex::{
    is_vertex_service_account_transport_context, is_vertex_transport_context,
    local_vertex_gemini_transport_unsupported_reason_with_network,
};

use crate::{build_transport_request_url_for_request_body, TransportRequestUrlParams};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameFormatProviderFamily {
    Standard,
    Gemini,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderRequestBehaviorParams<'a> {
    pub require_streaming: bool,
    pub provider_api_format: &'a str,
    pub report_kind: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SameFormatProviderRequestBehavior {
    pub is_antigravity: bool,
    pub is_gemini_cli: bool,
    pub is_claude_code: bool,
    pub is_claude_code_transport: bool,
    pub anthropic_compatibility_profile: AnthropicCompatibilityProfile,
    pub is_vertex: bool,
    pub is_kiro: bool,
    pub upstream_is_stream: bool,
    pub force_body_stream_field: bool,
    pub report_kind: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderRequestBodyInput<'a> {
    pub body_json: &'a Value,
    pub mapped_model: &'a str,
    pub client_api_format: &'a str,
    pub provider_api_format: &'a str,
    pub source_model: Option<&'a str>,
    pub family: SameFormatProviderFamily,
    pub body_rules: Option<&'a Value>,
    pub request_headers: Option<&'a http::HeaderMap>,
    pub upstream_is_stream: bool,
    pub force_body_stream_field: bool,
    pub kiro_auth_config: Option<&'a KiroAuthConfig>,
    pub is_claude_code: bool,
    pub enable_model_directives: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SameFormatProviderRequestBodyOutput {
    pub body: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compatibility_edits: Vec<SameFormatProviderCompatibilityEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SameFormatProviderCompatibilityEdit {
    pub field: String,
    pub action: SameFormatProviderCompatibilityEditAction,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SameFormatProviderCompatibilityEditAction {
    RuntimeRewrite,
    ProviderCompatibilityRewrite,
    ProviderEnvelope,
    OperatorRule,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderUpstreamUrlParams<'a> {
    pub provider_api_format: &'a str,
    pub mapped_model: &'a str,
    pub upstream_is_stream: bool,
    pub request_query: Option<&'a str>,
    pub kiro_api_region: Option<&'a str>,
    pub api_operation: Option<aether_ai_formats::ApiOperation>,
    pub provider_request_body: Option<&'a Value>,
}

#[derive(Debug, Clone, Copy)]
pub struct SameFormatProviderHeadersInput<'a> {
    pub headers: &'a http::HeaderMap,
    pub provider_request_body: &'a Value,
    pub original_request_body: &'a Value,
    pub header_rules: Option<&'a Value>,
    pub behavior: SameFormatProviderRequestBehavior,
    pub api_operation: Option<aether_ai_formats::ApiOperation>,
    pub auth_header: Option<&'a str>,
    pub auth_value: Option<&'a str>,
    pub extra_headers: &'a BTreeMap<String, String>,
    pub kiro_auth_config: Option<&'a KiroAuthConfig>,
    pub kiro_machine_id: Option<&'a str>,
}

pub fn classify_same_format_provider_request_behavior(
    transport: &GatewayProviderTransportSnapshot,
    params: SameFormatProviderRequestBehaviorParams<'_>,
) -> SameFormatProviderRequestBehavior {
    classify_same_format_provider_request_behavior_for_operation(transport, params, None)
}

pub fn classify_same_format_provider_request_behavior_for_operation(
    transport: &GatewayProviderTransportSnapshot,
    params: SameFormatProviderRequestBehaviorParams<'_>,
    api_operation: Option<aether_ai_formats::ApiOperation>,
) -> SameFormatProviderRequestBehavior {
    let is_antigravity = is_antigravity_provider_transport(transport);
    let is_gemini_cli = is_gemini_cli_provider_transport(transport);
    let is_claude_code_transport = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code");
    let anthropic_compatibility_profile =
        resolve_anthropic_compatibility_profile(transport, params.provider_api_format);
    let is_claude_code = anthropic_compatibility_profile.uses_claude_code_compatibility();
    let is_vertex = is_vertex_transport_context(transport);
    let is_kiro = is_kiro_provider_transport(transport);
    let gemini_cli_requires_upstream_streaming = is_gemini_cli
        && crate::gemini_cli::gemini_cli_v1internal_requires_upstream_streaming(
            params.provider_api_format,
            params.require_streaming,
        );
    let operation_requires_sync = matches!(
        api_operation,
        Some(aether_ai_formats::ApiOperation::ClaudeCountTokens)
    );
    let upstream_is_stream = !operation_requires_sync
        && aether_ai_formats::resolve_upstream_is_stream_for_provider(
            transport.endpoint.config.as_ref(),
            transport.provider.provider_type.as_str(),
            params.provider_api_format,
            params.require_streaming,
            is_kiro || is_antigravity || gemini_cli_requires_upstream_streaming,
        );
    let force_body_stream_field = !operation_requires_sync
        && aether_ai_formats::api_format_uses_body_stream_field(params.provider_api_format)
        && aether_ai_formats::endpoint_config_forces_upstream_stream_policy(
            transport.endpoint.config.as_ref(),
        );
    let report_kind = if is_kiro && !params.require_streaming {
        "claude_cli_sync_finalize"
    } else if (is_gemini_cli || is_antigravity) && !params.require_streaming {
        match params.report_kind {
            "gemini_chat_sync_success" => "gemini_chat_sync_finalize",
            "gemini_cli_sync_success" => "gemini_cli_sync_finalize",
            _ => params.report_kind,
        }
    } else {
        params.report_kind
    };

    SameFormatProviderRequestBehavior {
        is_antigravity,
        is_gemini_cli,
        is_claude_code,
        is_claude_code_transport,
        anthropic_compatibility_profile,
        is_vertex,
        is_kiro,
        upstream_is_stream,
        force_body_stream_field,
        report_kind,
    }
}

pub fn build_same_format_provider_request_body(
    input: SameFormatProviderRequestBodyInput<'_>,
) -> Option<Value> {
    build_same_format_provider_request_body_inner(input, None)
}

pub fn build_same_format_provider_request_body_with_compatibility_report(
    input: SameFormatProviderRequestBodyInput<'_>,
) -> Option<SameFormatProviderRequestBodyOutput> {
    let mut compatibility_edits = Vec::new();
    let body =
        build_same_format_provider_request_body_inner(input, Some(&mut compatibility_edits))?;
    Some(SameFormatProviderRequestBodyOutput {
        body,
        compatibility_edits,
    })
}

pub fn enforce_same_format_provider_api_operation_body_policy(
    body: &mut Value,
    api_operation: Option<aether_ai_formats::ApiOperation>,
) -> bool {
    if !matches!(
        api_operation,
        Some(aether_ai_formats::ApiOperation::ClaudeCountTokens)
    ) {
        return false;
    }
    body.as_object_mut()
        .is_some_and(|object| object.remove("stream").is_some())
}

fn build_same_format_provider_request_body_inner(
    input: SameFormatProviderRequestBodyInput<'_>,
    mut compatibility_edits: Option<&mut Vec<SameFormatProviderCompatibilityEdit>>,
) -> Option<Value> {
    if let Some(kiro_auth) = input.kiro_auth_config {
        let body = build_kiro_provider_request_body(
            input.body_json,
            input.mapped_model,
            kiro_auth,
            input.body_rules,
            input.request_headers,
        )?;
        record_compatibility_edit(
            &mut compatibility_edits,
            "$",
            SameFormatProviderCompatibilityEditAction::ProviderEnvelope,
            "wrapped same-format request in Kiro provider envelope",
        );
        return Some(body);
    }

    if embedding_multimodal_input_requires_aliyun_provider(
        input.client_api_format,
        input.provider_api_format,
        input.body_json,
    ) {
        return None;
    }

    let mut provider_request_body = if aether_ai_formats::api_format_alias_matches(
        input.client_api_format,
        input.provider_api_format,
    ) {
        let request_body_object = input.body_json.as_object()?;
        serde_json::Map::from_iter(
            request_body_object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        )
    } else {
        aether_ai_formats::convert_request_pure_with_context(
            input.client_api_format,
            input.provider_api_format,
            input.body_json,
            &aether_ai_formats::FormatContext::default().with_mapped_model(input.mapped_model),
        )
        .ok()?
        .value
        .as_object()?
        .clone()
    };
    match input.family {
        SameFormatProviderFamily::Standard => {
            let previous_model = provider_request_body.get("model").cloned();
            provider_request_body.insert(
                "model".to_string(),
                Value::String(input.mapped_model.to_string()),
            );
            if previous_model != Some(Value::String(input.mapped_model.to_string())) {
                record_compatibility_edit(
                    &mut compatibility_edits,
                    "model",
                    SameFormatProviderCompatibilityEditAction::RuntimeRewrite,
                    "rewrote request model to mapped upstream model",
                );
            }
        }
        SameFormatProviderFamily::Gemini => {
            if aether_ai_formats::api_format_alias_matches(
                input.provider_api_format,
                "gemini:interactions",
            ) {
                let rewrite_field = if provider_request_body.contains_key("model") {
                    "model"
                } else if provider_request_body.contains_key("agent") {
                    "agent"
                } else {
                    "model"
                };
                let previous_value = provider_request_body.get(rewrite_field).cloned();
                provider_request_body.insert(
                    rewrite_field.to_string(),
                    Value::String(input.mapped_model.to_string()),
                );
                if previous_value != Some(Value::String(input.mapped_model.to_string())) {
                    record_compatibility_edit(
                        &mut compatibility_edits,
                        rewrite_field,
                        SameFormatProviderCompatibilityEditAction::RuntimeRewrite,
                        "rewrote Gemini Interactions routing field to mapped upstream target",
                    );
                }
            } else {
                if provider_request_body.remove("model").is_some() {
                    record_compatibility_edit(
                        &mut compatibility_edits,
                        "model",
                        SameFormatProviderCompatibilityEditAction::RuntimeRewrite,
                        "removed top-level model because Gemini model is carried by the upstream URL",
                    );
                }
            }
        }
    }
    let mut provider_request_body = Value::Object(provider_request_body);
    if input.is_claude_code {
        let before = compatibility_edits
            .is_some()
            .then(|| provider_request_body.clone());
        crate::claude_code::sanitize_claude_code_request_body(&mut provider_request_body);
        if before.is_some_and(|before| before != provider_request_body) {
            record_compatibility_edit(
                &mut compatibility_edits,
                "body",
                SameFormatProviderCompatibilityEditAction::ProviderCompatibilityRewrite,
                "sanitized Claude Code request body for provider compatibility",
            );
        }
    }
    if input.enable_model_directives {
        if let Some(source_model) = input.source_model {
            let before = compatibility_edits
                .is_some()
                .then(|| provider_request_body.clone());
            aether_ai_formats::apply_model_directive_overrides_from_model(
                &mut provider_request_body,
                input.provider_api_format,
                input.mapped_model,
                source_model,
            );
            if before.is_some_and(|before| before != provider_request_body) {
                record_compatibility_edit(
                    &mut compatibility_edits,
                    "model_directives",
                    SameFormatProviderCompatibilityEditAction::RuntimeRewrite,
                    "applied model directive request body overrides",
                );
            }
        }
    }
    let before_body_rules = compatibility_edits
        .is_some()
        .then(|| provider_request_body.clone());
    if !apply_local_body_rules_with_request_headers(
        &mut provider_request_body,
        input.body_rules,
        Some(input.body_json),
        input.request_headers,
    ) {
        return None;
    }
    if before_body_rules.is_some_and(|before| before != provider_request_body) {
        record_compatibility_edit(
            &mut compatibility_edits,
            "body_rules",
            SameFormatProviderCompatibilityEditAction::OperatorRule,
            "applied configured provider body rules",
        );
    }
    if matches!(input.family, SameFormatProviderFamily::Gemini)
        && aether_ai_formats::api_format_alias_matches(
            input.provider_api_format,
            "gemini:generate_content",
        )
    {
        let stripped = strip_gemini_function_response_ids(&mut provider_request_body);
        if stripped > 0 {
            record_compatibility_edit(
                &mut compatibility_edits,
                "contents[].parts[].functionResponse.id",
                SameFormatProviderCompatibilityEditAction::ProviderCompatibilityRewrite,
                format!(
                    "stripped {stripped} Gemini functionResponse id field(s) rejected by upstreams"
                ),
            );
        }
    }
    let require_body_stream_field = input.force_body_stream_field
        || input
            .body_json
            .as_object()
            .is_some_and(|object| object.contains_key("stream"));
    let previous_stream = provider_request_body.get("stream").cloned();
    aether_ai_formats::enforce_request_body_stream_field(
        &mut provider_request_body,
        input.provider_api_format,
        input.upstream_is_stream,
        require_body_stream_field,
    );
    if previous_stream != provider_request_body.get("stream").cloned() {
        record_compatibility_edit(
            &mut compatibility_edits,
            "stream",
            SameFormatProviderCompatibilityEditAction::RuntimeRewrite,
            format!(
                "enforced provider stream policy with upstream_is_stream={}",
                input.upstream_is_stream
            ),
        );
    }
    let stripped_reasoning_items =
        aether_ai_formats::strip_incompatible_openai_responses_reasoning_items(
            &mut provider_request_body,
            input.provider_api_format,
        );
    if stripped_reasoning_items > 0 {
        record_compatibility_edit(
            &mut compatibility_edits,
            "input[].id",
            SameFormatProviderCompatibilityEditAction::ProviderCompatibilityRewrite,
            format!(
                "stripped {stripped_reasoning_items} non-replayable OpenAI Responses reasoning item(s)"
            ),
        );
    }
    let provider_model = provider_request_body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(input.mapped_model)
        .to_string();
    if !same_format_reasoning_effort_supported(
        &provider_request_body,
        input.provider_api_format,
        &provider_model,
        input.source_model.unwrap_or(input.mapped_model),
    ) {
        return None;
    }
    if aether_ai_formats::finalize_openai_provider_request(
        &mut provider_request_body,
        aether_ai_formats::OpenAiProviderRequestFinalization {
            source_api_format: input.provider_api_format,
            provider_api_format: input.provider_api_format,
            provider_type: "",
            provider_model: &provider_model,
            source_model: input.source_model.unwrap_or(input.mapped_model),
            body_rules: input.body_rules,
            upstream_is_stream: input.upstream_is_stream,
            require_body_stream_field,
        },
    )
    .is_err()
    {
        return None;
    }
    Some(provider_request_body)
}

fn same_format_reasoning_effort_supported(
    body: &Value,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
) -> bool {
    let provider_api_format = aether_ai_formats::normalize_api_format_alias(provider_api_format);
    let effort = match provider_api_format.as_str() {
        "openai:chat" => body.get("reasoning_effort"),
        "openai:responses" | "openai:responses:compact" | "openai:search" => body
            .get("reasoning")
            .and_then(Value::as_object)
            .and_then(|reasoning| reasoning.get("effort")),
        _ => return true,
    };
    let Some(effort) = effort.and_then(Value::as_str) else {
        return true;
    };
    let Some(effort) = aether_ai_formats::ReasoningEffort::parse(effort) else {
        return true;
    };
    aether_ai_formats::reasoning_effort_supported_for_model(
        provider_api_format.as_str(),
        provider_model,
        source_model,
        effort,
    )
}

fn record_compatibility_edit(
    edits: &mut Option<&mut Vec<SameFormatProviderCompatibilityEdit>>,
    field: impl Into<String>,
    action: SameFormatProviderCompatibilityEditAction,
    detail: impl Into<String>,
) {
    if let Some(edits) = edits.as_mut() {
        edits.push(SameFormatProviderCompatibilityEdit {
            field: field.into(),
            action,
            detail: detail.into(),
        });
    }
}

fn embedding_multimodal_input_requires_aliyun_provider(
    client_api_format: &str,
    provider_api_format: &str,
    body_json: &Value,
) -> bool {
    aether_ai_formats::is_embedding_api_format(client_api_format)
        && embedding_input_is_multimodal(body_json.get("input"))
        && aether_ai_formats::normalize_api_format_alias(provider_api_format)
            != "aliyun:multimodal_embedding"
}

fn embedding_input_is_multimodal(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty() && items.iter().all(embedding_content_is_multimodal))
}

fn embedding_content_is_multimodal(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        ["text", "image", "video", "multi_images"]
            .iter()
            .any(|key| object.contains_key(*key))
    })
}

fn strip_gemini_function_response_ids(value: &mut Value) -> usize {
    match value {
        Value::Object(object) => {
            let mut stripped = 0;
            for key in ["functionResponse", "function_response"] {
                if let Some(function_response) = object.get_mut(key).and_then(Value::as_object_mut)
                {
                    if function_response.remove("id").is_some() {
                        stripped += 1;
                    }
                }
            }
            for child in object.values_mut() {
                stripped += strip_gemini_function_response_ids(child);
            }
            stripped
        }
        Value::Array(items) => {
            let mut stripped = 0;
            for item in items {
                stripped += strip_gemini_function_response_ids(item);
            }
            stripped
        }
        _ => 0,
    }
}

pub fn build_same_format_provider_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    params: SameFormatProviderUpstreamUrlParams<'_>,
) -> Option<String> {
    build_transport_request_url_for_request_body(
        transport,
        TransportRequestUrlParams {
            provider_api_format: params.provider_api_format,
            mapped_model: Some(params.mapped_model),
            upstream_is_stream: params.upstream_is_stream,
            request_query: params.request_query,
            kiro_api_region: params.kiro_api_region,
            api_operation: params.api_operation,
        },
        params.provider_request_body,
    )
}

pub fn build_same_format_provider_headers(
    input: SameFormatProviderHeadersInput<'_>,
) -> Option<BTreeMap<String, String>> {
    if let Some(kiro_auth_config) = input.kiro_auth_config {
        return build_kiro_provider_headers(KiroProviderHeadersInput {
            headers: input.headers,
            provider_request_body: input.provider_request_body,
            original_request_body: input.original_request_body,
            header_rules: input.header_rules,
            auth_header: input.auth_header.unwrap_or_default(),
            auth_value: input.auth_value.unwrap_or_default(),
            auth_config: kiro_auth_config,
            machine_id: input.kiro_machine_id.unwrap_or_default(),
        });
    }

    let auth_header = input.auth_header.unwrap_or_default();
    let auth_value = input.auth_value.unwrap_or_default();
    let mut provider_request_headers = if input.behavior.is_claude_code_transport {
        build_claude_code_passthrough_headers(
            input.headers,
            auth_header,
            auth_value,
            input.extra_headers,
            input.behavior.upstream_is_stream,
        )
    } else if input.behavior.is_vertex {
        build_complete_passthrough_headers(
            input.headers,
            input.extra_headers,
            Some("application/json"),
        )
    } else {
        build_complete_passthrough_headers_with_auth(
            input.headers,
            auth_header,
            auth_value,
            input.extra_headers,
            Some("application/json"),
        )
    };

    let mut protected_headers = upstream_credential_header_names().to_vec();
    protected_headers.push("content-type");
    if !apply_local_header_rules_with_request_headers(
        &mut provider_request_headers,
        input.header_rules,
        &protected_headers,
        input.provider_request_body,
        Some(input.original_request_body),
        Some(input.headers),
    ) {
        return None;
    }
    if let (Some(auth_header), Some(auth_value)) = (input.auth_header, input.auth_value) {
        replace_upstream_auth_headers(&mut provider_request_headers, auth_header, auth_value);
    } else {
        replace_upstream_auth_headers(&mut provider_request_headers, "", "");
    }
    let claude_code_profile = *current_claude_code_transport_identity_profile();
    if input.behavior.is_claude_code_transport {
        claude_code_profile.apply_fixed_headers(
            &mut provider_request_headers,
            input.behavior.upstream_is_stream,
        );
    }
    if input.behavior.is_claude_code_transport || input.behavior.is_claude_code {
        claude_code_profile.apply_beta_policy(&mut provider_request_headers, input.api_operation);
    }
    if matches!(
        input.api_operation,
        Some(aether_ai_formats::ApiOperation::ClaudeCountTokens)
    ) {
        provider_request_headers.insert("accept".to_string(), "application/json".to_string());
    } else if input.behavior.upstream_is_stream {
        provider_request_headers.insert("accept".to_string(), "text/event-stream".to_string());
        force_identity_accept_encoding(&mut provider_request_headers);
    }
    Some(provider_request_headers)
}

pub fn same_format_provider_transport_supported(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
    api_format: &str,
) -> bool {
    same_format_provider_transport_unsupported_reason(behavior, transport, family, api_format)
        .is_none()
}

pub fn same_format_provider_transport_unsupported_reason(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
    api_format: &str,
) -> Option<&'static str> {
    if is_grok_provider_transport(transport) && matches!(family, SameFormatProviderFamily::Standard)
    {
        return None;
    }
    if behavior.is_kiro {
        local_kiro_request_transport_unsupported_reason_with_network(transport)
    } else if behavior.is_antigravity {
        None
    } else if behavior.is_claude_code_transport {
        local_claude_code_transport_unsupported_reason_with_network(transport, api_format)
    } else if behavior.is_vertex {
        local_vertex_gemini_transport_unsupported_reason_with_network(transport)
    } else {
        match family {
            SameFormatProviderFamily::Standard => {
                local_standard_transport_unsupported_reason_with_network(transport, api_format)
            }
            SameFormatProviderFamily::Gemini => {
                local_gemini_transport_unsupported_reason_with_network(transport, api_format)
            }
        }
    }
}

pub fn same_format_provider_transport_unsupported_reason_for_trace(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Option<&'static str> {
    let normalized_api_format =
        match aether_ai_formats::normalize_api_format_alias(provider_api_format).as_str() {
            "openai:chat" => "openai:chat",
            "openai:responses" => "openai:responses",
            "openai:responses:compact" => "openai:responses:compact",
            "openai:search" => "openai:search",
            "claude:messages" => "claude:messages",
            "gemini:generate_content" => "gemini:generate_content",
            "gemini:interactions" => "gemini:interactions",
            _ => return Some("transport_api_format_unsupported"),
        };
    let behavior = classify_same_format_provider_request_behavior(
        transport,
        SameFormatProviderRequestBehaviorParams {
            require_streaming: false,
            provider_api_format: normalized_api_format,
            report_kind: "trace_candidate_metadata",
        },
    );
    if !behavior.is_antigravity
        && !behavior.is_claude_code_transport
        && !behavior.is_gemini_cli
        && !behavior.is_vertex
        && !behavior.is_kiro
    {
        return None;
    }

    let family = if normalized_api_format.starts_with("gemini:") {
        SameFormatProviderFamily::Gemini
    } else {
        SameFormatProviderFamily::Standard
    };
    same_format_provider_transport_unsupported_reason(
        &behavior,
        transport,
        family,
        normalized_api_format,
    )
}

pub fn should_try_same_format_provider_oauth_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
    provider_api_format: &str,
) -> bool {
    let provider_api_format = aether_ai_formats::normalize_api_format_alias(provider_api_format);
    behavior.is_kiro
        || matches!(family, SameFormatProviderFamily::Standard)
            && resolve_same_format_standard_direct_auth(transport, provider_api_format.as_str())
                .is_none()
        || matches!(family, SameFormatProviderFamily::Gemini)
            && behavior.is_vertex
            && is_vertex_service_account_transport_context(transport)
        || matches!(family, SameFormatProviderFamily::Gemini)
            && !behavior.is_vertex
            && resolve_local_gemini_auth(transport).is_none()
}

pub fn resolve_same_format_provider_direct_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: SameFormatProviderFamily,
    provider_api_format: &str,
) -> Option<(String, String)> {
    if is_grok_provider_transport(transport) && matches!(family, SameFormatProviderFamily::Standard)
    {
        return resolve_grok_session_auth(transport);
    }
    if behavior.is_vertex {
        None
    } else {
        match family {
            SameFormatProviderFamily::Standard => {
                resolve_same_format_standard_direct_auth(transport, provider_api_format)
            }
            SameFormatProviderFamily::Gemini => resolve_local_gemini_auth(transport),
        }
    }
}

fn resolve_same_format_standard_direct_auth(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Option<(String, String)> {
    if aether_ai_formats::api_format_alias_matches(provider_api_format, "openai:embedding")
        || aether_ai_formats::api_format_alias_matches(provider_api_format, "openai:search")
    {
        resolve_local_openai_bearer_auth(transport)
    } else {
        resolve_local_standard_auth(transport)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use serde_json::json;

    fn sample_transport(provider_type: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider".to_string(),
                provider_type: provider_type.to_string(),
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
                api_format: "openai:chat".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://api.example.test".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
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
                api_formats: None,
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
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn classifies_streaming_and_report_kind_for_provider_private_transports() {
        let kiro = sample_transport("kiro");
        let behavior = classify_same_format_provider_request_behavior(
            &kiro,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "claude:messages",
                report_kind: "claude_chat_sync_success",
            },
        );

        assert!(behavior.is_kiro);
        assert!(behavior.upstream_is_stream);
        assert_eq!(behavior.report_kind, "claude_cli_sync_finalize");

        let antigravity = sample_transport("antigravity");
        let behavior = classify_same_format_provider_request_behavior(
            &antigravity,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "gemini:generate_content",
                report_kind: "gemini_chat_sync_success",
            },
        );

        assert!(behavior.is_antigravity);
        assert!(behavior.upstream_is_stream);
        assert_eq!(behavior.report_kind, "gemini_chat_sync_finalize");

        let gemini_cli = sample_transport("gemini_cli");
        let behavior = classify_same_format_provider_request_behavior(
            &gemini_cli,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "gemini:generate_content",
                report_kind: "gemini_cli_sync_success",
            },
        );

        assert!(!behavior.upstream_is_stream);
        assert_eq!(behavior.report_kind, "gemini_cli_sync_finalize");
    }

    #[test]
    fn anthropic_compatibility_profile_controls_legacy_claude_code_edits() {
        let mut native = sample_transport("custom");
        native.endpoint.api_format = "claude:messages".to_string();
        let native_behavior = classify_same_format_provider_request_behavior(
            &native,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "claude:messages",
                report_kind: "claude_chat_sync_success",
            },
        );
        assert!(!native_behavior.is_claude_code);
        assert!(!native_behavior.is_claude_code_transport);
        assert_eq!(
            native_behavior.anthropic_compatibility_profile,
            AnthropicCompatibilityProfile::NativeTransparent
        );

        native.endpoint.config = Some(json!({
            "anthropic": {"compatibility_profile": "claude_code_legacy"}
        }));
        let compat_behavior = classify_same_format_provider_request_behavior(
            &native,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "claude:messages",
                report_kind: "claude_chat_sync_success",
            },
        );
        assert!(compat_behavior.is_claude_code);
        assert!(!compat_behavior.is_claude_code_transport);
        assert_eq!(
            compat_behavior.anthropic_compatibility_profile,
            AnthropicCompatibilityProfile::ClaudeCodeLegacy
        );

        let request_body = json!({
            "model": "claude-client",
            "thinking": {"type": "enabled"},
            "context_management": {"edits": [{"type": "client_strategy"}]},
            "system": [{
                "type": "text",
                "text": "x-anthropic-billing-header: cc_version=9.9.9.abc; cc_entrypoint=cli;"
            }],
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "visible"},
                    {"type": "thinking", "thinking": "unsigned", "signature": ""}
                ]
            }]
        });
        let build_body = |behavior: SameFormatProviderRequestBehavior| {
            build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
                body_json: &request_body,
                mapped_model: "claude-upstream",
                client_api_format: "claude:messages",
                provider_api_format: "claude:messages",
                source_model: Some("claude-client"),
                family: SameFormatProviderFamily::Standard,
                body_rules: None,
                request_headers: None,
                upstream_is_stream: false,
                force_body_stream_field: false,
                kiro_auth_config: None,
                is_claude_code: behavior.is_claude_code,
                enable_model_directives: false,
            })
            .expect("body should build")
        };
        assert_eq!(
            build_body(native_behavior)["messages"][0]["content"]
                .as_array()
                .map(Vec::len),
            Some(2),
            "native Anthropic requests must remain untouched"
        );
        assert_eq!(
            build_body(native_behavior)["system"][0]["text"],
            request_body["system"][0]["text"],
            "native transparent requests must not rewrite billing identity"
        );
        assert_eq!(
            build_body(native_behavior)["context_management"],
            request_body["context_management"],
            "native transparent requests must not apply Claude Code body gates"
        );
        assert_eq!(
            build_body(compat_behavior)["messages"][0]["content"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "legacy compatibility may sanitize invalid thinking blocks"
        );
        assert_eq!(
            build_body(compat_behavior)["system"][0]["text"],
            "x-anthropic-billing-header: cc_version=2.1.161.abc; cc_entrypoint=cli;"
        );

        let mut legacy = sample_transport("claude_code");
        legacy.endpoint.api_format = "claude:messages".to_string();
        legacy.endpoint.config = Some(json!({
            "anthropic": {"compatibility_profile": "native_transparent"}
        }));
        let transparent_legacy_transport = classify_same_format_provider_request_behavior(
            &legacy,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "claude:messages",
                report_kind: "claude_chat_sync_success",
            },
        );
        assert!(!transparent_legacy_transport.is_claude_code);
        assert!(transparent_legacy_transport.is_claude_code_transport);

        let provider_request_body = json!({"model": "claude-upstream"});
        let empty_headers = http::HeaderMap::new();
        let empty_extra_headers = BTreeMap::new();
        let build_headers =
            |behavior: SameFormatProviderRequestBehavior,
             api_operation: Option<aether_ai_formats::ApiOperation>| {
                build_same_format_provider_headers(SameFormatProviderHeadersInput {
                    headers: &empty_headers,
                    provider_request_body: &provider_request_body,
                    original_request_body: &request_body,
                    header_rules: None,
                    behavior,
                    api_operation,
                    auth_header: Some("x-api-key"),
                    auth_value: Some("upstream-secret"),
                    extra_headers: &empty_extra_headers,
                    kiro_auth_config: None,
                    kiro_machine_id: None,
                })
                .expect("headers should build")
            };
        assert!(
            !build_headers(compat_behavior, None).contains_key("x-app"),
            "compatibility profile must not impersonate the Claude Code transport"
        );
        assert_eq!(
            build_headers(transparent_legacy_transport, None)
                .get("x-app")
                .map(String::as_str),
            Some("cli"),
            "Claude Code transport headers must survive a transparent body profile"
        );
        assert!(
            !build_headers(
                native_behavior,
                Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
            )
            .contains_key("anthropic-beta"),
            "native transparent token counting must not inject compatibility betas"
        );
        assert!(
            build_headers(
                compat_behavior,
                Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
            )["anthropic-beta"]
                .split(',')
                .any(|token| token == "token-counting-2024-11-01"),
            "legacy compatibility token counting requires the token-counting beta"
        );
        assert!(
            build_headers(
                transparent_legacy_transport,
                Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
            )["anthropic-beta"]
                .split(',')
                .any(|token| token == "token-counting-2024-11-01"),
            "Claude Code transport token counting requires the token-counting beta"
        );
    }

    #[test]
    fn same_format_behavior_resolves_endpoint_stream_policy() {
        let mut force_stream = sample_transport("openai");
        force_stream.endpoint.config = Some(json!({
            "upstream_stream_policy": "force_stream"
        }));
        let behavior = classify_same_format_provider_request_behavior(
            &force_stream,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "openai:chat",
                report_kind: "openai_chat_sync_success",
            },
        );
        assert!(behavior.upstream_is_stream);

        let mut force_non_stream = sample_transport("openai");
        force_non_stream.endpoint.config = Some(json!({
            "upstreamStreamPolicy": "force_non_stream"
        }));
        let behavior = classify_same_format_provider_request_behavior(
            &force_non_stream,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: true,
                provider_api_format: "openai:chat",
                report_kind: "openai_chat_stream_success",
            },
        );
        assert!(!behavior.upstream_is_stream);

        let mut auto = sample_transport("openai");
        auto.endpoint.config = Some(json!({
            "upstream_stream": "auto"
        }));
        let stream_behavior = classify_same_format_provider_request_behavior(
            &auto,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: true,
                provider_api_format: "openai:chat",
                report_kind: "openai_chat_stream_success",
            },
        );
        assert!(stream_behavior.upstream_is_stream);
        let sync_behavior = classify_same_format_provider_request_behavior(
            &auto,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "openai:chat",
                report_kind: "openai_chat_sync_success",
            },
        );
        assert!(!sync_behavior.upstream_is_stream);

        let mut compact = sample_transport("codex");
        compact.endpoint.config = Some(json!({
            "upstream_stream_policy": "force_stream"
        }));
        let compact_behavior = classify_same_format_provider_request_behavior(
            &compact,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: true,
                provider_api_format: "openai:responses:compact",
                report_kind: "openai_responses_compact_sync_success",
            },
        );
        assert!(!compact_behavior.upstream_is_stream);

        let mut search = sample_transport("codex");
        search.endpoint.config = Some(json!({
            "upstream_stream_policy": "force_stream"
        }));
        let search_behavior = classify_same_format_provider_request_behavior(
            &search,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: true,
                provider_api_format: "openai:search",
                report_kind: "openai_search_sync_success",
            },
        );
        assert!(!search_behavior.upstream_is_stream);
        assert!(!search_behavior.force_body_stream_field);
    }

    #[test]
    fn count_tokens_overrides_endpoint_stream_policy_and_removes_stream_field() {
        let mut transport = sample_transport("custom");
        transport.endpoint.api_format = "claude:messages".to_string();
        transport.endpoint.config = Some(json!({
            "upstream_stream_policy": "force_stream"
        }));
        let behavior = classify_same_format_provider_request_behavior_for_operation(
            &transport,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "claude:messages",
                report_kind: "claude_count_tokens_sync_success",
            },
            Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
        );

        assert!(!behavior.upstream_is_stream);
        assert!(!behavior.force_body_stream_field);

        let mut body = json!({"model": "claude-sonnet-4", "messages": [], "stream": true});
        assert!(enforce_same_format_provider_api_operation_body_policy(
            &mut body,
            Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
        ));
        assert!(body.get("stream").is_none());

        let headers = build_same_format_provider_headers(SameFormatProviderHeadersInput {
            headers: &http::HeaderMap::new(),
            provider_request_body: &body,
            original_request_body: &body,
            header_rules: None,
            behavior,
            api_operation: Some(aether_ai_formats::ApiOperation::ClaudeCountTokens),
            auth_header: Some("x-api-key"),
            auth_value: Some("secret"),
            extra_headers: &BTreeMap::new(),
            kiro_auth_config: None,
            kiro_machine_id: None,
        })
        .expect("headers should build");
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/json")
        );
    }

    #[test]
    fn same_format_behavior_preserves_hard_streaming_constraint() {
        let mut kiro = sample_transport("kiro");
        kiro.endpoint.config = Some(json!({
            "upstream_stream_policy": "force_non_stream"
        }));

        let behavior = classify_same_format_provider_request_behavior(
            &kiro,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: true,
                provider_api_format: "claude:messages",
                report_kind: "claude_chat_stream_success",
            },
        );

        assert!(behavior.upstream_is_stream);
    }

    #[test]
    fn same_format_behavior_preserves_gemini_cli_streaming_requests() {
        let mut gemini_cli = sample_transport("gemini_cli");
        gemini_cli.endpoint.config = Some(json!({
            "upstream_stream_policy": "force_non_stream"
        }));

        let stream_behavior = classify_same_format_provider_request_behavior(
            &gemini_cli,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: true,
                provider_api_format: "gemini:generate_content",
                report_kind: "gemini_cli_stream_success",
            },
        );
        assert!(stream_behavior.upstream_is_stream);

        let sync_behavior = classify_same_format_provider_request_behavior(
            &gemini_cli,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "gemini:generate_content",
                report_kind: "gemini_cli_sync_success",
            },
        );
        assert!(!sync_behavior.upstream_is_stream);
    }

    #[test]
    fn same_format_policy_resolution_drives_standard_body_stream_field() {
        for (endpoint_config, client_is_stream, expected_stream) in [
            (
                json!({"upstream_stream_policy": "force_stream"}),
                false,
                true,
            ),
            (
                json!({"upstreamStreamPolicy": "force_non_stream"}),
                true,
                false,
            ),
            (json!({"upstream_stream": "auto"}), true, true),
            (json!({"upstream_stream": "auto"}), false, false),
        ] {
            let mut transport = sample_transport("openai");
            transport.endpoint.config = Some(endpoint_config);
            let behavior = classify_same_format_provider_request_behavior(
                &transport,
                SameFormatProviderRequestBehaviorParams {
                    require_streaming: client_is_stream,
                    provider_api_format: "openai:chat",
                    report_kind: "openai_chat_policy_test",
                },
            );
            let body =
                build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
                    body_json: &json!({
                        "model": "client-model",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": client_is_stream
                    }),
                    mapped_model: "upstream-model",
                    client_api_format: "openai:chat",
                    provider_api_format: "openai:chat",
                    source_model: Some("client-model"),
                    family: SameFormatProviderFamily::Standard,
                    body_rules: None,
                    request_headers: None,
                    upstream_is_stream: behavior.upstream_is_stream,
                    force_body_stream_field: behavior.force_body_stream_field,
                    kiro_auth_config: None,
                    is_claude_code: false,
                    enable_model_directives: false,
                })
                .expect("body should build");

            assert_eq!(body.get("stream"), Some(&json!(expected_stream)));
        }
    }

    #[test]
    fn resolves_direct_auth_except_vertex() {
        let transport = sample_transport("openai");
        let behavior = classify_same_format_provider_request_behavior(
            &transport,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "openai:chat",
                report_kind: "openai_chat_sync_success",
            },
        );

        assert_eq!(
            resolve_same_format_provider_direct_auth(
                &behavior,
                &transport,
                SameFormatProviderFamily::Standard,
                "openai:chat",
            ),
            Some(("x-api-key".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn resolves_openai_embedding_direct_auth_with_bearer_header() {
        let mut transport = sample_transport("custom");
        transport.endpoint.api_format = "openai:embedding".to_string();
        transport.key.auth_type = "api_key".to_string();
        let behavior = classify_same_format_provider_request_behavior(
            &transport,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "openai:embedding",
                report_kind: "openai_embedding_sync_success",
            },
        );

        assert_eq!(
            resolve_same_format_provider_direct_auth(
                &behavior,
                &transport,
                SameFormatProviderFamily::Standard,
                "openai:embedding",
            ),
            Some(("authorization".to_string(), "Bearer secret".to_string()))
        );
    }

    #[test]
    fn resolves_openai_search_direct_auth_with_bearer_header() {
        let mut transport = sample_transport("custom");
        transport.endpoint.api_format = "openai:search".to_string();
        transport.key.auth_type = "api_key".to_string();
        let behavior = classify_same_format_provider_request_behavior(
            &transport,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "openai:search",
                report_kind: "openai_search_sync_success",
            },
        );

        assert_eq!(
            resolve_same_format_provider_direct_auth(
                &behavior,
                &transport,
                SameFormatProviderFamily::Standard,
                "openai:search",
            ),
            Some(("authorization".to_string(), "Bearer secret".to_string()))
        );
    }

    #[test]
    fn keeps_claude_same_format_api_key_on_x_api_key_header() {
        let mut transport = sample_transport("custom");
        transport.endpoint.api_format = "claude:messages".to_string();
        transport.key.auth_type = "api_key".to_string();
        let behavior = classify_same_format_provider_request_behavior(
            &transport,
            SameFormatProviderRequestBehaviorParams {
                require_streaming: false,
                provider_api_format: "claude:messages",
                report_kind: "claude_chat_sync_success",
            },
        );

        assert_eq!(
            resolve_same_format_provider_direct_auth(
                &behavior,
                &transport,
                SameFormatProviderFamily::Standard,
                "claude:messages",
            ),
            Some(("x-api-key".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn builds_same_format_standard_body_with_mapped_model_and_stream_flag() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}]
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:chat",
            provider_api_format: "openai:chat",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: true,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert_eq!(body.get("model"), Some(&json!("upstream-model")));
        assert_eq!(body.get("stream"), Some(&json!(true)));
    }

    #[test]
    fn same_format_standard_body_preserves_fields_that_cross_format_would_block() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}],
                "n": 2,
                "reasoning_effort": "max",
                "unknown_vendor_field": {"keep": true}
            }),
            mapped_model: "client-model",
            client_api_format: "openai:chat",
            provider_api_format: "/v1/chat/completions",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("same-format body should bypass canonical conversion");

        assert_eq!(body["n"], 2);
        assert_eq!(body["reasoning_effort"], "max");
        assert_eq!(body["unknown_vendor_field"]["keep"], true);
    }

    #[test]
    fn same_format_responses_body_preserves_gpt_5_6_protocol_contract_verbatim() {
        let input = json!([
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "before",
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    },
                    {"type": "input_text", "text": "after"}
                ]
            },
            {
                "type": "program",
                "call_id": "program-call-1",
                "fingerprint": "fp-program-1"
            },
            {
                "type": "program_output",
                "call_id": "program-call-1",
                "result": "hello",
                "status": "completed"
            },
            {
                "type": "multi_agent_call",
                "call_id": "agent-call-1",
                "action": "delegate",
                "arguments": {"query": "release notes"},
                "agent": "researcher"
            },
            {
                "type": "agent_message",
                "author": "researcher",
                "recipient": "assistant",
                "encrypted_content": "encrypted-agent-message"
            },
            {
                "type": "function_call_output",
                "call_id": "call-1",
                "output": "ok",
                "caller": {"type": "multi_agent", "id": "agent-call-1"}
            }
        ]);
        let prompt_cache_options = json!({
            "mode": "explicit",
            "ttl": "30m"
        });
        let body_json = json!({
            "model": "gpt-5.6-sol",
            "input": input,
            "reasoning": {
                "effort": "max",
                "mode": "pro",
                "context": "all_turns"
            },
            "prompt_cache_options": prompt_cache_options,
            "multi_agent": {
                "enabled": true,
                "max_concurrent_subagents": 2
            }
        });

        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &body_json,
            mapped_model: "gpt-5.6-sol",
            client_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            source_model: Some("gpt-5.6-sol"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("valid GPT-5.6 Responses request should use raw same-format transport");

        assert_eq!(body["input"], input);
        assert_eq!(body["reasoning"], body_json["reasoning"]);
        assert_eq!(body["prompt_cache_options"], prompt_cache_options);
        assert_eq!(body["multi_agent"], body_json["multi_agent"]);
    }

    #[test]
    fn same_format_prompt_cache_capability_uses_mapped_provider_model() {
        let body_json = json!({
            "model": "gpt-5.5",
            "messages": [{"role": "user", "content": "hello"}],
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"}
        });
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &body_json,
            mapped_model: "gpt-5.6-sol",
            client_api_format: "openai:chat",
            provider_api_format: "openai:chat",
            source_model: Some("gpt-5.5"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("final GPT-5.6 provider model should accept prompt_cache_options");

        assert_eq!(body["model"], "gpt-5.6-sol");
        assert_eq!(
            body["prompt_cache_options"],
            body_json["prompt_cache_options"]
        );

        let rejected =
            build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
                body_json: &body_json,
                mapped_model: "gpt-5.5",
                client_api_format: "openai:chat",
                provider_api_format: "openai:chat",
                source_model: Some("gpt-5.5"),
                family: SameFormatProviderFamily::Standard,
                body_rules: None,
                request_headers: None,
                upstream_is_stream: false,
                force_body_stream_field: false,
                kiro_auth_config: None,
                is_claude_code: false,
                enable_model_directives: false,
            });
        assert!(rejected.is_none());
    }

    #[test]
    fn same_format_reasoning_capability_uses_mapped_provider_model() {
        let body_json = json!({
            "model": "deployment-alias",
            "input": "hello",
            "reasoning": {"effort": "max"}
        });
        let build = |mapped_model: &str| {
            build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
                body_json: &body_json,
                mapped_model,
                client_api_format: "openai:responses",
                provider_api_format: "openai:responses",
                source_model: Some("deployment-alias"),
                family: SameFormatProviderFamily::Standard,
                body_rules: None,
                request_headers: None,
                upstream_is_stream: false,
                force_body_stream_field: false,
                kiro_auth_config: None,
                is_claude_code: false,
                enable_model_directives: false,
            })
        };

        assert!(build("gpt-5.6-sol").is_some());
        assert!(build("gpt-5.4").is_none());
    }

    #[test]
    fn same_format_gpt_5_6_requires_prompt_cache_options() {
        let body_json = json!({
            "model": "client-alias",
            "input": "hello",
            "prompt_cache_retention": "24h"
        });
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &body_json,
            mapped_model: "gpt-5.6-luna",
            client_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            source_model: Some("client-alias"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        });

        assert!(body.is_none());
    }

    #[test]
    fn same_format_responses_body_preserves_opaque_extension_fields() {
        let opaque_option = json!({"mode": "custom"});
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "gpt-5.6-sol",
                "input": "hello",
                "multi_agent": {"enabled": true},
                "opaque_responses_option": opaque_option
            }),
            mapped_model: "gpt-5.6-sol",
            client_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            source_model: Some("gpt-5.6-sol"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        });

        let body = body.expect("same-format Responses request should remain transparent");
        assert_eq!(body["multi_agent"], json!({"enabled": true}));
        assert_eq!(body["opaque_responses_option"], opaque_option);
    }

    #[test]
    fn cross_format_standard_body_fails_closed_for_lossy_chat_fields() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}],
                "n": 2
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:chat",
            provider_api_format: "openai:responses",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        });

        assert!(body.is_none());
    }

    #[test]
    fn same_format_embedding_body_rejects_multimodal_for_openai_like_provider() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "qwen3-vl-embedding",
                "input": [
                    {"text": "white running shoes"},
                    {"image": "https://example.com/shoe.png"}
                ]
            }),
            mapped_model: "openai-qwen-fallback",
            client_api_format: "openai:embedding",
            provider_api_format: "openai:embedding",
            source_model: Some("qwen3-vl-embedding"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        });

        assert!(body.is_none());
    }

    #[test]
    fn same_format_standard_body_overrides_client_stream_for_non_stream_upstream() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:chat",
            provider_api_format: "openai:chat",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert_eq!(body.get("model"), Some(&json!("upstream-model")));
        assert_eq!(body.get("stream"), Some(&json!(false)));
    }

    #[test]
    fn same_format_standard_body_does_not_add_stream_false_for_plain_sync_body() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}]
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:chat",
            provider_api_format: "openai:chat",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert_eq!(body.get("model"), Some(&json!("upstream-model")));
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn same_format_standard_body_forced_policy_adds_stream_false() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}]
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:chat",
            provider_api_format: "openai:chat",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: true,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert_eq!(body.get("model"), Some(&json!("upstream-model")));
        assert_eq!(body.get("stream"), Some(&json!(false)));
    }

    #[test]
    fn same_format_gemini_body_removes_leaked_client_stream_field() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
                "stream": true
            }),
            mapped_model: "gemini-upstream",
            client_api_format: "gemini:generate_content",
            provider_api_format: "gemini:generate_content",
            source_model: None,
            family: SameFormatProviderFamily::Gemini,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert!(body.get("stream").is_none());
    }

    #[test]
    fn same_format_gemini_interactions_body_keeps_body_routing_target() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "input": [{"role": "user", "content": "hello"}],
                "stream": true
            }),
            mapped_model: "gemini-3.5-flash",
            client_api_format: "gemini:interactions",
            provider_api_format: "gemini:interactions",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Gemini,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: true,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert_eq!(body.get("model"), Some(&json!("gemini-3.5-flash")));
        assert_eq!(body.get("stream"), Some(&json!(true)));
    }

    #[test]
    fn same_format_gemini_interactions_body_rewrites_agent_target() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "agent": "antigravity-preview-05-2026",
                "input": "hello"
            }),
            mapped_model: "antigravity-preview-05-2026",
            client_api_format: "gemini:interactions",
            provider_api_format: "gemini:interactions",
            source_model: None,
            family: SameFormatProviderFamily::Gemini,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert_eq!(
            body.get("agent"),
            Some(&json!("antigravity-preview-05-2026"))
        );
        assert!(body.get("model").is_none());
    }

    #[test]
    fn same_format_gemini_body_strips_function_response_id_for_upstream() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "contents": [
                    {
                        "role": "user",
                        "parts": [
                            {
                                "functionResponse": {
                                    "id": "call_123",
                                    "name": "lookup",
                                    "response": {
                                        "result": {
                                            "id": "keep_result_id",
                                            "ok": true
                                        }
                                    }
                                }
                            },
                            {
                                "function_response": {
                                    "id": "call_456",
                                    "name": "lookup_snake",
                                    "response": {"ok": true}
                                }
                            }
                        ]
                    }
                ],
                "stream": true
            }),
            mapped_model: "gemini-upstream",
            client_api_format: "gemini:generate_content",
            provider_api_format: "gemini:generate_content",
            source_model: None,
            family: SameFormatProviderFamily::Gemini,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: true,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        let function_response = &body["contents"][0]["parts"][0]["functionResponse"];
        assert!(function_response.get("id").is_none());
        assert_eq!(function_response["name"], "lookup");
        assert_eq!(
            function_response["response"]["result"]["id"],
            "keep_result_id"
        );

        let function_response = &body["contents"][0]["parts"][1]["function_response"];
        assert!(function_response.get("id").is_none());
        assert_eq!(function_response["name"], "lookup_snake");
    }

    #[test]
    fn same_format_body_report_records_provider_compatibility_edits() {
        let output = build_same_format_provider_request_body_with_compatibility_report(
            SameFormatProviderRequestBodyInput {
                body_json: &json!({
                    "model": "client-model",
                    "contents": [
                        {
                            "role": "user",
                            "parts": [
                                {
                                    "functionResponse": {
                                        "id": "call_123",
                                        "name": "lookup",
                                        "response": {"ok": true}
                                    }
                                },
                                {
                                    "function_response": {
                                        "id": "call_456",
                                        "name": "lookup_snake",
                                        "response": {"ok": true}
                                    }
                                }
                            ]
                        }
                    ],
                    "stream": true
                }),
                mapped_model: "gemini-upstream",
                client_api_format: "gemini:generate_content",
                provider_api_format: "gemini:generate_content",
                source_model: Some("client-model"),
                family: SameFormatProviderFamily::Gemini,
                body_rules: None,
                request_headers: None,
                upstream_is_stream: false,
                force_body_stream_field: false,
                kiro_auth_config: None,
                is_claude_code: false,
                enable_model_directives: false,
            },
        )
        .expect("same-format body should build");

        assert!(output.body.get("model").is_none());
        assert!(output.body.get("stream").is_none());
        assert!(output
            .body
            .pointer("/contents/0/parts/0/functionResponse/id")
            .is_none());
        assert!(output
            .body
            .pointer("/contents/0/parts/1/function_response/id")
            .is_none());
        assert!(output.compatibility_edits.iter().any(|edit| {
            edit.field == "model"
                && edit.action == SameFormatProviderCompatibilityEditAction::RuntimeRewrite
        }));
        assert!(output.compatibility_edits.iter().any(|edit| {
            edit.field == "stream"
                && edit.action == SameFormatProviderCompatibilityEditAction::RuntimeRewrite
        }));
        assert!(output.compatibility_edits.iter().any(|edit| {
            edit.field == "contents[].parts[].functionResponse.id"
                && edit.action
                    == SameFormatProviderCompatibilityEditAction::ProviderCompatibilityRewrite
                && edit.detail.contains("2")
        }));
    }

    #[test]
    fn same_format_responses_body_strips_foreign_reasoning_ids_and_reports_the_edit() {
        let request_body = json!({
            "model": "gpt-5.4",
            "input": [
                {"type": "reasoning", "id": "rs_provider_123", "summary": []},
                {
                    "type": "reasoning",
                    "id": "item_72d3bd8d367d01977ace23f1",
                    "summary": []
                },
                {"type": "message", "role": "user", "content": "continue"}
            ]
        });
        let output = build_same_format_provider_request_body_with_compatibility_report(
            SameFormatProviderRequestBodyInput {
                body_json: &request_body,
                mapped_model: "gpt-5.4",
                client_api_format: "openai:responses",
                provider_api_format: "openai:responses",
                source_model: Some("gpt-5.4"),
                family: SameFormatProviderFamily::Standard,
                body_rules: None,
                request_headers: None,
                upstream_is_stream: false,
                force_body_stream_field: false,
                kiro_auth_config: None,
                is_claude_code: false,
                enable_model_directives: false,
            },
        )
        .expect("same-format Responses body should build");

        let input = output.body["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["id"], "rs_provider_123");
        assert_eq!(input[1]["type"], "message");
        assert!(output.compatibility_edits.iter().any(|edit| {
            edit.field == "input[].id"
                && edit.action
                    == SameFormatProviderCompatibilityEditAction::ProviderCompatibilityRewrite
        }));
    }

    #[test]
    fn same_format_stream_policy_wins_after_body_rules() {
        let body_rules = json!([
            {"action":"set","path":"stream","value":true}
        ]);

        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:chat",
            provider_api_format: "openai:chat",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: Some(&body_rules),
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert_eq!(body.get("stream"), Some(&json!(false)));
    }

    #[test]
    fn same_format_compact_stream_policy_wins_after_body_rules() {
        let body_rules = json!([
            {"action":"set","path":"stream","value":true}
        ]);

        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "client-model",
                "input": "hello",
                "stream": true
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:responses:compact",
            provider_api_format: "openai:responses:compact",
            source_model: Some("client-model"),
            family: SameFormatProviderFamily::Standard,
            body_rules: Some(&body_rules),
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: false,
        })
        .expect("body should build");

        assert!(body.get("stream").is_none());
    }

    #[test]
    fn same_format_body_applies_model_directive_before_body_rules() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "model": "gpt-5.4-high",
                "messages": [{"role": "user", "content": "hello"}],
                "reasoning_effort": "low"
            }),
            mapped_model: "upstream-model",
            client_api_format: "openai:chat",
            provider_api_format: "openai:chat",
            source_model: Some("gpt-5.4-high"),
            family: SameFormatProviderFamily::Standard,
            body_rules: Some(&json!([
                {"action":"set","path":"metadata.body_rule_seen","value":true}
            ])),
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: true,
        })
        .expect("body should build");

        assert_eq!(body["model"], "upstream-model");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["metadata"]["body_rule_seen"], true);
    }

    #[test]
    fn search_body_projects_the_protocol_contract_and_keeps_fast_routing_only() {
        let body = build_same_format_provider_request_body(SameFormatProviderRequestBodyInput {
            body_json: &json!({
                "id": "session-1",
                "model": "gpt-5.6-luna-high-fast",
                "commands": {"search_query": [{"q": "Aether"}]},
                "settings": {"allowed_callers": ["direct"]},
                "store": false,
                "future_extension": {"enabled": true},
                "stream": true
            }),
            mapped_model: "gpt-5.6-luna",
            client_api_format: "openai:search",
            provider_api_format: "openai:search",
            source_model: Some("gpt-5.6-luna-high-fast"),
            family: SameFormatProviderFamily::Standard,
            body_rules: None,
            request_headers: None,
            upstream_is_stream: false,
            force_body_stream_field: false,
            kiro_auth_config: None,
            is_claude_code: false,
            enable_model_directives: true,
        })
        .expect("search body should build");

        assert_eq!(body["id"], "session-1");
        assert_eq!(body["model"], "gpt-5.6-luna");
        assert_eq!(body["commands"]["search_query"][0]["q"], "Aether");
        assert_eq!(body["settings"]["allowed_callers"][0], "direct");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(body.get("store").is_none());
        assert!(body.get("future_extension").is_none());
        assert!(body.get("service_tier").is_none());
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn builds_same_format_headers_with_auth_and_stream_accept() {
        let provider_request_body = json!({"model": "upstream-model"});
        let original_request_body = json!({"model": "client-model"});
        let mut request_headers = http::HeaderMap::new();
        request_headers.insert(
            http::header::ACCEPT_ENCODING,
            http::HeaderValue::from_static("gzip, br"),
        );
        let headers = build_same_format_provider_headers(SameFormatProviderHeadersInput {
            headers: &request_headers,
            provider_request_body: &provider_request_body,
            original_request_body: &original_request_body,
            header_rules: None,
            behavior: SameFormatProviderRequestBehavior {
                is_antigravity: false,
                is_gemini_cli: false,
                is_claude_code: false,
                is_claude_code_transport: false,
                anthropic_compatibility_profile: AnthropicCompatibilityProfile::NativeTransparent,
                is_vertex: false,
                is_kiro: false,
                upstream_is_stream: true,
                force_body_stream_field: false,
                report_kind: "openai_chat_stream_success",
            },
            api_operation: None,
            auth_header: Some("x-api-key"),
            auth_value: Some("secret"),
            extra_headers: &BTreeMap::new(),
            kiro_auth_config: None,
            kiro_machine_id: None,
        })
        .expect("headers should build");

        assert_eq!(headers.get("x-api-key").map(String::as_str), Some("secret"));
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("text/event-stream")
        );
        assert_eq!(
            headers.get("accept-encoding").map(String::as_str),
            Some("identity")
        );
    }

    #[test]
    fn same_format_headers_cannot_restore_credentials_or_internal_headers() {
        let provider_request_body = json!({"model": "upstream-model"});
        let original_request_body = json!({"model": "client-model"});
        let mut request_headers = http::HeaderMap::new();
        for (name, value) in [
            ("authorization", "Bearer client"),
            ("api-key", "client-api-key"),
            ("x-api-key", "client-x-api-key"),
            ("cookie", "session=client"),
            ("proxy-authorization", "Basic client"),
            ("x-aether-auth-user-id", "user-private"),
            ("x-aether-auth-api-key-id", "key-private"),
            ("x-aether-auth-balance-remaining", "12.34"),
            ("x-aether-gateway", "gateway-internal"),
        ] {
            request_headers.insert(
                http::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                http::HeaderValue::from_str(value).expect("valid header value"),
            );
        }
        let behavior = SameFormatProviderRequestBehavior {
            is_antigravity: false,
            is_gemini_cli: false,
            is_claude_code: false,
            is_claude_code_transport: false,
            anthropic_compatibility_profile: AnthropicCompatibilityProfile::NativeTransparent,
            is_vertex: false,
            is_kiro: false,
            upstream_is_stream: false,
            force_body_stream_field: false,
            report_kind: "claude_chat_sync_success",
        };
        let header_rules = json!([
            {"action": "set", "key": "cookie", "value": "session=rule"},
            {"action": "set", "key": "authorization", "value": "Bearer rule"},
            {"action": "set", "key": "x-aether-auth-user-id", "value": "user-rule"}
        ]);
        let extra_headers = BTreeMap::from([
            ("api-key".to_string(), "extra-api-key".to_string()),
            ("authorization".to_string(), "Bearer extra".to_string()),
            (
                "x-aether-auth-balance-remaining".to_string(),
                "99.99".to_string(),
            ),
            ("anthropic-beta".to_string(), "custom-beta".to_string()),
        ]);

        let headers = build_same_format_provider_headers(SameFormatProviderHeadersInput {
            headers: &request_headers,
            provider_request_body: &provider_request_body,
            original_request_body: &original_request_body,
            header_rules: Some(&header_rules),
            behavior,
            api_operation: None,
            auth_header: Some("x-api-key"),
            auth_value: Some("upstream-secret"),
            extra_headers: &extra_headers,
            kiro_auth_config: None,
            kiro_machine_id: None,
        })
        .expect("headers should build");

        assert_eq!(
            headers.get("x-api-key").map(String::as_str),
            Some("upstream-secret")
        );
        for stripped in ["authorization", "api-key", "cookie", "proxy-authorization"] {
            assert!(!headers.contains_key(stripped), "should strip {stripped}");
        }
        assert!(
            headers.keys().all(|name| !name.starts_with("x-aether-")),
            "Aether-owned headers must never leave provider egress: {headers:?}"
        );
        assert_eq!(
            headers.get("anthropic-beta").map(String::as_str),
            Some("custom-beta")
        );
    }
}
