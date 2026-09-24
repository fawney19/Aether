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
        "openai:embedding" => Some("openai:embedding"),
        "openai:rerank" => Some("openai:rerank"),
        "claude:messages" => Some("claude:messages"),
        "gemini:generate_content" => Some("gemini:generate_content"),
        "gemini:embedding" => Some("gemini:embedding"),
        "jina:embedding" => Some("jina:embedding"),
        "jina:rerank" => Some("jina:rerank"),
        "doubao:embedding" => Some("doubao:embedding"),
        "aliyun:multimodal_embedding" => Some("aliyun:multimodal_embedding"),
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

const MODELS_EMBEDDING_QUERY_API_FORMATS: &[&str] = &[
    "openai:embedding",
    "jina:embedding",
    "gemini:embedding",
    "doubao:embedding",
    "aliyun:multimodal_embedding",
];
const MODELS_RERANK_QUERY_API_FORMATS: &[&str] = &["openai:rerank", "jina:rerank"];

pub(super) fn models_query_api_formats(api_format: &str) -> &'static [&'static str] {
    match crate::ai_serving::normalize_api_format_alias(api_format).as_str() {
        "openai:chat"
        | "openai:responses"
        | "openai:responses:compact"
        | "claude:messages"
        | "gemini:generate_content" => MODELS_CROSS_FORMAT_QUERY_API_FORMATS,
        "openai:image" => &["openai:image"],
        "openai:embedding"
        | "jina:embedding"
        | "gemini:embedding"
        | "doubao:embedding"
        | "aliyun:multimodal_embedding" => MODELS_EMBEDDING_QUERY_API_FORMATS,
        "openai:rerank" | "jina:rerank" => MODELS_RERANK_QUERY_API_FORMATS,
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

fn mapping_scope_matches_for_models(
    mapping: &StoredProviderModelMapping,
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> bool {
    let api_format_matches = mapping.api_formats.as_ref().is_none_or(|api_formats| {
        api_formats
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(api_format))
    });
    if !api_format_matches {
        return false;
    }

    mapping.endpoint_ids.as_ref().is_none_or(|endpoint_ids| {
        endpoint_ids
            .iter()
            .any(|endpoint_id| endpoint_id == &row.endpoint_id)
    })
}

fn candidate_model_names_for_models(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::from([row.model_provider_model_name.clone()]);
    if let Some(mappings) = row.model_provider_model_mappings.as_ref() {
        for mapping in mappings {
            if mapping_scope_matches_for_models(mapping, row, api_format) {
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

pub(crate) fn filter_eligible_model_rows(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
    api_format: &str,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    let constraints =
        auth_snapshot.map(crate::data::candidate_selection::auth_snapshot_constraints);
    filter_eligible_model_rows_for_constraints(rows, constraints.as_ref(), api_format)
}

pub(crate) async fn filter_eligible_model_rows_for_state(
    state: &crate::AppState,
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
    api_format: &str,
) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::GatewayError> {
    let constraints =
        auth_snapshot.map(crate::data::candidate_selection::auth_snapshot_constraints);
    let rows = crate::data::candidate_selection::materialize_pool_candidate_rows_for_auth(
        state.data.as_ref(),
        rows,
        api_format,
        constraints.as_ref(),
    )
    .await
    .map_err(|error| crate::GatewayError::Internal(error.to_string()))?;
    Ok(filter_eligible_model_rows_for_constraints(
        rows,
        constraints.as_ref(),
        api_format,
    ))
}

pub(crate) fn filter_eligible_model_rows_for_constraints(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    auth_constraints: Option<&aether_scheduler_core::SchedulerAuthConstraints>,
    api_format: &str,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    rows.into_iter()
        .filter(|row| {
            aether_scheduler_core::auth_constraints_allow_provider(
                auth_constraints,
                &row.provider_id,
                &row.provider_name,
                &row.provider_type,
            )
        })
        .filter(|row| {
            aether_scheduler_core::auth_constraints_allow_provider_key(
                auth_constraints,
                &row.provider_id,
                &row.key_id,
            )
        })
        .filter(|row| {
            aether_scheduler_core::auth_constraints_allow_model(
                auth_constraints,
                &row.global_model_name,
                &row.global_model_name,
            )
        })
        .filter(|row| row_exposes_global_model_for_models(row, api_format))
        .collect()
}

pub(super) fn filter_rows_for_models(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
    api_format: &str,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    let mut filtered = filter_eligible_model_rows(rows, auth_snapshot, api_format);
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

#[cfg(test)]
mod tests {
    use super::{filter_eligible_model_rows, filter_eligible_model_rows_for_state};
    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data_contracts::repository::auth::{
        ResolvedAuthApiKeySnapshot, StoredAuthApiKeySnapshot,
    };
    use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
    use std::sync::Arc;

    fn row(key_id: &str) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-1".to_string(),
            provider_name: "Provider 1".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 0,
            provider_is_active: true,
            provider_pool_enabled: false,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 0,
            key_global_priority_by_format: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-1".to_string(),
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

    fn auth_snapshot(standalone: bool) -> ResolvedAuthApiKeySnapshot {
        let mut stored = StoredAuthApiKeySnapshot::new(
            "user-1".to_string(),
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["provider-1"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
            "api-key-1".to_string(),
            Some("default".to_string()),
            true,
            false,
            standalone,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("snapshot should build");
        stored
            .user_provider_key_policies
            .insert("provider-1".to_string(), vec!["key-allowed".to_string()]);
        ResolvedAuthApiKeySnapshot::from_stored(stored, 0)
    }

    fn pool_rows() -> Vec<StoredMinimalCandidateSelectionRow> {
        let mut first = row("key-allowed-first");
        first.provider_pool_enabled = true;
        first.key_allowed_models = None;
        let mut later = row("key-allowed-later");
        later.provider_pool_enabled = true;
        later.key_allowed_models = None;
        vec![first, later]
    }

    fn pool_state(rows: Vec<StoredMinimalCandidateSelectionRow>) -> crate::AppState {
        let repository = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows));
        crate::AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_minimal_candidate_selection_reader_for_tests(
                    repository,
                ),
            )
    }

    fn pool_auth_snapshot(allowed_key_ids: Vec<&str>) -> ResolvedAuthApiKeySnapshot {
        let mut snapshot = auth_snapshot(false);
        snapshot.user_provider_key_policies.insert(
            "provider-1".to_string(),
            allowed_key_ids.into_iter().map(str::to_string).collect(),
        );
        snapshot
    }

    #[test]
    fn model_rows_exclude_keys_outside_user_group_policy() {
        let snapshot = auth_snapshot(false);
        let rows = filter_eligible_model_rows(
            vec![row("key-blocked"), row("key-allowed")],
            Some(&snapshot),
            "openai:chat",
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key_id, "key-allowed");
    }

    #[test]
    fn standalone_model_rows_ignore_user_group_key_policy() {
        let snapshot = auth_snapshot(true);
        let rows = filter_eligible_model_rows(
            vec![row("key-blocked"), row("key-allowed")],
            Some(&snapshot),
            "openai:chat",
        );

        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn pool_model_rows_use_later_allowed_key_instead_of_representative() {
        let pool_rows = pool_rows();
        let representative = pool_rows[0].clone();
        let state = pool_state(pool_rows);
        let snapshot = pool_auth_snapshot(vec!["key-allowed-later"]);

        let rows = filter_eligible_model_rows_for_state(
            &state,
            vec![representative],
            Some(&snapshot),
            "openai:chat",
        )
        .await
        .expect("pool model rows should resolve");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key_id, "key-allowed-later");
        assert!(!rows[0].provider_pool_enabled);
    }

    #[tokio::test]
    async fn empty_pool_key_policy_hides_model() {
        let pool_rows = pool_rows();
        let representative = pool_rows[0].clone();
        let state = pool_state(pool_rows);
        let snapshot = pool_auth_snapshot(Vec::new());

        let rows = filter_eligible_model_rows_for_state(
            &state,
            vec![representative],
            Some(&snapshot),
            "openai:chat",
        )
        .await
        .expect("pool model rows should resolve");

        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn allowed_pool_key_without_model_permission_hides_model() {
        let mut pool_rows = pool_rows();
        pool_rows[1].key_allowed_models = Some(vec!["another-model".to_string()]);
        let representative = pool_rows[0].clone();
        let state = pool_state(pool_rows);
        let snapshot = pool_auth_snapshot(vec!["key-allowed-later"]);

        let rows = filter_eligible_model_rows_for_state(
            &state,
            vec![representative],
            Some(&snapshot),
            "openai:chat",
        )
        .await
        .expect("pool model rows should resolve");

        assert!(rows.is_empty());
    }
}
