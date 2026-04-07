use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    PublicHealthStatusCount, PublicHealthTimelineBucket, RequestCandidateReadRepository,
    RequestCandidateStatus, RequestCandidateWriteRepository, StoredRequestCandidate,
    UpsertRequestCandidateRecord,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryRequestCandidateRepository {
    by_id: RwLock<BTreeMap<String, StoredRequestCandidate>>,
}

impl InMemoryRequestCandidateRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredRequestCandidate>,
    {
        let mut by_id = BTreeMap::new();
        for item in items {
            by_id.insert(item.id.clone(), item);
        }
        Self {
            by_id: RwLock::new(by_id),
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
            .by_id
            .read()
            .expect("request candidate repository lock")
            .values()
            .filter(|row| row.request_id == request_id)
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.candidate_index
                .cmp(&right.candidate_index)
                .then(left.retry_index.cmp(&right.retry_index))
                .then(left.created_at_unix_secs.cmp(&right.created_at_unix_secs))
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

        let mut rows = self
            .by_id
            .read()
            .expect("request candidate repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.created_at_unix_secs.cmp(&left.created_at_unix_secs));
        rows.truncate(limit);
        Ok(rows)
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
            .by_id
            .read()
            .expect("request candidate repository lock")
            .values()
            .filter(|row| row.provider_id.as_deref() == Some(provider_id))
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.created_at_unix_secs.cmp(&left.created_at_unix_secs));
        rows.truncate(limit);
        Ok(rows)
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
            .by_id
            .read()
            .expect("request candidate repository lock")
            .values()
            .filter(|row| {
                row.endpoint_id
                    .as_ref()
                    .is_some_and(|endpoint_id| endpoint_ids.contains(endpoint_id))
                    && row.created_at_unix_secs >= since_unix_secs
                    && matches!(
                        row.status,
                        RequestCandidateStatus::Success
                            | RequestCandidateStatus::Failed
                            | RequestCandidateStatus::Skipped
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| right.created_at_unix_secs.cmp(&left.created_at_unix_secs));
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
            .by_id
            .read()
            .expect("request candidate repository lock")
            .values()
        {
            let Some(endpoint_id) = row.endpoint_id.as_ref() else {
                continue;
            };
            if !endpoint_ids.contains(endpoint_id) || row.created_at_unix_secs < since_unix_secs {
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
        let span = until_unix_secs.saturating_sub(since_unix_secs);
        let mut buckets = BTreeMap::<(String, u32), PublicHealthTimelineBucket>::new();

        for row in self
            .by_id
            .read()
            .expect("request candidate repository lock")
            .values()
        {
            let Some(endpoint_id) = row.endpoint_id.as_ref() else {
                continue;
            };
            if !endpoint_ids.contains(endpoint_id)
                || row.created_at_unix_secs < since_unix_secs
                || row.created_at_unix_secs > until_unix_secs
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

            let segment_idx = if span == 0 {
                0
            } else {
                let offset = row.created_at_unix_secs.saturating_sub(since_unix_secs);
                let idx = ((offset as u128) * (segments as u128) / (span as u128)) as u32;
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
                    min_created_at_unix_secs: None,
                    max_created_at_unix_secs: None,
                });
            bucket.total_count += 1;
            if row.status == RequestCandidateStatus::Success {
                bucket.success_count += 1;
            } else if row.status == RequestCandidateStatus::Failed {
                bucket.failed_count += 1;
            }
            bucket.min_created_at_unix_secs = Some(
                bucket
                    .min_created_at_unix_secs
                    .map(|value| value.min(row.created_at_unix_secs))
                    .unwrap_or(row.created_at_unix_secs),
            );
            bucket.max_created_at_unix_secs = Some(
                bucket
                    .max_created_at_unix_secs
                    .map(|value| value.max(row.created_at_unix_secs))
                    .unwrap_or(row.created_at_unix_secs),
            );
        }

        Ok(buckets.into_values().collect())
    }
}

#[async_trait]
impl RequestCandidateWriteRepository for InMemoryRequestCandidateRepository {
    async fn upsert(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<StoredRequestCandidate, DataLayerError> {
        candidate.validate()?;

        let mut by_id = self
            .by_id
            .write()
            .expect("request candidate repository lock");
        let existing = by_id
            .values()
            .find(|row| {
                row.request_id == candidate.request_id
                    && row.candidate_index == candidate.candidate_index
                    && row.retry_index == candidate.retry_index
            })
            .cloned();

        let created_at_unix_secs = existing
            .as_ref()
            .map(|row| row.created_at_unix_secs)
            .or(candidate.created_at_unix_secs)
            .or(candidate.started_at_unix_secs)
            .or(candidate.finished_at_unix_secs)
            .unwrap_or_default();

        let stored = StoredRequestCandidate {
            id: existing
                .as_ref()
                .map(|row| row.id.clone())
                .unwrap_or_else(|| candidate.id.clone()),
            request_id: candidate.request_id.clone(),
            user_id: candidate
                .user_id
                .or_else(|| existing.as_ref().and_then(|row| row.user_id.clone())),
            api_key_id: candidate
                .api_key_id
                .or_else(|| existing.as_ref().and_then(|row| row.api_key_id.clone())),
            username: candidate
                .username
                .or_else(|| existing.as_ref().and_then(|row| row.username.clone())),
            api_key_name: candidate
                .api_key_name
                .or_else(|| existing.as_ref().and_then(|row| row.api_key_name.clone())),
            candidate_index: candidate.candidate_index,
            retry_index: candidate.retry_index,
            provider_id: candidate
                .provider_id
                .or_else(|| existing.as_ref().and_then(|row| row.provider_id.clone())),
            endpoint_id: candidate
                .endpoint_id
                .or_else(|| existing.as_ref().and_then(|row| row.endpoint_id.clone())),
            key_id: candidate
                .key_id
                .or_else(|| existing.as_ref().and_then(|row| row.key_id.clone())),
            status: candidate.status,
            skip_reason: candidate
                .skip_reason
                .or_else(|| existing.as_ref().and_then(|row| row.skip_reason.clone())),
            is_cached: candidate
                .is_cached
                .unwrap_or_else(|| existing.as_ref().map(|row| row.is_cached).unwrap_or(false)),
            status_code: candidate
                .status_code
                .or_else(|| existing.as_ref().and_then(|row| row.status_code)),
            error_type: candidate
                .error_type
                .or_else(|| existing.as_ref().and_then(|row| row.error_type.clone())),
            error_message: candidate
                .error_message
                .or_else(|| existing.as_ref().and_then(|row| row.error_message.clone())),
            latency_ms: candidate
                .latency_ms
                .or_else(|| existing.as_ref().and_then(|row| row.latency_ms)),
            concurrent_requests: candidate
                .concurrent_requests
                .or_else(|| existing.as_ref().and_then(|row| row.concurrent_requests)),
            extra_data: candidate
                .extra_data
                .or_else(|| existing.as_ref().and_then(|row| row.extra_data.clone())),
            required_capabilities: candidate.required_capabilities.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|row| row.required_capabilities.clone())
            }),
            created_at_unix_secs,
            started_at_unix_secs: candidate
                .started_at_unix_secs
                .or_else(|| existing.as_ref().and_then(|row| row.started_at_unix_secs)),
            finished_at_unix_secs: candidate
                .finished_at_unix_secs
                .or_else(|| existing.as_ref().and_then(|row| row.finished_at_unix_secs)),
        };

        by_id.insert(stored.id.clone(), stored.clone());
        Ok(stored)
    }

    async fn delete_created_before(
        &self,
        created_before_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        if limit == 0 {
            return Ok(0);
        }

        let mut by_id = self
            .by_id
            .write()
            .expect("request candidate repository lock");
        let mut ids = by_id
            .values()
            .filter(|row| row.created_at_unix_secs < created_before_unix_secs)
            .map(|row| (row.created_at_unix_secs, row.id.clone()))
            .collect::<Vec<_>>();
        ids.sort();

        let mut deleted = 0usize;
        for (_, id) in ids.into_iter().take(limit) {
            if by_id.remove(&id).is_some() {
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

    fn sample_candidate(
        id: &str,
        request_id: &str,
        created_at_unix_secs: i64,
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
            created_at_unix_secs,
            Some(created_at_unix_secs),
            Some(created_at_unix_secs + 1),
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
    async fn aggregates_finalized_health_data_by_endpoint_ids() {
        let repository = InMemoryRequestCandidateRepository::seed(vec![
            sample_candidate("cand-1", "req-1", 100),
            sample_candidate("cand-2", "req-2", 200),
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
                extra_data: None,
                required_capabilities: None,
                created_at_unix_secs: Some(100),
                started_at_unix_secs: None,
                finished_at_unix_secs: None,
            })
            .await
            .expect("create should succeed");
        assert_eq!(created.id, "cand-1");
        assert_eq!(created.status, RequestCandidateStatus::Available);

        let updated = repository
            .upsert(UpsertRequestCandidateRecord {
                id: "cand-1-replacement".to_string(),
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
                latency_ms: Some(25),
                concurrent_requests: Some(2),
                extra_data: None,
                required_capabilities: None,
                created_at_unix_secs: None,
                started_at_unix_secs: Some(101),
                finished_at_unix_secs: Some(102),
            })
            .await
            .expect("update should succeed");
        assert_eq!(updated.id, "cand-1");
        assert_eq!(updated.status, RequestCandidateStatus::Success);
        assert_eq!(updated.status_code, Some(200));
        assert_eq!(updated.latency_ms, Some(25));
        assert_eq!(updated.started_at_unix_secs, Some(101));
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
