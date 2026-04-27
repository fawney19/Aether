use self::selection::{
    collect_selectable_candidates, collect_selectable_candidates_with_skip_reasons,
};
use super::state::SchedulerRuntimeState;

mod affinity;
mod enumeration;
mod ranking;
mod resolution;
mod runtime;
mod selection;

#[cfg(test)]
mod tests;

use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot;
use aether_scheduler_core::{
    candidate_model_names, candidate_supports_required_capability, matches_model_mapping,
    normalize_api_format, resolve_provider_model_name, select_provider_model_name,
    SchedulerMinimalCandidateSelectionCandidate,
};
use aether_wallet::{ProviderBillingType, ProviderQuotaSnapshot};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) use self::selection::SchedulerSkippedCandidate;

use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::data::candidate_selection::{
    read_global_model_names_for_api_format, read_global_model_names_for_required_capability,
    MinimalCandidateSelectionRowSource,
};
use crate::GatewayError;

#[cfg_attr(not(test), allow(dead_code))]
const SCHEDULER_AFFINITY_MAX_ENTRIES: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequiredCapabilityMatchMode {
    Compatible,
    Exclusive,
}

pub(crate) async fn list_selectable_candidates(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &impl SchedulerRuntimeState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    collect_selectable_candidates(
        selection_row_source,
        runtime_state,
        api_format,
        global_model_name,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        now_unix_secs,
    )
    .await
}

pub(crate) fn is_exact_all_skipped_by_auth_limit(
    selected: &[SchedulerMinimalCandidateSelectionCandidate],
    skipped: &[SchedulerSkippedCandidate],
) -> bool {
    selection::is_exact_all_skipped_by_auth_limit(selected, skipped)
}

pub(crate) async fn list_selectable_candidates_with_skip_reasons(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &impl SchedulerRuntimeState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<
    (
        Vec<SchedulerMinimalCandidateSelectionCandidate>,
        Vec<SchedulerSkippedCandidate>,
    ),
    GatewayError,
> {
    collect_selectable_candidates_with_skip_reasons(
        selection_row_source,
        runtime_state,
        api_format,
        global_model_name,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        now_unix_secs,
    )
    .await
}

pub(crate) async fn list_selectable_candidates_for_required_capability_without_requested_model(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &impl SchedulerRuntimeState,
    candidate_api_format: &str,
    required_capability: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    Ok(
        list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal(
            selection_row_source,
            runtime_state,
            candidate_api_format,
            required_capability,
            require_streaming,
            auth_snapshot,
            now_unix_secs,
        )
        .await?
        .0,
    )
}

pub(crate) async fn list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &impl SchedulerRuntimeState,
    candidate_api_format: &str,
    required_capability: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<(Vec<SchedulerMinimalCandidateSelectionCandidate>, bool), GatewayError> {
    let normalized_api_format = normalize_api_format(candidate_api_format);
    if normalized_api_format.is_empty() {
        return Ok((Vec::new(), false));
    }

    let capability_mode = required_capability_match_mode(required_capability);
    let model_names = match capability_mode {
        RequiredCapabilityMatchMode::Exclusive => {
            read_global_model_names_for_required_capability(
                selection_row_source,
                &normalized_api_format,
                required_capability,
                require_streaming,
                auth_snapshot,
            )
            .await
        }
        RequiredCapabilityMatchMode::Compatible => {
            read_global_model_names_for_api_format(
                selection_row_source,
                &normalized_api_format,
                require_streaming,
                auth_snapshot,
            )
            .await
        }
    }
    .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let required_capabilities = build_required_capabilities_object(required_capability);
    let mut all_attempts_blocked_by_auth_limit = !model_names.is_empty();

    for global_model_name in model_names {
        let (candidates, skipped_candidates) = collect_selectable_candidates_with_skip_reasons(
            selection_row_source,
            runtime_state,
            &normalized_api_format,
            &global_model_name,
            require_streaming,
            required_capabilities.as_ref(),
            auth_snapshot,
            now_unix_secs,
        )
        .await?;
        all_attempts_blocked_by_auth_limit &=
            is_exact_all_skipped_by_auth_limit(&candidates, &skipped_candidates);
        match capability_mode {
            RequiredCapabilityMatchMode::Exclusive => {
                let filtered = candidates
                    .into_iter()
                    .filter(|candidate| {
                        candidate_supports_required_capability(candidate, required_capability)
                    })
                    .collect::<Vec<_>>();
                if !filtered.is_empty() {
                    return Ok((filtered, false));
                }
            }
            RequiredCapabilityMatchMode::Compatible => {
                if candidates.is_empty() {
                    continue;
                }
                return Ok((candidates, false));
            }
        }
    }

    Ok((Vec::new(), all_attempts_blocked_by_auth_limit))
}

fn required_capability_match_mode(required_capability: &str) -> RequiredCapabilityMatchMode {
    match required_capability.trim().to_ascii_lowercase().as_str() {
        "cache_1h" | "context_1m" => RequiredCapabilityMatchMode::Compatible,
        _ => RequiredCapabilityMatchMode::Exclusive,
    }
}

fn build_required_capabilities_object(required_capability: &str) -> Option<serde_json::Value> {
    let required_capability = required_capability.trim();
    if required_capability.is_empty() {
        return None;
    }

    let mut capabilities = serde_json::Map::new();
    capabilities.insert(
        required_capability.to_string(),
        serde_json::Value::Bool(true),
    );
    Some(serde_json::Value::Object(capabilities))
}
