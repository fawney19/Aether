use aether_data::DataLayerError;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredPoolKeyCandidateRowsQuery,
    StoredRequestedModelCandidateRowsQuery,
};
use aether_scheduler_core::{
    auth_constraints_allow_api_format, collect_global_model_names_for_required_capability,
    enumerate_minimal_candidate_selection_with_model_directives, normalize_api_format,
    resolve_requested_global_model_name_with_model_directives,
    row_supports_requested_model_with_model_directives, EnumerateMinimalCandidateSelectionInput,
    SchedulerAuthConstraints, SchedulerMinimalCandidateSelectionCandidate,
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

    async fn read_minimal_candidate_selection_rows_for_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>;

    async fn read_minimal_candidate_selection_rows_for_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>;

    async fn read_minimal_candidate_selection_rows_for_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>;

    async fn read_pool_key_candidate_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>;
}

pub(crate) const REQUESTED_MODEL_CANDIDATE_PAGE_SIZE: u32 = 256;
pub(crate) const REQUESTED_MODEL_MAX_SCANNED_ROWS: u32 = 2048;

#[derive(Debug, Clone)]
pub(crate) struct RequestedModelCandidateRowsPage {
    pub(crate) rows: Vec<StoredMinimalCandidateSelectionRow>,
    pub(crate) scanned_rows: u32,
    pub(crate) end_of_requested_name: bool,
}

pub(crate) async fn read_requested_model_rows(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    requested_model_name: &str,
    enable_model_directives: bool,
) -> Result<Option<(String, Vec<StoredMinimalCandidateSelectionRow>)>, DataLayerError> {
    let fast_rows = read_requested_model_rows_fast_path(
        state,
        api_format,
        requested_model_name,
        enable_model_directives,
    )
    .await?;
    let mut rows = filter_rows_for_requested_model(
        fast_rows,
        requested_model_name,
        api_format,
        enable_model_directives,
    );
    if rows.is_empty() {
        let fallback_rows = state
            .read_minimal_candidate_selection_rows_for_api_format(api_format)
            .await?;
        rows = filter_rows_for_requested_model(
            fallback_rows,
            requested_model_name,
            api_format,
            enable_model_directives,
        );
    }
    if rows.is_empty() {
        return Ok(None);
    }

    let Some(resolved_global_model_name) =
        resolve_requested_global_model_name_with_model_directives(
            &rows,
            requested_model_name,
            api_format,
            enable_model_directives,
        )
    else {
        return Ok(None);
    };

    let resolved_rows = rows
        .into_iter()
        .filter(|row| row.global_model_name == resolved_global_model_name)
        .collect();

    Ok(Some((resolved_global_model_name, resolved_rows)))
}

fn filter_rows_for_requested_model(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    requested_model_name: &str,
    api_format: &str,
    enable_model_directives: bool,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    rows.into_iter()
        .filter(|row| {
            row_supports_requested_model_with_model_directives(
                row,
                requested_model_name,
                api_format,
                enable_model_directives,
            )
        })
        .collect()
}

async fn read_requested_model_rows_fast_path(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    requested_model_name: &str,
    enable_model_directives: bool,
) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
    let requested_names =
        requested_model_candidate_names(requested_model_name, enable_model_directives);

    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for requested_name in requested_names {
        if requested_name.is_empty() {
            continue;
        }
        let mut offset = 0;
        let mut scanned = 0;
        while scanned < REQUESTED_MODEL_MAX_SCANNED_ROWS {
            let limit =
                REQUESTED_MODEL_CANDIDATE_PAGE_SIZE.min(REQUESTED_MODEL_MAX_SCANNED_ROWS - scanned);
            let page = state
                .read_minimal_candidate_selection_rows_for_api_format_and_requested_model_page(
                    &StoredRequestedModelCandidateRowsQuery {
                        api_format: api_format.to_string(),
                        requested_model_name: requested_name.clone(),
                        offset,
                        limit,
                    },
                )
                .await?;
            if page.is_empty() {
                break;
            }
            let page_len = page.len() as u32;
            for row in page {
                if seen.insert((
                    row.endpoint_id.clone(),
                    row.key_id.clone(),
                    row.model_id.clone(),
                )) {
                    rows.push(row);
                }
            }
            scanned = scanned.saturating_add(page_len);
            if page_len < limit {
                break;
            }
            offset = offset.saturating_add(limit);
        }
    }
    Ok(rows)
}

pub(crate) fn requested_model_candidate_names(
    requested_model_name: &str,
    enable_model_directives: bool,
) -> Vec<String> {
    let mut requested_names = vec![requested_model_name.trim().to_string()];
    if enable_model_directives {
        if let Some(base_model) =
            crate::ai_serving::model_directive_base_model(requested_model_name)
        {
            if !requested_names.iter().any(|value| value == &base_model) {
                requested_names.push(base_model);
            }
        }
    }
    requested_names
}

pub(crate) async fn read_requested_model_rows_fast_path_page(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    requested_model_name: &str,
    requested_name: &str,
    offset: u32,
    limit: u32,
    enable_model_directives: bool,
) -> Result<RequestedModelCandidateRowsPage, DataLayerError> {
    let limit = limit.max(1);
    let page = state
        .read_minimal_candidate_selection_rows_for_api_format_and_requested_model_page(
            &StoredRequestedModelCandidateRowsQuery {
                api_format: api_format.to_string(),
                requested_model_name: requested_name.to_string(),
                offset,
                limit,
            },
        )
        .await?;
    let scanned_rows = page.len() as u32;
    let end_of_requested_name = scanned_rows < limit;
    let rows = filter_rows_for_requested_model(
        page,
        requested_model_name,
        api_format,
        enable_model_directives,
    );
    Ok(RequestedModelCandidateRowsPage {
        rows,
        scanned_rows,
        end_of_requested_name,
    })
}

pub(crate) async fn enumerate_minimal_candidate_selection_with_required_capabilities(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    requested_model_name: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&serde_json::Value>,
    enable_model_directives: bool,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, DataLayerError> {
    enumerate_minimal_candidate_selection_with_required_capabilities_for_request_operation(
        state,
        api_format,
        requested_model_name,
        require_streaming,
        auth_snapshot,
        required_capabilities,
        enable_model_directives,
        None,
    )
    .await
}

pub(crate) async fn enumerate_minimal_candidate_selection_with_required_capabilities_for_request_operation(
    state: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    requested_model_name: &str,
    require_streaming: bool,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&serde_json::Value>,
    enable_model_directives: bool,
    request_operation: Option<&str>,
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

    let Some((resolved_global_model_name, rows)) = read_requested_model_rows(
        state,
        &normalized_api_format,
        requested_model_name,
        enable_model_directives,
    )
    .await?
    else {
        return Ok(Vec::new());
    };
    let auth_constraints = auth_snapshot.map(auth_snapshot_constraints);
    enumerate_minimal_candidate_selection_with_model_directives(
        EnumerateMinimalCandidateSelectionInput {
            rows,
            normalized_api_format: &normalized_api_format,
            request_operation,
            requested_model_name,
            resolved_global_model_name: resolved_global_model_name.as_str(),
            require_streaming,
            required_capabilities,
            auth_constraints: auth_constraints.as_ref(),
        },
        enable_model_directives,
    )
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

pub(crate) fn auth_snapshot_constraints(
    snapshot: &GatewayAuthApiKeySnapshot,
) -> SchedulerAuthConstraints {
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

#[cfg(test)]
mod tests {
    use super::{
        read_requested_model_rows, read_requested_model_rows_fast_path_page,
        MinimalCandidateSelectionRowSource, StoredMinimalCandidateSelectionRow,
    };
    use aether_data::DataLayerError;
    use aether_data_contracts::repository::candidate_selection::{
        StoredPoolKeyCandidateRowsQuery, StoredRequestedModelCandidateRowsQuery,
    };
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingSelectionSource {
        fast_rows: Vec<StoredMinimalCandidateSelectionRow>,
        fallback_rows: Vec<StoredMinimalCandidateSelectionRow>,
        fast_calls: AtomicUsize,
        fallback_calls: AtomicUsize,
    }

    impl CountingSelectionSource {
        fn new(
            fast_rows: Vec<StoredMinimalCandidateSelectionRow>,
            fallback_rows: Vec<StoredMinimalCandidateSelectionRow>,
        ) -> Self {
            Self {
                fast_rows,
                fallback_rows,
                fast_calls: AtomicUsize::new(0),
                fallback_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl MinimalCandidateSelectionRowSource for CountingSelectionSource {
        async fn read_minimal_candidate_selection_rows_for_api_format_and_global_model(
            &self,
            _api_format: &str,
            _global_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            Ok(Vec::new())
        }

        async fn read_minimal_candidate_selection_rows_for_api_format_and_requested_model(
            &self,
            _api_format: &str,
            _requested_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            Ok(Vec::new())
        }

        async fn read_minimal_candidate_selection_rows_for_api_format_and_requested_model_page(
            &self,
            query: &StoredRequestedModelCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.fast_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .fast_rows
                .iter()
                .skip(query.offset as usize)
                .take(query.limit as usize)
                .cloned()
                .collect())
        }

        async fn read_minimal_candidate_selection_rows_for_api_format(
            &self,
            _api_format: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.fallback_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.fallback_rows.clone())
        }

        async fn read_pool_key_candidate_rows_for_group(
            &self,
            _query: &StoredPoolKeyCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            Ok(Vec::new())
        }
    }

    fn sample_row(global_model_name: &str) -> StoredMinimalCandidateSelectionRow {
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
            key_id: "key-1".to_string(),
            key_name: "key".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 10,
            key_global_priority_by_format: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: global_model_name.to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: global_model_name.to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    #[tokio::test]
    async fn requested_model_rows_use_fast_path_without_full_format_scan() {
        let source = CountingSelectionSource::new(vec![sample_row("gpt-5")], Vec::new());

        let result = read_requested_model_rows(&source, "openai:chat", "gpt-5", false)
            .await
            .expect("read should succeed")
            .expect("rows should resolve");

        assert_eq!(result.0, "gpt-5");
        assert_eq!(result.1.len(), 1);
        assert_eq!(source.fast_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.fallback_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn requested_model_rows_fall_back_to_full_format_scan_when_fast_path_misses() {
        let source = CountingSelectionSource::new(Vec::new(), vec![sample_row("gpt-5")]);

        let result = read_requested_model_rows(&source, "openai:chat", "gpt-5", false)
            .await
            .expect("read should succeed")
            .expect("rows should resolve");

        assert_eq!(result.0, "gpt-5");
        assert_eq!(result.1.len(), 1);
        assert_eq!(source.fast_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn requested_model_rows_fast_path_stops_at_scan_limit() {
        let mut rows = Vec::new();
        for index in 0..(super::REQUESTED_MODEL_MAX_SCANNED_ROWS + 5) {
            let mut row = sample_row("gpt-5");
            row.provider_id = format!("provider-{index}");
            row.endpoint_id = format!("endpoint-{index}");
            row.key_id = format!("key-{index}");
            row.model_id = format!("model-{index}");
            rows.push(row);
        }
        let source = CountingSelectionSource::new(rows, Vec::new());

        let result = read_requested_model_rows(&source, "openai:chat", "gpt-5", false)
            .await
            .expect("read should succeed")
            .expect("rows should resolve");

        assert_eq!(
            result.1.len(),
            super::REQUESTED_MODEL_MAX_SCANNED_ROWS as usize
        );
        assert_eq!(
            source.fast_calls.load(Ordering::SeqCst),
            (super::REQUESTED_MODEL_MAX_SCANNED_ROWS / super::REQUESTED_MODEL_CANDIDATE_PAGE_SIZE)
                as usize
        );
        assert_eq!(source.fallback_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn requested_model_rows_page_reads_only_requested_slice() {
        let mut rows = Vec::new();
        for index in 0..10 {
            let mut row = sample_row("gpt-5");
            row.key_id = format!("key-{index}");
            rows.push(row);
        }
        let source = CountingSelectionSource::new(rows, Vec::new());

        let page = read_requested_model_rows_fast_path_page(
            &source,
            "openai:chat",
            "gpt-5",
            "gpt-5",
            4,
            3,
            false,
        )
        .await
        .expect("page read should succeed");

        assert_eq!(page.scanned_rows, 3);
        assert!(!page.end_of_requested_name);
        assert_eq!(
            page.rows
                .iter()
                .map(|row| row.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-4", "key-5", "key-6"]
        );
        assert_eq!(source.fast_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.fallback_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn requested_model_rows_page_reports_end_of_requested_name() {
        let source = CountingSelectionSource::new(vec![sample_row("gpt-5")], Vec::new());

        let page = read_requested_model_rows_fast_path_page(
            &source,
            "openai:chat",
            "gpt-5",
            "gpt-5",
            0,
            3,
            false,
        )
        .await
        .expect("page read should succeed");

        assert_eq!(page.scanned_rows, 1);
        assert!(page.end_of_requested_name);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(source.fast_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.fallback_calls.load(Ordering::SeqCst), 0);
    }
}
