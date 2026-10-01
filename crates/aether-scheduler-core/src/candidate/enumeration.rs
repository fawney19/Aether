use std::collections::BTreeSet;

use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use aether_data_contracts::DataLayerError;

use super::types::{
    EnumerateMinimalCandidateSelectionInput, SchedulerMinimalCandidateSelectionCandidate,
};

pub fn enumerate_minimal_candidate_selection(
    input: EnumerateMinimalCandidateSelectionInput<'_>,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, DataLayerError> {
    enumerate_minimal_candidate_selection_inner(input, false)
}

pub fn enumerate_minimal_candidate_selection_with_model_directives(
    input: EnumerateMinimalCandidateSelectionInput<'_>,
    enable_model_directives: bool,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, DataLayerError> {
    enumerate_minimal_candidate_selection_inner(input, enable_model_directives)
}

fn enumerate_minimal_candidate_selection_inner(
    input: EnumerateMinimalCandidateSelectionInput<'_>,
    enable_model_directives: bool,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, DataLayerError> {
    let EnumerateMinimalCandidateSelectionInput {
        rows,
        normalized_api_format,
        request_operation,
        requested_model_name,
        resolved_global_model_name,
        require_streaming,
        auth_constraints,
        ..
    } = input;

    if normalized_api_format.is_empty() {
        return Ok(Vec::new());
    }
    if !crate::auth_constraints_allow_api_format(auth_constraints, normalized_api_format) {
        return Ok(Vec::new());
    }
    if !crate::auth_constraints_allow_model_with_model_directives(
        auth_constraints,
        requested_model_name,
        resolved_global_model_name,
        enable_model_directives,
    ) {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::with_capacity(rows.len());
    for row in rows {
        if !crate::auth_constraints_allow_provider(
            auth_constraints,
            &row.provider_id,
            &row.provider_name,
            &row.provider_type,
        ) {
            continue;
        }
        if !crate::auth_constraints_allow_provider_key(auth_constraints, &row.key_id, &row.key_name)
        {
            continue;
        }
        if require_streaming && !row.supports_streaming() {
            continue;
        }
        let Some((selected_provider_model_name, mapping_matched_model)) =
            crate::resolve_provider_model_name_with_model_directives_and_request_operation(
                &row,
                requested_model_name,
                normalized_api_format,
                enable_model_directives,
                request_operation,
            )
        else {
            continue;
        };

        let supports_streaming = row.supports_streaming();
        candidates.push(SchedulerMinimalCandidateSelectionCandidate {
            provider_id: row.provider_id,
            provider_name: row.provider_name,
            provider_type: row.provider_type,
            provider_priority: row.provider_priority,
            endpoint_id: row.endpoint_id,
            endpoint_api_format: row.endpoint_api_format,
            key_id: row.key_id,
            key_name: row.key_name,
            key_auth_type: row.key_auth_type,
            key_internal_priority: row.key_internal_priority,
            key_global_priority_for_format: crate::extract_global_priority_for_format(
                row.key_global_priority_by_format.as_ref(),
                normalized_api_format,
            )?,
            key_capabilities: row.key_capabilities,
            model_id: row.model_id,
            global_model_id: row.global_model_id,
            global_model_name: row.global_model_name,
            selected_provider_model_name,
            supports_streaming,
            mapping_matched_model,
        });
    }

    Ok(candidates)
}

pub fn collect_global_model_names_for_required_capability(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    normalized_api_format: &str,
    required_capability: &str,
    require_streaming: bool,
    auth_constraints: Option<&crate::SchedulerAuthConstraints>,
) -> Vec<String> {
    if normalized_api_format.is_empty() || required_capability.trim().is_empty() {
        return Vec::new();
    }
    if !crate::auth_constraints_allow_api_format(auth_constraints, normalized_api_format) {
        return Vec::new();
    }

    let mut model_names = BTreeSet::new();
    for row in rows {
        if !crate::auth_constraints_allow_provider(
            auth_constraints,
            &row.provider_id,
            &row.provider_name,
            &row.provider_type,
        ) {
            continue;
        }
        if !crate::auth_constraints_allow_provider_key(auth_constraints, &row.key_id, &row.key_name)
        {
            continue;
        }
        if !crate::row_supports_required_capability(&row, required_capability) {
            continue;
        }
        if require_streaming && !row.supports_streaming() {
            continue;
        }
        if !crate::auth_constraints_allow_model(
            auth_constraints,
            &row.global_model_name,
            &row.global_model_name,
        ) {
            continue;
        }
        model_names.insert(row.global_model_name);
    }

    model_names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SchedulerAuthConstraints;

    fn sample_row(key_id: &str, key_name: &str) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-1".to_string(),
            provider_name: "provider".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: key_name.to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 10,
            key_global_priority_by_format: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5".to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    #[test]
    fn enumeration_filters_disallowed_provider_keys() {
        let constraints = SchedulerAuthConstraints {
            allowed_provider_keys: Some(vec!["key-allowed".to_string()]),
            ..Default::default()
        };

        let candidates =
            enumerate_minimal_candidate_selection(EnumerateMinimalCandidateSelectionInput {
                rows: vec![
                    sample_row("key-denied", "denied"),
                    sample_row("key-allowed", "allowed"),
                ],
                normalized_api_format: "openai:chat",
                request_operation: None,
                requested_model_name: "gpt-5",
                resolved_global_model_name: "gpt-5",
                require_streaming: false,
                required_capabilities: None,
                auth_constraints: Some(&constraints),
            })
            .expect("candidate enumeration should succeed");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].key_id, "key-allowed");
    }
}
