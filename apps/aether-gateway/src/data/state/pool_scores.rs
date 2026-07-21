use super::{
    DataLayerError, GatewayDataState, GetPoolMemberScoresByIdsQuery,
    ListPoolMemberProbeCandidatesQuery, ListPoolMemberScoresQuery, ListRankedPoolMembersQuery,
    PoolMemberHardState, PoolMemberIdentity, PoolMemberProbeAttempt, PoolMemberProbeResult,
    PoolMemberScheduleFeedback, PoolScoreScope, StoredPoolMemberScore, UpsertPoolMemberScore,
};
use aether_data_contracts::repository::pool_scores::PoolMemberScoreUpsertMode;

impl GatewayDataState {
    pub(crate) async fn list_ranked_pool_members(
        &self,
        query: &ListRankedPoolMembersQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        match &self.pool_score_reader {
            Some(repository) => repository.list_ranked_pool_members(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_pool_member_probe_candidates(
        &self,
        query: &ListPoolMemberProbeCandidatesQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        match &self.pool_score_reader {
            Some(repository) => repository.list_pool_member_probe_candidates(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_pool_member_scores(
        &self,
        query: &ListPoolMemberScoresQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        match &self.pool_score_reader {
            Some(repository) => repository.list_pool_member_scores(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn get_pool_member_scores_by_ids(
        &self,
        query: &GetPoolMemberScoresByIdsQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        match &self.pool_score_reader {
            Some(repository) => repository.get_pool_member_scores_by_ids(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn upsert_pool_member_score(
        &self,
        score: UpsertPoolMemberScore,
    ) -> Result<Option<StoredPoolMemberScore>, DataLayerError> {
        match &self.pool_score_writer {
            Some(repository) => repository.upsert_pool_member_score(score).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn upsert_pool_member_score_with_mode(
        &self,
        score: UpsertPoolMemberScore,
        mode: PoolMemberScoreUpsertMode,
    ) -> Result<Option<StoredPoolMemberScore>, DataLayerError> {
        match &self.pool_score_writer {
            Some(repository) => repository
                .upsert_pool_member_score_with_mode(score, mode)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn record_pool_member_probe_result(
        &self,
        result: PoolMemberProbeResult,
    ) -> Result<usize, DataLayerError> {
        match &self.pool_score_writer {
            Some(repository) => repository.record_pool_member_probe_result(result).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn mark_pool_member_probe_in_progress(
        &self,
        attempt: PoolMemberProbeAttempt,
    ) -> Result<usize, DataLayerError> {
        match &self.pool_score_writer {
            Some(repository) => repository.mark_pool_member_probe_in_progress(attempt).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn record_pool_member_schedule_feedback(
        &self,
        feedback: PoolMemberScheduleFeedback,
    ) -> Result<usize, DataLayerError> {
        match &self.pool_score_writer {
            Some(repository) => {
                repository
                    .record_pool_member_schedule_feedback(feedback)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn mark_pool_member_hard_state(
        &self,
        identity: &PoolMemberIdentity,
        scope: Option<&PoolScoreScope>,
        hard_state: PoolMemberHardState,
        updated_at: u64,
    ) -> Result<usize, DataLayerError> {
        match &self.pool_score_writer {
            Some(repository) => {
                repository
                    .mark_pool_member_hard_state(identity, scope, hard_state, updated_at)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn delete_pool_member_scores_for_member(
        &self,
        identity: &PoolMemberIdentity,
    ) -> Result<usize, DataLayerError> {
        match &self.pool_score_writer {
            Some(repository) => {
                repository
                    .delete_pool_member_scores_for_member(identity)
                    .await
            }
            None => Ok(0),
        }
    }
}
