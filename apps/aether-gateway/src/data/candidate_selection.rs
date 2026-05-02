use aether_data::DataLayerError;
use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use aether_scheduler_core::{
    auth_constraints_allow_api_format, collect_global_model_names_for_required_capability,
    enumerate_minimal_candidate_selection, normalize_api_format,
    resolve_requested_global_model_name, row_supports_requested_model,
    EnumerateMinimalCandidateSelectionInput, SchedulerAuthConstraints,
    SchedulerMinimalCandidateSelectionCandidate,
};
use async_trait::async_trait;
use std::collections::BTreeSet;

use super::auth::GatewayAuthApiKeySnapshot;

#[async_trait]
pub(crate) trait MinimalCandidateSelectionRowSource {
    async fn read_minimal_candidate_selection_rows_for_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>;

    async fn read_minimal_candidate_selection_rows_for_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>;
}

pub(crate) async fn read_requested_model_rows(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    requested_model_name: &str,
) -> Result<Option<(String, Vec<StoredMinimalCandidateSelectionRow>)>, DataLayerError> {
    let rows = state
        .read_minimal_candidate_selection_rows_for_api_format(api_format)
        .await?;
    let rows = rows
        .into_iter()
        .filter(|row| row_supports_requested_model(row, requested_model_name, api_format))
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(None);
    }

    let Some(resolved_global_model_name) =
        resolve_requested_global_model_name(&rows, requested_model_name, api_format)
    else {
        return Ok(None);
    };

    let resolved_rows = rows
        .into_iter()
        .filter(|row| row.global_model_name == resolved_global_model_name)
        .collect();

    Ok(Some((resolved_global_model_name, resolved_rows)))
}

pub(crate) async fn enumerate_minimal_candidate_selection_with_required_capabilities(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    requested_model_name: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&serde_json::Value>,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, DataLayerError> {
    let normalized_api_format = normalize_api_format(api_format);
    if normalized_api_format.is_empty() {
        return Ok(Vec::new());
    }

    if !auth_constraints_allow_api_format(
        auth_snapshot.map(auth_snapshot_constraints).as_ref(),
        &normalized_api_format,
    ) {
        return Ok(Vec::new());
    }

    let Some((resolved_global_model_name, rows)) =
        read_requested_model_rows(state, &normalized_api_format, requested_model_name).await?
    else {
        return Ok(Vec::new());
    };
    let auth_constraints = auth_snapshot.map(auth_snapshot_constraints);
    enumerate_minimal_candidate_selection(EnumerateMinimalCandidateSelectionInput {
        rows,
        normalized_api_format: &normalized_api_format,
        requested_model_name,
        resolved_global_model_name: resolved_global_model_name.as_str(),
        require_streaming,
        required_capabilities,
        auth_constraints: auth_constraints.as_ref(),
    })
}

pub(crate) async fn read_global_model_names_for_required_capability(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    required_capability: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
) -> Result<Vec<String>, DataLayerError> {
    let normalized_api_format = normalize_api_format(api_format);
    let required_capability = required_capability.trim();
    if normalized_api_format.is_empty() || required_capability.is_empty() {
        return Ok(Vec::new());
    }

    if !auth_constraints_allow_api_format(
        auth_snapshot.map(auth_snapshot_constraints).as_ref(),
        &normalized_api_format,
    ) {
        return Ok(Vec::new());
    }

    let rows = state
        .read_minimal_candidate_selection_rows_for_api_format(&normalized_api_format)
        .await?;
    let auth_constraints = auth_snapshot.map(auth_snapshot_constraints);
    Ok(collect_global_model_names_for_required_capability(
        rows,
        &normalized_api_format,
        required_capability,
        require_streaming,
        auth_constraints.as_ref(),
    ))
}

pub(crate) async fn read_global_model_names_for_api_format(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
) -> Result<Vec<String>, DataLayerError> {
    let normalized_api_format = normalize_api_format(api_format);
    if normalized_api_format.is_empty() {
        return Ok(Vec::new());
    }

    if !auth_constraints_allow_api_format(
        auth_snapshot.map(auth_snapshot_constraints).as_ref(),
        &normalized_api_format,
    ) {
        return Ok(Vec::new());
    }

    let rows = state
        .read_minimal_candidate_selection_rows_for_api_format(&normalized_api_format)
        .await?;
    let auth_constraints = auth_snapshot.map(auth_snapshot_constraints);
    let mut model_names = BTreeSet::new();

    for row in rows {
        if require_streaming && !row.supports_streaming() {
            continue;
        }
        if !aether_scheduler_core::auth_constraints_allow_provider(
            auth_constraints.as_ref(),
            &row.provider_id,
            &row.provider_name,
            &row.provider_type,
        ) {
            continue;
        }
        if !aether_scheduler_core::auth_constraints_allow_model(
            auth_constraints.as_ref(),
            &row.global_model_name,
            &row.global_model_name,
        ) {
            continue;
        }
        model_names.insert(row.global_model_name);
    }

    Ok(model_names.into_iter().collect())
}

fn auth_snapshot_constraints(snapshot: &GatewayAuthApiKeySnapshot) -> SchedulerAuthConstraints {
    SchedulerAuthConstraints {
        allowed_providers: snapshot
            .effective_allowed_providers()
            .map(|items| items.to_vec()),
        allowed_api_formats: snapshot
            .effective_allowed_api_formats()
            .map(|items| items.to_vec()),
        allowed_models: snapshot
            .effective_allowed_models()
            .map(|items| items.to_vec()),
    }
}
