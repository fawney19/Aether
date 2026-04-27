use std::collections::BTreeSet;

use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use aether_data_contracts::DataLayerError;

use super::types::{
    EnumerateMinimalCandidateSelectionInput, SchedulerMinimalCandidateSelectionCandidate,
};

pub fn enumerate_minimal_candidate_selection(
    input: EnumerateMinimalCandidateSelectionInput<'_>,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, DataLayerError> {
    let EnumerateMinimalCandidateSelectionInput {
        rows,
        normalized_api_format,
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
    if !crate::auth_constraints_allow_model(
        auth_constraints,
        requested_model_name,
        resolved_global_model_name,
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
        if require_streaming && !row.supports_streaming() {
            continue;
        }
        let Some((selected_provider_model_name, mapping_matched_model)) =
            crate::resolve_provider_model_name(&row, requested_model_name, normalized_api_format)
        else {
            continue;
        };

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
