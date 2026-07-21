use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    score_with_delta, GetPoolMemberScoresByIdsQuery, ListPoolMemberProbeCandidatesQuery,
    ListPoolMemberScoresQuery, ListRankedPoolMembersQuery, PoolMemberHardState, PoolMemberIdentity,
    PoolMemberProbeAttempt, PoolMemberProbeResult, PoolMemberProbeStatus,
    PoolMemberScheduleFeedback, PoolMemberScoreWriteRepository, PoolScoreReadRepository,
    PoolScoreScope, StoredPoolMemberScore, UpsertPoolMemberScore,
};
use crate::repository::pool_scores::merge_score_reason_patch;
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryPoolMemberScoreRepository {
    scores: RwLock<BTreeMap<String, StoredPoolMemberScore>>,
}

impl InMemoryPoolMemberScoreRepository {
    pub fn seed<I>(scores: I) -> Self
    where
        I: IntoIterator<Item = StoredPoolMemberScore>,
    {
        Self {
            scores: RwLock::new(
                scores
                    .into_iter()
                    .map(|score| (score.id.clone(), score))
                    .collect(),
            ),
        }
    }

    fn matches_identity(score: &StoredPoolMemberScore, identity: &PoolMemberIdentity) -> bool {
        score.pool_kind == identity.pool_kind
            && score.pool_id == identity.pool_id
            && score.member_kind == identity.member_kind
            && score.member_id == identity.member_id
    }

    fn matches_scope(score: &StoredPoolMemberScore, scope: Option<&PoolScoreScope>) -> bool {
        let Some(scope) = scope else {
            return true;
        };
        score.capability == scope.capability
            && score.scope_kind == scope.scope_kind
            && score.scope_id == scope.scope_id
    }

    fn sort_ranked(scores: &mut [StoredPoolMemberScore]) {
        scores.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    right
                        .last_ranked_at
                        .unwrap_or(0)
                        .cmp(&left.last_ranked_at.unwrap_or(0))
                })
                .then_with(|| left.member_id.cmp(&right.member_id))
                .then_with(|| left.id.cmp(&right.id))
        });
    }

    fn sort_probe(scores: &mut [StoredPoolMemberScore]) {
        scores.sort_by(|left, right| {
            probe_priority(left)
                .cmp(&probe_priority(right))
                .then_with(|| right.probe_failure_count.cmp(&left.probe_failure_count))
                .then_with(|| {
                    left.last_probe_success_at
                        .unwrap_or(0)
                        .cmp(&right.last_probe_success_at.unwrap_or(0))
                })
                .then_with(|| {
                    left.last_scheduled_at
                        .unwrap_or(0)
                        .cmp(&right.last_scheduled_at.unwrap_or(0))
                        .reverse()
                })
                .then_with(|| left.member_id.cmp(&right.member_id))
        });
    }
}

#[async_trait]
impl PoolScoreReadRepository for InMemoryPoolMemberScoreRepository {
    async fn list_ranked_pool_members(
        &self,
        query: &ListRankedPoolMembersQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let hard_states = query
            .hard_states
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let probe_statuses = query.probe_statuses.as_ref().map(|items| {
            items
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        });
        let mut scores = self
            .scores
            .read()
            .expect("pool member score repository lock")
            .values()
            .filter(|score| {
                score.pool_kind == query.pool_kind
                    && score.pool_id == query.pool_id
                    && score.capability == query.capability
                    && score.scope_kind == query.scope_kind
                    && score.scope_id == query.scope_id
                    && (hard_states.is_empty() || hard_states.contains(&score.hard_state))
                    && probe_statuses
                        .as_ref()
                        .is_none_or(|statuses| statuses.contains(&score.probe_status))
            })
            .cloned()
            .collect::<Vec<_>>();
        Self::sort_ranked(&mut scores);
        Ok(scores
            .into_iter()
            .skip(query.offset)
            .take(query.limit.max(1))
            .collect())
    }

    async fn list_pool_member_scores(
        &self,
        query: &ListPoolMemberScoresQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let hard_states = query
            .hard_states
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let probe_statuses = query.probe_statuses.as_ref().map(|items| {
            items
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        });
        let mut scores = self
            .scores
            .read()
            .expect("pool member score repository lock")
            .values()
            .filter(|score| {
                score.pool_kind == query.pool_kind
                    && score.pool_id == query.pool_id
                    && query
                        .capability
                        .as_ref()
                        .is_none_or(|capability| score.capability == *capability)
                    && query
                        .scope_kind
                        .as_ref()
                        .is_none_or(|scope_kind| score.scope_kind == *scope_kind)
                    && query
                        .scope_id
                        .as_ref()
                        .is_none_or(|scope_id| score.scope_id.as_ref() == Some(scope_id))
                    && (hard_states.is_empty() || hard_states.contains(&score.hard_state))
                    && probe_statuses
                        .as_ref()
                        .is_none_or(|statuses| statuses.contains(&score.probe_status))
            })
            .cloned()
            .collect::<Vec<_>>();
        Self::sort_ranked(&mut scores);
        Ok(scores
            .into_iter()
            .skip(query.offset)
            .take(query.limit.max(1))
            .collect())
    }

    async fn list_pool_member_probe_candidates(
        &self,
        query: &ListPoolMemberProbeCandidatesQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut scores = self
            .scores
            .read()
            .expect("pool member score repository lock")
            .values()
            .filter(|score| {
                score.pool_kind == query.pool_kind
                    && score.pool_id == query.pool_id
                    && query
                        .capability
                        .as_ref()
                        .is_none_or(|capability| score.capability == *capability)
                    && matches!(
                        score.hard_state,
                        PoolMemberHardState::Available
                            | PoolMemberHardState::Unknown
                            | PoolMemberHardState::Cooldown
                            | PoolMemberHardState::QuotaExhausted
                    )
                    && match score.probe_status {
                        PoolMemberProbeStatus::Never
                        | PoolMemberProbeStatus::Failed
                        | PoolMemberProbeStatus::Stale => true,
                        PoolMemberProbeStatus::Ok => score
                            .last_probe_success_at
                            .is_none_or(|ts| ts <= query.stale_before_unix_secs),
                        PoolMemberProbeStatus::InProgress => score
                            .last_probe_attempt_at
                            .is_none_or(|ts| ts <= query.stale_before_unix_secs),
                    }
            })
            .cloned()
            .collect::<Vec<_>>();
        Self::sort_probe(&mut scores);
        Ok(scores.into_iter().take(query.limit.max(1)).collect())
    }

    async fn get_pool_member_scores_by_ids(
        &self,
        query: &GetPoolMemberScoresByIdsQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let ids = query
            .ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let scores = self
            .scores
            .read()
            .expect("pool member score repository lock")
            .values()
            .filter(|score| ids.contains(&score.id))
            .cloned()
            .collect::<Vec<_>>();
        Ok(scores)
    }
}

#[async_trait]
impl PoolMemberScoreWriteRepository for InMemoryPoolMemberScoreRepository {
    async fn upsert_pool_member_score(
        &self,
        score: UpsertPoolMemberScore,
    ) -> Result<StoredPoolMemberScore, DataLayerError> {
        score.validate()?;
        let stored = score.into_stored();
        self.scores
            .write()
            .expect("pool member score repository lock")
            .insert(stored.id.clone(), stored.clone());
        Ok(stored)
    }

    async fn mark_pool_member_probe_in_progress(
        &self,
        attempt: PoolMemberProbeAttempt,
    ) -> Result<usize, DataLayerError> {
        let mut updated = 0;
        let mut guard = self
            .scores
            .write()
            .expect("pool member score repository lock");
        for score in guard.values_mut() {
            if !Self::matches_identity(score, &attempt.identity)
                || !Self::matches_scope(score, attempt.scope.as_ref())
            {
                continue;
            }
            score.last_probe_attempt_at = Some(attempt.attempted_at);
            score.probe_status = PoolMemberProbeStatus::InProgress;
            score.score_reason = merge_score_reason_patch(
                score.score_reason.clone(),
                attempt.score_reason_patch.clone(),
            );
            score.updated_at = attempt.attempted_at;
            updated += 1;
        }
        Ok(updated)
    }

    async fn record_pool_member_probe_result(
        &self,
        result: PoolMemberProbeResult,
    ) -> Result<usize, DataLayerError> {
        let mut updated = 0;
        let mut guard = self
            .scores
            .write()
            .expect("pool member score repository lock");
        for score in guard.values_mut() {
            if !Self::matches_identity(score, &result.identity)
                || !Self::matches_scope(score, result.scope.as_ref())
            {
                continue;
            }
            score.last_probe_attempt_at = Some(result.attempted_at);
            score.probe_status = result.probe_status;
            if result.succeeded {
                score.last_probe_success_at = Some(result.attempted_at);
                score.probe_failure_count = 0;
            } else {
                score.last_probe_failure_at = Some(result.attempted_at);
                score.probe_failure_count = score.probe_failure_count.saturating_add(1);
            }
            if let Some(hard_state) = result.hard_state {
                score.hard_state = hard_state;
            }
            score.score_reason = merge_score_reason_patch(
                score.score_reason.clone(),
                result.score_reason_patch.clone(),
            );
            score.updated_at = result.attempted_at;
            updated += 1;
        }
        Ok(updated)
    }

    async fn record_pool_member_schedule_feedback(
        &self,
        feedback: PoolMemberScheduleFeedback,
    ) -> Result<usize, DataLayerError> {
        let mut updated = 0;
        let mut guard = self
            .scores
            .write()
            .expect("pool member score repository lock");
        for score in guard.values_mut() {
            if !Self::matches_identity(score, &feedback.identity)
                || !Self::matches_scope(score, feedback.scope.as_ref())
            {
                continue;
            }
            score.last_scheduled_at = Some(feedback.scheduled_at);
            match feedback.succeeded {
                Some(true) => {
                    score.last_success_at = Some(feedback.scheduled_at);
                }
                Some(false) => {
                    score.last_failure_at = Some(feedback.scheduled_at);
                    score.failure_count = score.failure_count.saturating_add(1);
                }
                None => {}
            }
            if let Some(hard_state) = feedback.hard_state {
                score.hard_state = hard_state;
            }
            score.score = score_with_delta(score.score, feedback.score_delta);
            score.score_reason = merge_score_reason_patch(
                score.score_reason.clone(),
                feedback.score_reason_patch.clone(),
            );
            score.updated_at = feedback.scheduled_at;
            updated += 1;
        }
        Ok(updated)
    }

    async fn mark_pool_member_hard_state(
        &self,
        identity: &PoolMemberIdentity,
        scope: Option<&PoolScoreScope>,
        hard_state: PoolMemberHardState,
        updated_at: u64,
    ) -> Result<usize, DataLayerError> {
        let mut updated = 0;
        let mut guard = self
            .scores
            .write()
            .expect("pool member score repository lock");
        for score in guard.values_mut() {
            if Self::matches_identity(score, identity) && Self::matches_scope(score, scope) {
                score.hard_state = hard_state;
                score.updated_at = updated_at;
                updated += 1;
            }
        }
        Ok(updated)
    }

    async fn delete_pool_member_scores_for_member(
        &self,
        identity: &PoolMemberIdentity,
    ) -> Result<usize, DataLayerError> {
        let mut guard = self
            .scores
            .write()
            .expect("pool member score repository lock");
        let before = guard.len();
        guard.retain(|_, score| !Self::matches_identity(score, identity));
        Ok(before.saturating_sub(guard.len()))
    }
}

fn probe_priority(score: &StoredPoolMemberScore) -> u8 {
    if score.last_scheduled_at.is_some() && score.probe_status != PoolMemberProbeStatus::Ok {
        return 0;
    }
    match score.hard_state {
        PoolMemberHardState::QuotaExhausted => 1,
        PoolMemberHardState::Unknown => 2,
        _ if score.probe_status == PoolMemberProbeStatus::Stale => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::pool_scores::{
        POOL_KIND_PROVIDER_KEY_POOL, POOL_MEMBER_KIND_PROVIDER_API_KEY,
        POOL_SCORE_CAPABILITY_API_FORMAT, POOL_SCORE_SCOPE_KIND_MODEL,
    };

    fn score(id: &str, member_id: &str, value: f64) -> StoredPoolMemberScore {
        StoredPoolMemberScore {
            id: id.to_string(),
            pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
            pool_id: "provider-1".to_string(),
            member_kind: POOL_MEMBER_KIND_PROVIDER_API_KEY.to_string(),
            member_id: member_id.to_string(),
            capability: POOL_SCORE_CAPABILITY_API_FORMAT.to_string(),
            scope_kind: POOL_SCORE_SCOPE_KIND_MODEL.to_string(),
            scope_id: Some("model-1".to_string()),
            score: value,
            hard_state: PoolMemberHardState::Available,
            score_version: 1,
            score_reason: serde_json::json!({}),
            last_ranked_at: Some(1),
            last_scheduled_at: None,
            last_success_at: None,
            last_failure_at: None,
            failure_count: 0,
            last_probe_attempt_at: None,
            last_probe_success_at: None,
            last_probe_failure_at: None,
            probe_failure_count: 0,
            probe_status: PoolMemberProbeStatus::Never,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn lists_ranked_members_by_score() {
        let repository = InMemoryPoolMemberScoreRepository::seed(vec![
            score("score-1", "key-1", 0.2),
            score("score-2", "key-2", 0.9),
        ]);

        let rows = repository
            .list_ranked_pool_members(&ListRankedPoolMembersQuery {
                pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
                pool_id: "provider-1".to_string(),
                capability: POOL_SCORE_CAPABILITY_API_FORMAT.to_string(),
                scope_kind: POOL_SCORE_SCOPE_KIND_MODEL.to_string(),
                scope_id: Some("model-1".to_string()),
                hard_states: vec![PoolMemberHardState::Available],
                probe_statuses: None,
                offset: 0,
                limit: 10,
            })
            .await
            .expect("list should succeed");

        assert_eq!(
            rows.into_iter()
                .map(|row| row.member_id)
                .collect::<Vec<_>>(),
            vec!["key-2".to_string(), "key-1".to_string()]
        );
    }

    #[tokio::test]
    async fn marks_probe_in_progress_without_incrementing_failure_count() {
        let repository =
            InMemoryPoolMemberScoreRepository::seed(vec![score("score-1", "key-1", 0.2)]);

        let updated = repository
            .mark_pool_member_probe_in_progress(PoolMemberProbeAttempt {
                identity: PoolMemberIdentity::provider_api_key("provider-1", "key-1"),
                scope: None,
                attempted_at: 100,
                score_reason_patch: Some(serde_json::json!({ "last_probe": "in_progress" })),
            })
            .await
            .expect("mark should succeed");

        assert_eq!(updated, 1);
        let rows = repository
            .list_pool_member_scores(&ListPoolMemberScoresQuery {
                pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
                pool_id: "provider-1".to_string(),
                capability: None,
                scope_kind: None,
                scope_id: None,
                hard_states: Vec::new(),
                probe_statuses: Some(vec![PoolMemberProbeStatus::InProgress]),
                offset: 0,
                limit: 10,
            })
            .await
            .expect("list should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].last_probe_attempt_at, Some(100));
        assert_eq!(rows[0].probe_failure_count, 0);
    }
}
