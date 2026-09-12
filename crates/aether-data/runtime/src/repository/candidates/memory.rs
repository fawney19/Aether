use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use super::{
    request_candidate_lifecycle_would_regress, PublicHealthStatusCount, PublicHealthTimelineBucket,
    RequestCandidateReadRepository, RequestCandidateStatus, RequestCandidateWriteRepository,
    StoredRequestCandidate, UpsertRequestCandidateRecord,
};
use crate::DataLayerError;
use async_trait::async_trait;

fn sanitize_stored_candidate(mut candidate: StoredRequestCandidate) -> StoredRequestCandidate {
    candidate.sanitize_for_persistence();
    candidate
}

fn merge_extra_data(
    existing: Option<serde_json::Value>,
    overlay: Option<serde_json::Value>,
    preserve_error_details: bool,
) -> Option<serde_json::Value> {
    match (existing, overlay) {
        (
            Some(serde_json::Value::Object(mut existing_object)),
            Some(serde_json::Value::Object(mut overlay_object)),
        ) => {
            if preserve_error_details {
                for key in [
                    "upstream_response",
                    "error_flow",
                    "failure_diagnostic",
                    "request_conversion_error",
                    "request_body_build_error",
                ] {
                    overlay_object.remove(key);
                }
            }
            existing_object.extend(overlay_object);
            Some(serde_json::Value::Object(existing_object))
        }
        (_existing, Some(overlay)) => Some(overlay),
        (existing, None) => existing,
    }
}

#[derive(Debug, Default)]
pub struct InMemoryRequestCandidateRepository {
    rows: RwLock<CandidateRows>,
}

#[derive(Debug, Default)]
struct CandidateRows {
    by_id: BTreeMap<String, StoredRequestCandidate>,
    by_request: BTreeMap<String, BTreeSet<String>>,
    by_created: BTreeSet<(std::cmp::Reverse<u64>, String)>,
}

impl CandidateRows {
    fn remove(&mut self, id: &str) -> Option<StoredRequestCandidate> {
        let row = self.by_id.remove(id)?;
        self.by_created
            .remove(&(std::cmp::Reverse(row.created_at_unix_ms), row.id.clone()));
        if let Some(ids) = self.by_request.get_mut(&row.request_id) {
            ids.remove(id);
            if ids.is_empty() {
                self.by_request.remove(&row.request_id);
            }
        }
        Some(row)
    }

    fn insert(&mut self, row: StoredRequestCandidate) -> &StoredRequestCandidate {
        // Keep all indexes behind one lock and sanitize every insertion. Reads
        // can clone these records without rebuilding their diagnostic JSON.
        let row = sanitize_stored_candidate(row);
        self.remove(&row.id);
        self.by_request
            .entry(row.request_id.clone())
            .or_default()
            .insert(row.id.clone());
        self.by_created
            .insert((std::cmp::Reverse(row.created_at_unix_ms), row.id.clone()));
        self.by_id
            .entry(row.id.clone())
            .insert_entry(row)
            .into_mut()
    }

    fn for_request(&self, request_id: &str) -> impl Iterator<Item = &StoredRequestCandidate> {
        self.by_request
            .get(request_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.by_id.get(id))
    }
}

impl InMemoryRequestCandidateRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredRequestCandidate>,
    {
        let mut rows = CandidateRows::default();
        for item in items {
            rows.insert(item);
        }
        Self {
            rows: RwLock::new(rows),
        }
    }
}

#[async_trait]
impl RequestCandidateReadRepository for InMemoryRequestCandidateRepository {
    async fn list_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        let mut rows = self
            .rows
            .read()
            .expect("request candidate repository lock")
            .for_request(request_id)
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.candidate_index
                .cmp(&right.candidate_index)
                .then(left.retry_index.cmp(&right.retry_index))
                .then(left.created_at_unix_ms.cmp(&right.created_at_unix_ms))
        });
        Ok(rows)
    }

    async fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let rows = self.rows.read().expect("request candidate repository lock");
        Ok(rows
            .by_created
            .iter()
            .take(limit)
            .filter_map(|(_, id)| rows.by_id.get(id))
            .cloned()
            .collect())
    }

    async fn list_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut rows = self
            .rows
            .read()
            .expect("request candidate repository lock")
            .by_id
            .values()
            .filter(|row| row.provider_id.as_deref() == Some(provider_id))
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|entry| std::cmp::Reverse(entry.created_at_unix_ms));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn list_recent_runtime(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        let rows = self.rows.read().expect("request candidate repository lock");
        Ok(rows
            .by_created
            .iter()
            .take(limit)
            .filter_map(|(_, id)| rows.by_id.get(id))
            .map(StoredRequestCandidate::runtime_snapshot)
            .collect())
    }

    async fn list_finalized_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if endpoint_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let endpoint_ids = endpoint_ids.iter().cloned().collect::<BTreeSet<_>>();
        let mut rows = self
            .rows
            .read()
            .expect("request candidate repository lock")
            .by_id
            .values()
            .filter(|row| {
                row.endpoint_id
                    .as_ref()
                    .is_some_and(|endpoint_id| endpoint_ids.contains(endpoint_id))
                    && row.created_at_unix_ms >= since_unix_secs * 1000
                    && matches!(
                        row.status,
                        RequestCandidateStatus::Success
                            | RequestCandidateStatus::Failed
                            | RequestCandidateStatus::Skipped
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by_key(|entry| std::cmp::Reverse(entry.created_at_unix_ms));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn count_finalized_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<PublicHealthStatusCount>, DataLayerError> {
        if endpoint_ids.is_empty() {
            return Ok(Vec::new());
        }

        let endpoint_ids = endpoint_ids.iter().cloned().collect::<BTreeSet<_>>();
        let mut counts = BTreeMap::<(String, &'static str), u64>::new();
        for row in self
            .rows
            .read()
            .expect("request candidate repository lock")
            .by_id
            .values()
        {
            let Some(endpoint_id) = row.endpoint_id.as_ref() else {
                continue;
            };
            if !endpoint_ids.contains(endpoint_id)
                || row.created_at_unix_ms < since_unix_secs * 1000
            {
                continue;
            }
            if !matches!(
                row.status,
                RequestCandidateStatus::Success
                    | RequestCandidateStatus::Failed
                    | RequestCandidateStatus::Skipped
            ) {
                continue;
            }
            let status_key = match row.status {
                RequestCandidateStatus::Success => "success",
                RequestCandidateStatus::Failed => "failed",
                RequestCandidateStatus::Skipped => "skipped",
                _ => continue,
            };
            *counts.entry((endpoint_id.clone(), status_key)).or_insert(0) += 1;
        }

        Ok(counts
            .into_iter()
            .map(|((endpoint_id, status_key), count)| {
                let status = match status_key {
                    "success" => RequestCandidateStatus::Success,
                    "failed" => RequestCandidateStatus::Failed,
                    "skipped" => RequestCandidateStatus::Skipped,
                    _ => unreachable!("filtered status should stay finalized"),
                };
                PublicHealthStatusCount {
                    endpoint_id,
                    status,
                    count,
                }
            })
            .collect())
    }

    async fn aggregate_finalized_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<Vec<PublicHealthTimelineBucket>, DataLayerError> {
        if endpoint_ids.is_empty() || segments == 0 || until_unix_secs < since_unix_secs {
            return Ok(Vec::new());
        }

        let endpoint_ids = endpoint_ids.iter().cloned().collect::<BTreeSet<_>>();
        let span_ms = until_unix_secs.saturating_sub(since_unix_secs) * 1000;
        let mut buckets = BTreeMap::<(String, u32), PublicHealthTimelineBucket>::new();

        for row in self
            .rows
            .read()
            .expect("request candidate repository lock")
            .by_id
            .values()
        {
            let Some(endpoint_id) = row.endpoint_id.as_ref() else {
                continue;
            };
            if !endpoint_ids.contains(endpoint_id)
                || row.created_at_unix_ms < since_unix_secs * 1000
                || row.created_at_unix_ms > until_unix_secs * 1000
            {
                continue;
            }
            if !matches!(
                row.status,
                RequestCandidateStatus::Success
                    | RequestCandidateStatus::Failed
                    | RequestCandidateStatus::Skipped
            ) {
                continue;
            }

            let segment_idx = if span_ms == 0 {
                0
            } else {
                let offset = row
                    .created_at_unix_ms
                    .saturating_sub(since_unix_secs * 1000);
                let idx = ((offset as u128) * (segments as u128) / (span_ms as u128)) as u32;
                idx.min(segments.saturating_sub(1))
            };
            let bucket = buckets
                .entry((endpoint_id.clone(), segment_idx))
                .or_insert_with(|| PublicHealthTimelineBucket {
                    endpoint_id: endpoint_id.clone(),
                    segment_idx,
                    total_count: 0,
                    success_count: 0,
                    failed_count: 0,
                    min_created_at_unix_ms: None,
                    max_created_at_unix_ms: None,
                });
            bucket.total_count += 1;
            if row.status == RequestCandidateStatus::Success {
                bucket.success_count += 1;
            } else if row.status == RequestCandidateStatus::Failed {
                bucket.failed_count += 1;
            }
            bucket.min_created_at_unix_ms = Some(
                bucket
                    .min_created_at_unix_ms
                    .map(|value| value.min(row.created_at_unix_ms))
                    .unwrap_or(row.created_at_unix_ms),
            );
            bucket.max_created_at_unix_ms = Some(
                bucket
                    .max_created_at_unix_ms
                    .map(|value| value.max(row.created_at_unix_ms))
                    .unwrap_or(row.created_at_unix_ms),
            );
        }

        Ok(buckets.into_values().collect())
    }
}

#[async_trait]
impl RequestCandidateWriteRepository for InMemoryRequestCandidateRepository {
    async fn upsert(
        &self,
        mut candidate: UpsertRequestCandidateRecord,
    ) -> Result<StoredRequestCandidate, DataLayerError> {
        candidate.sanitize_for_persistence();
        candidate.validate()?;

        let mut rows = self
            .rows
            .write()
            .expect("request candidate repository lock");
        let existing = rows
            .for_request(&candidate.request_id)
            .find(|row| {
                row.candidate_index == candidate.candidate_index
                    && row.retry_index == candidate.retry_index
            })
            .cloned();

        let preserve_existing_lifecycle = existing.as_ref().is_some_and(|row| {
            request_candidate_lifecycle_would_regress(row.status, candidate.status)
        });
        let merged_status = if preserve_existing_lifecycle {
            existing
                .as_ref()
                .map(|row| row.status)
                .unwrap_or(candidate.status)
        } else {
            candidate.status
        };
        let created_at_unix_ms = existing
            .as_ref()
            .map(|row| row.created_at_unix_ms)
            .or(candidate.created_at_unix_ms)
            .or(candidate.started_at_unix_ms)
            .or(candidate.finished_at_unix_ms)
            .unwrap_or_default();

        let stored = StoredRequestCandidate {
            id: existing
                .as_ref()
                .map(|row| row.id.clone())
                .unwrap_or_else(|| candidate.id.clone()),
            request_id: candidate.request_id.clone(),
            user_id: existing
                .as_ref()
                .and_then(|row| row.user_id.clone())
                .or(candidate.user_id),
            api_key_id: existing
                .as_ref()
                .and_then(|row| row.api_key_id.clone())
                .or(candidate.api_key_id),
            username: existing
                .as_ref()
                .and_then(|row| row.username.clone())
                .or(candidate.username),
            api_key_name: existing
                .as_ref()
                .and_then(|row| row.api_key_name.clone())
                .or(candidate.api_key_name),
            candidate_index: candidate.candidate_index,
            retry_index: candidate.retry_index,
            provider_id: existing
                .as_ref()
                .and_then(|row| row.provider_id.clone())
                .or(candidate.provider_id),
            endpoint_id: existing
                .as_ref()
                .and_then(|row| row.endpoint_id.clone())
                .or(candidate.endpoint_id),
            key_id: existing
                .as_ref()
                .and_then(|row| row.key_id.clone())
                .or(candidate.key_id),
            status: merged_status,
            skip_reason: candidate
                .skip_reason
                .or_else(|| existing.as_ref().and_then(|row| row.skip_reason.clone())),
            is_cached: candidate
                .is_cached
                .unwrap_or_else(|| existing.as_ref().map(|row| row.is_cached).unwrap_or(false)),
            status_code: if preserve_existing_lifecycle {
                existing.as_ref().and_then(|row| row.status_code)
            } else {
                candidate
                    .status_code
                    .or_else(|| existing.as_ref().and_then(|row| row.status_code))
            },
            error_type: if preserve_existing_lifecycle {
                existing.as_ref().and_then(|row| row.error_type.clone())
            } else {
                candidate
                    .error_type
                    .or_else(|| existing.as_ref().and_then(|row| row.error_type.clone()))
            },
            error_message: if preserve_existing_lifecycle {
                existing.as_ref().and_then(|row| row.error_message.clone())
            } else {
                candidate
                    .error_message
                    .or_else(|| existing.as_ref().and_then(|row| row.error_message.clone()))
            },
            latency_ms: if preserve_existing_lifecycle {
                existing.as_ref().and_then(|row| row.latency_ms)
            } else {
                candidate
                    .latency_ms
                    .or_else(|| existing.as_ref().and_then(|row| row.latency_ms))
            },
            concurrent_requests: candidate
                .concurrent_requests
                .or_else(|| existing.as_ref().and_then(|row| row.concurrent_requests)),
            extra_data: merge_extra_data(
                existing.as_ref().and_then(|row| row.extra_data.clone()),
                candidate.extra_data,
                preserve_existing_lifecycle,
            ),
            required_capabilities: candidate.required_capabilities.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|row| row.required_capabilities.clone())
            }),
            created_at_unix_ms,
            started_at_unix_ms: existing
                .as_ref()
                .and_then(|row| row.started_at_unix_ms)
                .or(candidate.started_at_unix_ms),
            finished_at_unix_ms: if preserve_existing_lifecycle {
                existing.as_ref().and_then(|row| row.finished_at_unix_ms)
            } else {
                candidate
                    .finished_at_unix_ms
                    .or_else(|| existing.as_ref().and_then(|row| row.finished_at_unix_ms))
            },
        };
        Ok(rows.insert(stored).clone())
    }

    async fn delete_created_before(
        &self,
        created_before_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        if limit == 0 {
            return Ok(0);
        }

        let mut rows = self
            .rows
            .write()
            .expect("request candidate repository lock");
        let mut ids = rows
            .by_id
            .values()
            .filter(|row| row.created_at_unix_ms < created_before_unix_secs * 1000)
            .map(|row| (row.created_at_unix_ms, row.id.clone()))
            .collect::<Vec<_>>();
        ids.sort();

        let mut deleted = 0usize;
        for (_, id) in ids.into_iter().take(limit) {
            if rows.remove(&id).is_some() {
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryRequestCandidateRepository;
    use crate::repository::candidates::{
        RequestCandidateReadRepository, RequestCandidateStatus, RequestCandidateWriteRepository,
        StoredRequestCandidate, UpsertRequestCandidateRecord,
    };
    use serde_json::json;

    fn sample_candidate(
        id: &str,
        request_id: &str,
        created_at_unix_ms: i64,
    ) -> StoredRequestCandidate {
        StoredRequestCandidate::new(
            id.to_string(),
            request_id.to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Success,
            None,
            false,
            Some(200),
            None,
            None,
            Some(10),
            Some(1),
            None,
            None,
            created_at_unix_ms,
            Some(created_at_unix_ms),
            Some(created_at_unix_ms + 1),
        )
        .expect("candidate should build")
    }

    #[tokio::test]
    async fn lists_request_candidates_by_request_id_in_candidate_order() {
        let repository = InMemoryRequestCandidateRepository::seed(vec![
            sample_candidate("cand-2", "req-1", 200),
            sample_candidate("cand-1", "req-1", 100),
            sample_candidate("cand-3", "req-2", 300),
        ]);

        let rows = repository
            .list_by_request_id("req-1")
            .await
            .expect("list should succeed");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].request_id, "req-1");
        assert_eq!(rows[1].request_id, "req-1");
    }

    #[tokio::test]
    async fn request_index_tracks_replaced_ids_and_removes_empty_requests() {
        let repository = InMemoryRequestCandidateRepository::seed([
            sample_candidate("same-id", "old-request", 100),
            sample_candidate("same-id", "new-request", 200),
            sample_candidate("other-id", "new-request", 300),
        ]);
        assert!(repository
            .list_by_request_id("old-request")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            repository
                .list_by_request_id("new-request")
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(repository.delete_created_before(1, 1).await.unwrap(), 1);
        let remaining = repository.list_by_request_id("new-request").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "other-id");
        assert_eq!(repository.delete_created_before(1, 1).await.unwrap(), 1);
        let rows = repository.rows.read().unwrap();
        assert!(rows.by_id.is_empty());
        assert!(rows.by_request.is_empty());
        assert!(rows.by_created.is_empty());
    }

    #[tokio::test]
    async fn recent_index_preserves_equal_timestamp_order_and_replaced_dates() {
        let repository = InMemoryRequestCandidateRepository::seed([
            sample_candidate("b", "req-b", 400),
            sample_candidate("a", "req-a", 200),
            sample_candidate("c", "req-c", 200),
            sample_candidate("b", "req-b", 100),
        ]);
        let recent = repository.list_recent(2).await.unwrap();
        assert_eq!(
            recent.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(repository.list_recent(0).await.unwrap().len(), 0);
        assert_eq!(repository.rows.read().unwrap().by_created.len(), 3);
    }

    #[tokio::test]
    async fn runtime_reads_keep_metadata_without_diagnostic_payloads() {
        let mut candidate = sample_candidate("candidate", "request", 100);
        candidate.extra_data = Some(json!({"upstream_response": {"body": "x".repeat(32_768)}}));
        candidate.error_message = Some("diagnostic detail".into());
        candidate.required_capabilities = Some(json!({"vision": true}));
        candidate.concurrent_requests = Some(17);
        let repository = InMemoryRequestCandidateRepository::seed([candidate]);
        let full = repository.list_recent(1).await.unwrap();
        let runtime = repository.list_recent_runtime(1).await.unwrap();
        assert_eq!(
            runtime,
            full.iter()
                .map(StoredRequestCandidate::runtime_snapshot)
                .collect::<Vec<_>>()
        );
        assert_eq!(runtime[0].concurrent_requests, Some(17));
        assert!(runtime[0].extra_data.is_none());
        assert!(runtime[0].error_message.is_none());
        assert!(runtime[0].required_capabilities.is_none());
        assert!(full[0].extra_data.is_some());
        assert_eq!(repository.list_recent(1).await.unwrap(), full);
        assert!(repository.list_recent_runtime(0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn lists_recent_request_candidates_in_descending_created_order() {
        let repository = InMemoryRequestCandidateRepository::seed(vec![
            sample_candidate("cand-1", "req-1", 100),
            sample_candidate("cand-2", "req-2", 200),
        ]);

        let rows = repository
            .list_recent(10)
            .await
            .expect("list recent should succeed");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "cand-2");
        assert_eq!(rows[1].id, "cand-1");
    }

    #[tokio::test]
    async fn seed_and_reads_sanitize_candidates_that_bypass_contract_constructors() {
        let raw_candidate = StoredRequestCandidate {
            id: "cand-raw".to_string(),
            request_id: "req-raw".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            candidate_index: 0,
            retry_index: 0,
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: None,
            status: RequestCandidateStatus::Failed,
            skip_reason: Some("secret=/private/path".to_string()),
            is_cached: false,
            status_code: Some(500),
            error_type: Some("token=secret".to_string()),
            error_message: Some("Bearer secret-token".to_string()),
            latency_ms: Some(10),
            concurrent_requests: Some(1),
            extra_data: Some(json!({
                "gateway_execution_runtime": true,
                "request_headers": {"authorization": "Bearer secret-token"},
                "request_body": {"password": "secret"}
            })),
            required_capabilities: Some(json!({
                "streaming": "true",
                "internal_capability": "secret"
            })),
            created_at_unix_ms: 100,
            started_at_unix_ms: Some(100),
            finished_at_unix_ms: Some(110),
        };
        let repository = InMemoryRequestCandidateRepository::seed(vec![raw_candidate.clone()]);

        {
            let stored = repository
                .rows
                .read()
                .expect("request candidate repository lock");
            let candidate = stored
                .by_id
                .get("cand-raw")
                .expect("seeded candidate should exist");
            assert_eq!(
                candidate.error_message.as_deref(),
                Some("Bearer secret-token")
            );
            assert_eq!(candidate.skip_reason.as_deref(), Some("unclassified_skip"));
            assert_eq!(candidate.error_type.as_deref(), Some("unclassified_error"));
            assert_eq!(
                candidate.extra_data,
                Some(json!({"gateway_execution_runtime": true}))
            );
            assert_eq!(
                candidate.required_capabilities,
                Some(json!({"streaming": true}))
            );
        }

        let mut bypassed_candidate = raw_candidate;
        bypassed_candidate.id = "cand-bypassed".to_string();
        bypassed_candidate.request_id = "req-bypassed".to_string();
        repository
            .rows
            .write()
            .expect("request candidate repository lock")
            .insert(bypassed_candidate);

        let rows = repository
            .list_recent(10)
            .await
            .expect("list recent should succeed");
        let candidate = rows
            .iter()
            .find(|candidate| candidate.id == "cand-bypassed")
            .expect("bypassed candidate should be returned");
        assert_eq!(
            candidate.error_message.as_deref(),
            Some("Bearer secret-token")
        );
        assert_eq!(
            candidate.extra_data,
            Some(json!({"gateway_execution_runtime": true}))
        );
        assert_eq!(
            candidate.required_capabilities,
            Some(json!({"streaming": true}))
        );

        let merged = repository
            .upsert(UpsertRequestCandidateRecord {
                id: "cand-merged".to_string(),
                request_id: "req-bypassed".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                candidate_index: 0,
                retry_index: 0,
                provider_id: None,
                endpoint_id: None,
                key_id: None,
                status: RequestCandidateStatus::Success,
                skip_reason: None,
                is_cached: None,
                status_code: Some(200),
                error_type: None,
                error_message: Some("Bearer new-secret".to_string()),
                latency_ms: Some(12),
                concurrent_requests: None,
                extra_data: Some(json!({
                    "stream_completed": true,
                    "request_body": {"password": "new-secret"}
                })),
                required_capabilities: Some(json!({
                    "vision": 1,
                    "internal_capability": "new-secret"
                })),
                created_at_unix_ms: Some(100),
                started_at_unix_ms: Some(100),
                finished_at_unix_ms: Some(112),
            })
            .await
            .expect("candidate merge should succeed");
        assert_eq!(merged.id, "cand-bypassed");
        assert_eq!(merged.error_message.as_deref(), Some("Bearer secret-token"));
        assert_eq!(
            merged.extra_data,
            Some(json!({
                "gateway_execution_runtime": true,
                "stream_completed": true
            }))
        );
        assert_eq!(merged.required_capabilities, Some(json!({"vision": true})));
    }

    #[tokio::test]
    async fn aggregates_finalized_health_data_by_endpoint_ids() {
        let repository = InMemoryRequestCandidateRepository::seed(vec![
            sample_candidate("cand-1", "req-1", 100_000),
            sample_candidate("cand-2", "req-2", 200_000),
        ]);

        let counts = repository
            .count_finalized_statuses_by_endpoint_ids_since(&["endpoint-1".to_string()], 0)
            .await
            .expect("count should succeed");
        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].endpoint_id, "endpoint-1");
        assert_eq!(counts[0].status, RequestCandidateStatus::Success);
        assert_eq!(counts[0].count, 2);

        let timeline = repository
            .aggregate_finalized_timeline_by_endpoint_ids_since(
                &["endpoint-1".to_string()],
                0,
                300,
                3,
            )
            .await
            .expect("timeline should succeed");
        assert_eq!(timeline.len(), 2);

        let attempts = repository
            .list_finalized_by_endpoint_ids_since(&["endpoint-1".to_string()], 0, 1)
            .await
            .expect("attempt list should succeed");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].id, "cand-2");
    }

    #[tokio::test]
    async fn upsert_writes_and_updates_request_candidate() {
        let repository = InMemoryRequestCandidateRepository::default();
        let created = repository
            .upsert(UpsertRequestCandidateRecord {
                id: "cand-1".to_string(),
                request_id: "req-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("api-key-1".to_string()),
                username: Some("alice".to_string()),
                api_key_name: Some("default".to_string()),
                candidate_index: 0,
                retry_index: 0,
                provider_id: Some("provider-1".to_string()),
                endpoint_id: Some("endpoint-1".to_string()),
                key_id: Some("key-1".to_string()),
                status: RequestCandidateStatus::Available,
                skip_reason: None,
                is_cached: Some(false),
                status_code: None,
                error_type: None,
                error_message: None,
                latency_ms: None,
                concurrent_requests: None,
                extra_data: Some(json!({
                    "execution_strategy": "local_cross_format",
                    "provider_name": "primary",
                })),
                required_capabilities: Some(json!({"streaming": true})),
                created_at_unix_ms: Some(100),
                started_at_unix_ms: None,
                finished_at_unix_ms: None,
            })
            .await
            .expect("create should succeed");
        assert_eq!(created.id, "cand-1");
        assert_eq!(created.status, RequestCandidateStatus::Available);

        let updated = repository
            .upsert(UpsertRequestCandidateRecord {
                id: "cand-1-replacement".to_string(),
                request_id: "req-1".to_string(),
                user_id: Some("attacker-user".to_string()),
                api_key_id: Some("attacker-api-key".to_string()),
                username: Some("mallory".to_string()),
                api_key_name: Some("attacker-key".to_string()),
                candidate_index: 0,
                retry_index: 0,
                provider_id: Some("attacker-provider".to_string()),
                endpoint_id: Some("attacker-endpoint".to_string()),
                key_id: Some("attacker-provider-key".to_string()),
                status: RequestCandidateStatus::Success,
                skip_reason: None,
                is_cached: None,
                status_code: Some(200),
                error_type: None,
                error_message: None,
                latency_ms: Some(25),
                concurrent_requests: Some(2),
                extra_data: Some(json!({
                    "provider_api_format": "openai:responses",
                    "provider_name": "updated",
                })),
                required_capabilities: Some(json!({"vision": true})),
                created_at_unix_ms: None,
                started_at_unix_ms: Some(101),
                finished_at_unix_ms: Some(102),
            })
            .await
            .expect("update should succeed");
        assert_eq!(updated.id, "cand-1");
        assert_eq!(updated.status, RequestCandidateStatus::Success);
        assert_eq!(updated.user_id.as_deref(), Some("user-1"));
        assert_eq!(updated.api_key_id.as_deref(), Some("api-key-1"));
        assert!(updated.username.is_none());
        assert!(updated.api_key_name.is_none());
        assert_eq!(updated.provider_id.as_deref(), Some("provider-1"));
        assert_eq!(updated.endpoint_id.as_deref(), Some("endpoint-1"));
        assert_eq!(updated.key_id.as_deref(), Some("key-1"));
        assert_eq!(updated.required_capabilities, Some(json!({"vision": true})));
        assert_eq!(updated.status_code, Some(200));
        assert_eq!(updated.latency_ms, Some(25));
        assert_eq!(
            updated
                .extra_data
                .as_ref()
                .and_then(|value| value.get("execution_strategy")),
            Some(&json!("local_cross_format"))
        );
        assert_eq!(
            updated
                .extra_data
                .as_ref()
                .and_then(|value| value.get("provider_api_format")),
            Some(&json!("openai:responses"))
        );
        assert_eq!(
            updated
                .extra_data
                .as_ref()
                .and_then(|value| value.get("provider_name")),
            None
        );
        assert_eq!(updated.started_at_unix_ms, Some(101));
    }

    #[tokio::test]
    async fn upsert_keeps_first_terminal_candidate_fact_when_another_terminal_arrives_late() {
        let existing = StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            RequestCandidateStatus::Failed,
            None,
            false,
            Some(503),
            Some("upstream_error".to_string()),
            Some("retryable upstream failure".to_string()),
            Some(45),
            Some(1),
            Some(json!({
                "stream_completed": true,
                "upstream_response": {
                    "status_code": 503,
                    "body": {"error": {"message": "original upstream failure"}}
                }
            })),
            None,
            100,
            Some(101),
            Some(145),
        )
        .expect("candidate should build");
        let repository = InMemoryRequestCandidateRepository::seed(vec![existing]);

        let updated = repository
            .upsert(UpsertRequestCandidateRecord {
                id: "cand-1-late".to_string(),
                request_id: "req-1".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                candidate_index: 0,
                retry_index: 0,
                provider_id: None,
                endpoint_id: None,
                key_id: None,
                status: RequestCandidateStatus::Success,
                skip_reason: None,
                is_cached: None,
                status_code: Some(200),
                error_type: None,
                error_message: None,
                latency_ms: Some(9_999),
                concurrent_requests: Some(2),
                extra_data: Some(json!({
                    "gateway_execution_runtime": true,
                    "upstream_response": {"status_code": 200, "body": "late unrelated response"}
                })),
                required_capabilities: None,
                created_at_unix_ms: None,
                started_at_unix_ms: Some(102),
                finished_at_unix_ms: None,
            })
            .await
            .expect("late update should succeed");

        assert_eq!(updated.id, "cand-1");
        assert_eq!(updated.status, RequestCandidateStatus::Failed);
        assert_eq!(updated.status_code, Some(503));
        assert_eq!(updated.error_type.as_deref(), Some("upstream_error"));
        assert_eq!(
            updated.error_message.as_deref(),
            Some("retryable upstream failure")
        );
        assert_eq!(updated.latency_ms, Some(45));
        assert_eq!(updated.concurrent_requests, Some(2));
        assert_eq!(updated.finished_at_unix_ms, Some(145));
        assert_eq!(
            updated.extra_data,
            Some(json!({
                "gateway_execution_runtime": true,
                "stream_completed": true,
                "upstream_response": {
                    "status_code": 503,
                    "body": {"error": {"message": "original upstream failure"}}
                }
            }))
        );
    }

    #[tokio::test]
    async fn delete_created_before_removes_oldest_matching_rows_up_to_limit() {
        let repository = InMemoryRequestCandidateRepository::seed(vec![
            sample_candidate("cand-1", "req-1", 100),
            sample_candidate("cand-2", "req-2", 200),
            sample_candidate("cand-3", "req-3", 400),
        ]);

        let deleted = repository
            .delete_created_before(350, 1)
            .await
            .expect("delete should succeed");
        assert_eq!(deleted, 1);

        let rows = repository
            .list_recent(10)
            .await
            .expect("list recent should succeed");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "cand-3");
        assert_eq!(rows[1].id, "cand-2");
    }
}
