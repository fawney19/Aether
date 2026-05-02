use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_scheduler_core::matches_model_mapping;

use super::GatewayPublicRequestContext;

pub(crate) fn models_api_format(request_context: &GatewayPublicRequestContext) -> Option<&str> {
    let signature = request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.auth_endpoint_signature.as_deref())
        .map(str::trim)
        .filter(|signature| !signature.is_empty())?;
    match crate::ai_serving::normalize_api_format_alias(signature).as_str() {
        "openai:chat" => Some("openai:chat"),
        "openai:responses" => Some("openai:responses"),
        "openai:responses:compact" => Some("openai:responses:compact"),
        "openai:image" => Some("openai:image"),
        "claude:messages" => Some("claude:messages"),
        "gemini:generate_content" => Some("gemini:generate_content"),
        _ => None,
    }
}

const MODELS_CROSS_FORMAT_QUERY_API_FORMATS: &[&str] = &[
    "openai:chat",
    "openai:responses",
    "openai:responses:compact",
    "openai:image",
    "claude:messages",
    "gemini:generate_content",
];

pub(super) fn models_query_api_formats(api_format: &str) -> &'static [&'static str] {
    match crate::ai_serving::normalize_api_format_alias(api_format).as_str() {
        "openai:chat"
        | "openai:responses"
        | "openai:responses:compact"
        | "claude:messages"
        | "gemini:generate_content" => MODELS_CROSS_FORMAT_QUERY_API_FORMATS,
        "openai:image" => &["openai:image"],
        _ => &[],
    }
}

pub(super) fn models_detail_id(request_path: &str) -> Option<String> {
    let raw = if let Some(value) = request_path.strip_prefix("/v1/models/") {
        value
    } else if let Some(value) = request_path.strip_prefix("/v1beta/models/") {
        value
    } else {
        return None;
    };
    let normalized = raw.trim().trim_start_matches("models/").trim();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn auth_snapshot_allows_provider_for_models(
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
    provider_id: &str,
    provider_name: &str,
    provider_type: &str,
) -> bool {
    let Some(allowed) = auth_snapshot
        .and_then(crate::data::auth::GatewayAuthApiKeySnapshot::effective_allowed_providers)
    else {
        return true;
    };

    allowed.iter().any(|value| {
        aether_scheduler_core::provider_matches_allowed_value(
            value,
            provider_id,
            provider_name,
            provider_type,
        )
    })
}

fn auth_snapshot_allows_model_for_models(
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
    global_model_name: &str,
) -> bool {
    let Some(allowed) = auth_snapshot
        .and_then(crate::data::auth::GatewayAuthApiKeySnapshot::effective_allowed_models)
    else {
        return true;
    };
    allowed.iter().any(|value| value == global_model_name)
}

fn mapping_scope_matches_for_models(
    mapping: &StoredProviderModelMapping,
    api_format: &str,
) -> bool {
    let Some(api_formats) = mapping.api_formats.as_ref() else {
        return true;
    };
    api_formats
        .iter()
        .any(|value| value.trim().eq_ignore_ascii_case(api_format))
}

fn candidate_model_names_for_models(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::from([row.model_provider_model_name.clone()]);
    if let Some(mappings) = row.model_provider_model_mappings.as_ref() {
        for mapping in mappings {
            if mapping_scope_matches_for_models(mapping, api_format) {
                names.insert(mapping.name.clone());
            }
        }
    }
    names
}

pub(crate) fn matches_model_mapping_for_models(pattern: &str, model_name: &str) -> bool {
    matches_model_mapping(pattern, model_name)
}

fn row_exposes_global_model_for_models(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> bool {
    let Some(key_allowed_models) = row.key_allowed_models.as_ref() else {
        return true;
    };
    if key_allowed_models.is_empty() {
        return false;
    }
    if key_allowed_models
        .iter()
        .any(|value| value == &row.global_model_name)
    {
        return true;
    }

    let candidate_models = candidate_model_names_for_models(row, api_format);
    for allowed_model in key_allowed_models {
        if candidate_models.contains(allowed_model) {
            return true;
        }
    }

    let Some(global_model_mappings) = row.global_model_mappings.as_ref() else {
        return false;
    };
    for allowed_model in key_allowed_models {
        for pattern in global_model_mappings {
            if matches_model_mapping_for_models(pattern, allowed_model) {
                return true;
            }
        }
    }

    false
}

pub(super) fn filter_rows_for_models(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
    api_format: &str,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    let mut filtered = rows
        .into_iter()
        .filter(|row| {
            auth_snapshot_allows_provider_for_models(
                auth_snapshot,
                &row.provider_id,
                &row.provider_name,
                &row.provider_type,
            )
        })
        .filter(|row| auth_snapshot_allows_model_for_models(auth_snapshot, &row.global_model_name))
        .filter(|row| row_exposes_global_model_for_models(row, api_format))
        .collect::<Vec<_>>();
    filtered.sort_by(|left, right| left.global_model_name.cmp(&right.global_model_name));
    let mut deduped = Vec::new();
    let mut last_model_name: Option<String> = None;
    for row in filtered {
        if last_model_name.as_deref() == Some(row.global_model_name.as_str()) {
            continue;
        }
        last_model_name = Some(row.global_model_name.clone());
        deduped.push(row);
    }
    deduped
}
