use std::collections::BTreeSet;

use aether_scheduler_core::{
    enumerate_minimal_candidate_selection_with_model_directives, normalize_api_format,
    EnumerateMinimalCandidateSelectionInput, SchedulerMinimalCandidateSelectionCandidate,
};

use crate::ai_serving::{
    candidate_common_transport_skip_reason, candidate_transport_pair_skip_reason,
    request_candidate_api_formats, request_conversion_transport_unsupported_reason,
    CandidateTransportPolicyFacts, GatewayProviderTransportSnapshot,
};
use crate::{AppState, GatewayError};

const CODEX_ADAPTER_CLIENT_API_FORMAT: &str = "openai:responses";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAdapterGlobalModelCompatibility {
    pub(crate) global_model: String,
    pub(crate) compatible: bool,
    pub(crate) reasons: Vec<String>,
    pub(crate) summary: Option<String>,
}

pub(crate) async fn read_codex_adapter_global_model_compatibilities(
    state: &AppState,
    global_models: &[String],
) -> Result<Vec<CodexAdapterGlobalModelCompatibility>, GatewayError> {
    let mut normalized_names = Vec::new();
    let mut seen = BTreeSet::new();
    for raw_name in global_models {
        let name = raw_name.trim();
        if name.is_empty() || !seen.insert(name.to_string()) {
            continue;
        }
        normalized_names.push(name.to_string());
    }

    let mut items = Vec::with_capacity(normalized_names.len());
    for global_model in normalized_names {
        items.push(read_codex_adapter_global_model_compatibility(state, &global_model).await?);
    }
    Ok(items)
}

pub(crate) fn codex_adapter_compatibility_summary(
    compatibility: &CodexAdapterGlobalModelCompatibility,
) -> String {
    compatibility
        .summary
        .clone()
        .unwrap_or_else(|| "当前没有可用于 Responses 的候选链路".to_string())
}

async fn read_codex_adapter_global_model_compatibility(
    state: &AppState,
    global_model: &str,
) -> Result<CodexAdapterGlobalModelCompatibility, GatewayError> {
    let mut reasons = Vec::new();
    let mut saw_rows = false;
    let mut saw_candidates = false;

    for candidate_api_format in
        request_candidate_api_formats(CODEX_ADAPTER_CLIENT_API_FORMAT, false)
    {
        let Some(candidates) =
            enumerate_codex_adapter_candidates(state, global_model, candidate_api_format).await?
        else {
            continue;
        };
        saw_rows = true;
        if candidates.is_empty() {
            continue;
        }
        saw_candidates = true;

        for candidate in candidates {
            let Some(transport) = state
                .read_provider_transport_snapshot(
                    &candidate.provider_id,
                    &candidate.endpoint_id,
                    &candidate.key_id,
                )
                .await?
            else {
                push_reason(&mut reasons, "transport_snapshot_missing");
                continue;
            };

            if let Some(skip_reason) =
                candidate_common_skip_reason(&transport, &candidate, global_model)
            {
                push_reason(&mut reasons, skip_reason);
                continue;
            }
            if let Some(skip_reason) =
                candidate_transport_pair_skip_reason(&transport, CODEX_ADAPTER_CLIENT_API_FORMAT)
            {
                push_reason(&mut reasons, skip_reason);
                continue;
            }
            if let Some(skip_reason) = codex_adapter_transport_support_skip_reason(&transport) {
                push_reason(&mut reasons, skip_reason);
                continue;
            }

            return Ok(CodexAdapterGlobalModelCompatibility {
                global_model: global_model.to_string(),
                compatible: true,
                reasons: Vec::new(),
                summary: None,
            });
        }
    }

    if !saw_rows {
        push_reason(&mut reasons, "no_candidate_rows");
    } else if !saw_candidates {
        push_reason(&mut reasons, "no_selectable_candidates");
    }

    Ok(CodexAdapterGlobalModelCompatibility {
        global_model: global_model.to_string(),
        compatible: false,
        summary: compatibility_reason_summary(&reasons),
        reasons,
    })
}

async fn enumerate_codex_adapter_candidates(
    state: &AppState,
    global_model: &str,
    candidate_api_format: &str,
) -> Result<Option<Vec<SchedulerMinimalCandidateSelectionCandidate>>, GatewayError> {
    let rows = state
        .list_minimal_candidate_selection_rows_for_api_format_and_global_model(
            candidate_api_format,
            global_model,
        )
        .await?;
    if rows.is_empty() {
        return Ok(None);
    }

    let normalized_api_format = normalize_api_format(candidate_api_format);
    if normalized_api_format.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let enable_model_directives =
        crate::system_features::reasoning_model_directive_enabled_for_api_format_and_model(
            state,
            candidate_api_format,
            Some(global_model),
        )
        .await;

    enumerate_minimal_candidate_selection_with_model_directives(
        EnumerateMinimalCandidateSelectionInput {
            rows,
            normalized_api_format: normalized_api_format.as_str(),
            requested_model_name: global_model,
            resolved_global_model_name: global_model,
            require_streaming: false,
            required_capabilities: None,
            auth_constraints: None,
        },
        enable_model_directives,
    )
    .map(Some)
    .map_err(|error| GatewayError::Internal(error.to_string()))
}

fn candidate_common_skip_reason(
    transport: &GatewayProviderTransportSnapshot,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    global_model: &str,
) -> Option<&'static str> {
    candidate_common_transport_skip_reason(
        transport,
        CandidateTransportPolicyFacts {
            endpoint_api_format: candidate.endpoint_api_format.as_str(),
            global_model_name: candidate.global_model_name.as_str(),
            selected_provider_model_name: candidate.selected_provider_model_name.as_str(),
            mapping_matched_model: candidate.mapping_matched_model.as_deref(),
        },
        Some(global_model),
    )
}

fn codex_adapter_transport_support_skip_reason(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<&'static str> {
    let provider_api_format = transport.endpoint.api_format.trim();
    if crate::ai_serving::api_format_alias_matches(
        provider_api_format,
        CODEX_ADAPTER_CLIENT_API_FORMAT,
    ) {
        return crate::ai_serving::transport::local_standard_transport_unsupported_reason_with_network(
            transport,
            provider_api_format,
        );
    }

    let conversion_kind = crate::ai_serving::request_conversion_kind(
        CODEX_ADAPTER_CLIENT_API_FORMAT,
        provider_api_format,
    )?;
    request_conversion_transport_unsupported_reason(transport, conversion_kind)
}

fn push_reason(reasons: &mut Vec<String>, reason: &str) {
    if reasons.iter().any(|existing| existing == reason) {
        return;
    }
    reasons.push(reason.to_string());
}

fn compatibility_reason_summary(reasons: &[String]) -> Option<String> {
    if reasons.is_empty() {
        return None;
    }

    let labels = reasons
        .iter()
        .map(|reason| compatibility_reason_label(reason))
        .take(2)
        .collect::<Vec<_>>();
    Some(labels.join("、"))
}

fn compatibility_reason_label(reason: &str) -> &'static str {
    match reason {
        "no_candidate_rows" | "no_selectable_candidates" => "没有可用的模型映射",
        "transport_snapshot_missing" => "缺少可用传输快照",
        "provider_inactive" => "Provider 未启用",
        "endpoint_inactive" => "Endpoint 未启用",
        "key_inactive" => "Key 未启用",
        "endpoint_api_format_changed" => "Endpoint 格式已变化",
        "key_api_format_disabled" => "Key 未开放该格式",
        "key_model_disabled" => "Key 未开放该模型",
        "format_conversion_disabled" => "未开启格式转换",
        "transport_api_format_unsupported" => "传输不支持该格式",
        "transport_header_rules_unsupported" => "Header 规则不受支持",
        "transport_body_rules_unsupported" => "Body 规则不受支持",
        "transport_oauth_resolution_unsupported" => "认证解析方式不受支持",
        "transport_custom_path_unsupported" => "自定义路径不受支持",
        "transport_proxy_unsupported" => "代理配置不受支持",
        "transport_proxy_or_profile_unsupported" => "代理或传输配置不受支持",
        "transport_profile_unsupported" => "传输配置不受支持",
        "transport_provider_type_unsupported" => "Provider 类型不受支持",
        "transport_endpoint_kind_unsupported" => "Endpoint 类型不受支持",
        _ => "当前不可用于 Responses",
    }
}
