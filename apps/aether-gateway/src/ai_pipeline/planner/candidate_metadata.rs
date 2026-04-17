use aether_contracts::ProxySnapshot;
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::{json, Map, Value};

use crate::ai_pipeline::planner::candidate_eligibility::EligibleLocalExecutionCandidate;
use crate::ai_pipeline::transport::{
    body_rules_are_locally_supported, header_rules_are_locally_supported,
    local_gemini_transport_unsupported_reason_with_network,
    local_openai_chat_transport_unsupported_reason,
    local_standard_transport_unsupported_reason_with_network, resolve_transport_tls_profile,
    supports_local_oauth_request_auth_resolution, transport_proxy_is_locally_supported,
};
use crate::ai_pipeline::{
    request_conversion_enabled_for_transport, request_conversion_kind,
    request_conversion_requires_enable_flag, request_conversion_transport_unsupported_reason,
    request_pair_allowed_for_transport, GatewayProviderTransportSnapshot,
};
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy};
use crate::append_execution_contract_fields_to_value;

pub(crate) struct LocalExecutionCandidateMetadataParts<'a> {
    pub(crate) eligible: &'a EligibleLocalExecutionCandidate,
    pub(crate) provider_api_format: &'a str,
    pub(crate) client_api_format: &'a str,
    pub(crate) extra_fields: Map<String, Value>,
}

pub(crate) fn build_request_trace_proxy_value(
    transport: Option<&GatewayProviderTransportSnapshot>,
    resolved_proxy: Option<&ProxySnapshot>,
) -> Option<Value> {
    let resolved_proxy = resolved_proxy?;
    let mut object = Map::new();

    if let Some(node_id) = trimmed_non_empty(resolved_proxy.node_id.as_deref()) {
        object.insert("node_id".to_string(), Value::String(node_id));
    }
    if let Some(node_name) = trimmed_non_empty(resolved_proxy.label.as_deref()) {
        object.insert("node_name".to_string(), Value::String(node_name));
    }
    if let Some(url) = sanitize_trace_proxy_url(resolved_proxy.url.as_deref()) {
        object.insert("url".to_string(), Value::String(url));
    }
    if let Some(source) = resolve_request_trace_proxy_source(transport, true) {
        object.insert("source".to_string(), Value::String(source.to_string()));
    }

    (!object.is_empty()).then_some(Value::Object(object))
}

pub(crate) fn build_local_execution_candidate_metadata(
    parts: LocalExecutionCandidateMetadataParts<'_>,
) -> Value {
    build_local_execution_candidate_metadata_for_candidate(
        &parts.eligible.candidate,
        Some(&parts.eligible.transport),
        parts.provider_api_format,
        parts.client_api_format,
        parts.extra_fields,
    )
}

pub(crate) fn build_local_execution_candidate_metadata_for_candidate(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    transport: Option<&GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    client_api_format: &str,
    extra_fields: Map<String, Value>,
) -> Value {
    let mut object = Map::new();
    object.insert(
        "provider_api_format".to_string(),
        Value::String(provider_api_format.to_string()),
    );
    object.insert(
        "client_api_format".to_string(),
        Value::String(client_api_format.to_string()),
    );
    object.insert(
        "global_model_id".to_string(),
        Value::String(candidate.global_model_id.clone()),
    );
    object.insert(
        "global_model_name".to_string(),
        Value::String(candidate.global_model_name.clone()),
    );
    object.insert(
        "model_id".to_string(),
        Value::String(candidate.model_id.clone()),
    );
    object.insert(
        "selected_provider_model_name".to_string(),
        Value::String(candidate.selected_provider_model_name.clone()),
    );
    object.insert(
        "mapping_matched_model".to_string(),
        candidate
            .mapping_matched_model
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    object.insert(
        "provider_name".to_string(),
        Value::String(candidate.provider_name.clone()),
    );
    object.insert(
        "key_name".to_string(),
        Value::String(candidate.key_name.clone()),
    );
    object.extend(extra_fields);
    append_transport_diagnostics_to_value(
        Value::Object(object),
        transport,
        client_api_format,
        provider_api_format,
    )
}

pub(crate) fn build_local_execution_candidate_contract_metadata(
    parts: LocalExecutionCandidateMetadataParts<'_>,
    execution_strategy: ExecutionStrategy,
    conversion_mode: ConversionMode,
    provider_contract: &str,
) -> Value {
    append_execution_contract_fields_to_value(
        build_local_execution_candidate_metadata_for_candidate(
            &parts.eligible.candidate,
            Some(&parts.eligible.transport),
            parts.provider_api_format,
            parts.client_api_format,
            parts.extra_fields,
        ),
        execution_strategy,
        conversion_mode,
        parts.client_api_format,
        provider_contract,
    )
}

pub(crate) fn build_local_execution_candidate_contract_metadata_for_candidate(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    transport: Option<&GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    client_api_format: &str,
    extra_fields: Map<String, Value>,
    execution_strategy: ExecutionStrategy,
    conversion_mode: ConversionMode,
    provider_contract: &str,
) -> Value {
    append_execution_contract_fields_to_value(
        build_local_execution_candidate_metadata_for_candidate(
            candidate,
            transport,
            provider_api_format,
            client_api_format,
            extra_fields,
        ),
        execution_strategy,
        conversion_mode,
        client_api_format,
        provider_contract,
    )
}

fn append_transport_diagnostics_to_value(
    value: Value,
    transport: Option<&GatewayProviderTransportSnapshot>,
    client_api_format: &str,
    provider_api_format: &str,
) -> Value {
    let Value::Object(mut object) = value else {
        return value;
    };
    object.insert(
        "transport_diagnostics".to_string(),
        transport
            .map(|transport| {
                build_transport_diagnostics(transport, client_api_format, provider_api_format)
            })
            .unwrap_or_else(|| json!({ "transport_snapshot_available": false })),
    );
    Value::Object(object)
}

fn build_transport_diagnostics(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> Value {
    let resolved_tls_profile = resolve_transport_tls_profile(transport);
    let configured_tls_profile = transport
        .key
        .fingerprint
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|value| value.get("tls_profile"))
        .cloned()
        .unwrap_or(Value::Null);
    let has_oauth_config = transport.key.decrypted_auth_config.is_some();
    let oauth_resolution_supported =
        !has_oauth_config || supports_local_oauth_request_auth_resolution(transport);
    let request_transport_unsupported_reason = resolve_request_transport_unsupported_reason(
        transport,
        client_api_format,
        provider_api_format,
    );

    json!({
        "transport_snapshot_available": true,
        "provider_type": transport.provider.provider_type,
        "provider_is_active": transport.provider.is_active,
        "endpoint_is_active": transport.endpoint.is_active,
        "key_is_active": transport.key.is_active,
        "provider_enable_format_conversion": transport.provider.enable_format_conversion,
        "provider_keep_priority_on_conversion": transport.provider.keep_priority_on_conversion,
        "endpoint_format_acceptance_config": transport.endpoint.format_acceptance_config,
        "endpoint_custom_path": transport.endpoint.custom_path,
        "header_rules": transport.endpoint.header_rules,
        "header_rules_supported": header_rules_are_locally_supported(transport.endpoint.header_rules.as_ref()),
        "body_rules": transport.endpoint.body_rules,
        "body_rules_supported": body_rules_are_locally_supported(transport.endpoint.body_rules.as_ref()),
        "proxy": {
            "locally_supported": transport_proxy_is_locally_supported(transport),
            "provider": summarize_proxy_config(transport.provider.proxy.as_ref()),
            "endpoint": summarize_proxy_config(transport.endpoint.proxy.as_ref()),
            "key": summarize_proxy_config(transport.key.proxy.as_ref()),
        },
        "auth": {
            "key_auth_type": transport.key.auth_type,
            "has_oauth_config": has_oauth_config,
            "oauth_request_auth_resolution_supported": oauth_resolution_supported,
        },
        "fingerprint": transport.key.fingerprint,
        "configured_tls_profile": configured_tls_profile,
        "resolved_tls_profile": resolved_tls_profile,
        "request_pair": {
            "client_api_format": client_api_format,
            "provider_api_format": provider_api_format,
            "requires_conversion_enable_flag": request_conversion_requires_enable_flag(
                client_api_format,
                provider_api_format,
            ),
            "conversion_enabled": request_conversion_enabled_for_transport(
                transport,
                client_api_format,
                provider_api_format,
            ),
            "pair_allowed": request_pair_allowed_for_transport(
                transport,
                client_api_format,
                provider_api_format,
            ),
            "transport_unsupported_reason": request_transport_unsupported_reason,
        },
    })
}

fn summarize_proxy_config(proxy: Option<&Value>) -> Value {
    let Some(object) = proxy.and_then(Value::as_object) else {
        return Value::Null;
    };
    let has_url = object
        .get("url")
        .or_else(|| object.get("proxy_url"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    json!({
        "enabled": object.get("enabled").cloned().unwrap_or(Value::Null),
        "mode": object.get("mode").cloned().unwrap_or(Value::Null),
        "node_id": object.get("node_id").cloned().unwrap_or(Value::Null),
        "label": object.get("label").cloned().unwrap_or(Value::Null),
        "has_url": has_url,
    })
}

fn resolve_request_trace_proxy_source(
    transport: Option<&GatewayProviderTransportSnapshot>,
    has_resolved_proxy: bool,
) -> Option<&'static str> {
    let transport = transport?;
    if transport_has_explicit_proxy(transport.key.proxy.as_ref()) {
        return Some("key");
    }
    if transport_has_explicit_proxy(transport.endpoint.proxy.as_ref()) {
        return Some("endpoint");
    }
    if transport_has_explicit_proxy(transport.provider.proxy.as_ref()) {
        return Some("provider");
    }
    has_resolved_proxy.then_some("system")
}

fn transport_has_explicit_proxy(proxy: Option<&Value>) -> bool {
    let Some(object) = proxy.and_then(Value::as_object) else {
        return false;
    };
    let enabled = object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !enabled {
        return false;
    }

    object
        .get("node_id")
        .or_else(|| object.get("url"))
        .or_else(|| object.get("proxy_url"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

fn sanitize_trace_proxy_url(url: Option<&str>) -> Option<String> {
    let raw = url.map(str::trim).filter(|value| !value.is_empty())?;
    let parsed = url::Url::parse(raw).ok()?;
    let scheme = parsed.scheme().trim();
    let host = parsed.host_str()?.trim();
    if scheme.is_empty() || host.is_empty() {
        return None;
    }

    let mut safe = format!("{scheme}://{host}");
    if let Some(port) = parsed.port() {
        safe.push(':');
        safe.push_str(port.to_string().as_str());
    }
    Some(safe)
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_request_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<&'static str> {
    let client_api_format = client_api_format.trim().to_ascii_lowercase();
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    if client_api_format == provider_api_format {
        return match provider_api_format.as_str() {
            "openai:chat" => local_openai_chat_transport_unsupported_reason(transport),
            "gemini:chat" | "gemini:cli" => local_gemini_transport_unsupported_reason_with_network(
                transport,
                provider_api_format.as_str(),
            ),
            _ => local_standard_transport_unsupported_reason_with_network(
                transport,
                provider_api_format.as_str(),
            ),
        };
    }
    match request_conversion_kind(client_api_format.as_str(), provider_api_format.as_str()) {
        Some(kind) => request_conversion_transport_unsupported_reason(transport, kind),
        None => Some("transport_api_format_unsupported"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_local_execution_candidate_contract_metadata_for_candidate,
        build_local_execution_candidate_metadata_for_candidate,
    };
    use crate::ai_pipeline::transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use crate::ai_pipeline::{ConversionMode, ExecutionStrategy, GatewayProviderTransportSnapshot};
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
    use serde_json::{json, Value};

    fn sample_candidate() -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "RightCode".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 22,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:cli".to_string(),
            key_id: "key-1".to_string(),
            key_name: "codex".to_string(),
            key_auth_type: "oauth".to_string(),
            key_internal_priority: 10,
            key_global_priority_for_format: None,
            key_capabilities: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-1".to_string(),
            global_model_name: "gpt-5.4".to_string(),
            selected_provider_model_name: "gpt-5.4".to_string(),
            mapping_matched_model: None,
        }
    }

    fn sample_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "RightCode".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: Some(json!({"enabled": true, "mode": "node", "node_id": "proxy-node-1"})),
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:cli".to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1/responses".to_string()),
                config: None,
                format_acceptance_config: Some(json!({
                    "enabled": true,
                    "accept_formats": ["claude:cli"]
                })),
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "codex".to_string(),
                auth_type: "oauth".to_string(),
                is_active: true,
                api_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: Some(json!({
                    "tls_profile": "chrome_136",
                    "user_agent": "Mozilla/5.0"
                })),
                decrypted_api_key: "sk-test".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn candidate_contract_metadata_includes_transport_diagnostics() {
        let metadata = build_local_execution_candidate_contract_metadata_for_candidate(
            &sample_candidate(),
            Some(&sample_transport()),
            "openai:cli",
            "claude:cli",
            serde_json::Map::new(),
            ExecutionStrategy::LocalCrossFormat,
            ConversionMode::Bidirectional,
            "openai:cli",
        );

        assert_eq!(metadata["transport_diagnostics"]["provider_type"], "codex");
        assert_eq!(
            metadata["transport_diagnostics"]["fingerprint"]["tls_profile"],
            "chrome_136"
        );
        assert_eq!(
            metadata["transport_diagnostics"]["resolved_tls_profile"],
            "chrome_136"
        );
        assert_eq!(
            metadata["transport_diagnostics"]["request_pair"]["conversion_enabled"],
            Value::Bool(true)
        );
        assert!(
            metadata["transport_diagnostics"]["request_pair"]["transport_unsupported_reason"]
                .is_null()
        );
    }

    #[test]
    fn candidate_metadata_marks_missing_transport_snapshot() {
        let metadata = build_local_execution_candidate_metadata_for_candidate(
            &sample_candidate(),
            None,
            "openai:cli",
            "openai:cli",
            serde_json::Map::new(),
        );

        assert_eq!(
            metadata["transport_diagnostics"]["transport_snapshot_available"],
            Value::Bool(false)
        );
    }
}
