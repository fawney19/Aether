use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use aether_data_contracts::repository::pool_scores::*;

use crate::error::SqlResultExt;
use crate::{DataLayerError, MysqlPool};

const SCORE_COLUMNS: &str = r#"
SELECT
  id,
  pool_kind,
  pool_id,
  member_kind,
  member_id,
  capability,
  scope_kind,
  scope_id,
  score,
  hard_state,
  score_version,
  score_reason,
  last_ranked_at,
  last_scheduled_at,
  last_success_at,
  last_failure_at,
  failure_count,
  last_probe_attempt_at,
  last_probe_success_at,
  last_probe_failure_at,
  probe_failure_count,
  probe_status,
  updated_at
FROM pool_member_scores
"#;

const UPSERT_PRESERVING_NULLABLE_TIMESTAMPS_SQL: &str = r#"
INSERT INTO pool_member_scores (
  id, pool_kind, pool_id, member_kind, member_id, capability, scope_kind, scope_id,
  score, hard_state, score_version, score_reason, last_ranked_at, last_scheduled_at,
  last_success_at, last_failure_at, failure_count, last_probe_attempt_at,
  last_probe_success_at, last_probe_failure_at, probe_failure_count, probe_status, updated_at
) VALUES (
  ?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?
)
ON DUPLICATE KEY UPDATE
  pool_kind = VALUES(pool_kind),
  pool_id = VALUES(pool_id),
  member_kind = VALUES(member_kind),
  member_id = VALUES(member_id),
  capability = VALUES(capability),
  scope_kind = VALUES(scope_kind),
  scope_id = VALUES(scope_id),
  score = VALUES(score),
  hard_state = VALUES(hard_state),
  score_version = VALUES(score_version),
  score_reason = VALUES(score_reason),
  last_ranked_at = VALUES(last_ranked_at),
  last_scheduled_at = COALESCE(VALUES(last_scheduled_at), last_scheduled_at),
  last_success_at = COALESCE(VALUES(last_success_at), last_success_at),
  last_failure_at = COALESCE(VALUES(last_failure_at), last_failure_at),
  failure_count = VALUES(failure_count),
  last_probe_attempt_at = COALESCE(VALUES(last_probe_attempt_at), last_probe_attempt_at),
  last_probe_success_at = COALESCE(VALUES(last_probe_success_at), last_probe_success_at),
  last_probe_failure_at = COALESCE(VALUES(last_probe_failure_at), last_probe_failure_at),
  probe_failure_count = VALUES(probe_failure_count),
  probe_status = VALUES(probe_status),
  updated_at = VALUES(updated_at)
"#;

const UPSERT_OAUTH_RECOVERY_SQL: &str = r#"
INSERT INTO pool_member_scores (
  id, pool_kind, pool_id, member_kind, member_id, capability, scope_kind, scope_id,
  score, hard_state, score_version, score_reason, last_ranked_at, last_scheduled_at,
  last_success_at, last_failure_at, failure_count, last_probe_attempt_at,
  last_probe_success_at, last_probe_failure_at, probe_failure_count, probe_status, updated_at
) VALUES (
  ?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?
)
ON DUPLICATE KEY UPDATE
  pool_kind = VALUES(pool_kind),
  pool_id = VALUES(pool_id),
  member_kind = VALUES(member_kind),
  member_id = VALUES(member_id),
  capability = VALUES(capability),
  scope_kind = VALUES(scope_kind),
  scope_id = VALUES(scope_id),
  score = IF(updated_at <= VALUES(updated_at), VALUES(score), score),
  hard_state = IF(updated_at <= VALUES(updated_at), VALUES(hard_state), hard_state),
  score_version = IF(updated_at <= VALUES(updated_at), VALUES(score_version), score_version),
  score_reason = IF(updated_at <= VALUES(updated_at), VALUES(score_reason), score_reason),
  last_ranked_at = IF(updated_at <= VALUES(updated_at), VALUES(last_ranked_at), last_ranked_at),
  failure_count = IF(
    last_failure_at IS NULL OR last_failure_at <= VALUES(updated_at),
    VALUES(failure_count), failure_count),
  last_failure_at = IF(
    last_failure_at IS NULL OR last_failure_at <= VALUES(updated_at),
    VALUES(last_failure_at), last_failure_at),
  probe_status = IF(
    (last_probe_attempt_at IS NOT NULL AND last_probe_attempt_at > VALUES(updated_at))
      OR (last_probe_success_at IS NOT NULL AND last_probe_success_at > VALUES(updated_at))
      OR (last_probe_failure_at IS NOT NULL AND last_probe_failure_at > VALUES(updated_at)),
    probe_status, VALUES(probe_status)),
  probe_failure_count = IF(
    last_probe_failure_at IS NULL OR last_probe_failure_at <= VALUES(updated_at),
    VALUES(probe_failure_count), probe_failure_count),
  last_probe_failure_at = IF(
    last_probe_failure_at IS NULL OR last_probe_failure_at <= VALUES(updated_at),
    VALUES(last_probe_failure_at), last_probe_failure_at),
  updated_at = GREATEST(updated_at, VALUES(updated_at))
"#;

fn pool_member_score_upsert_sql(mode: PoolMemberScoreUpsertMode) -> &'static str {
    match mode {
        PoolMemberScoreUpsertMode::PreserveExistingNullableTimestamps => {
            UPSERT_PRESERVING_NULLABLE_TIMESTAMPS_SQL
        }
        PoolMemberScoreUpsertMode::OAuthRecovery => UPSERT_OAUTH_RECOVERY_SQL,
    }
}

#[derive(Debug, Clone)]
pub struct MysqlPoolMemberScoreRepository {
    pool: MysqlPool,
}

impl MysqlPoolMemberScoreRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn find_scores_by_identity(
        &self,
        identity: &PoolMemberIdentity,
        scope: Option<&PoolScoreScope>,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(SCORE_COLUMNS);
        builder
            .push(" WHERE pool_kind = ")
            .push_bind(identity.pool_kind.clone())
            .push(" AND pool_id = ")
            .push_bind(identity.pool_id.clone())
            .push(" AND member_kind = ")
            .push_bind(identity.member_kind.clone())
            .push(" AND member_id = ")
            .push_bind(identity.member_id.clone());
        if let Some(scope) = scope {
            builder
                .push(" AND capability = ")
                .push_bind(scope.capability.clone())
                .push(" AND scope_kind = ")
                .push_bind(scope.scope_kind.clone());
            if let Some(scope_id) = &scope.scope_id {
                builder.push(" AND scope_id = ").push_bind(scope_id.clone());
            } else {
                builder.push(" AND scope_id IS NULL");
            }
        }
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }
}

#[async_trait]
impl PoolScoreReadRepository for MysqlPoolMemberScoreRepository {
    async fn list_ranked_pool_members(
        &self,
        query: &ListRankedPoolMembersQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(SCORE_COLUMNS);
        builder
            .push(" WHERE pool_kind = ")
            .push_bind(query.pool_kind.clone())
            .push(" AND pool_id = ")
            .push_bind(query.pool_id.clone())
            .push(" AND capability = ")
            .push_bind(query.capability.clone())
            .push(" AND scope_kind = ")
            .push_bind(query.scope_kind.clone());
        if let Some(scope_id) = &query.scope_id {
            builder.push(" AND scope_id = ").push_bind(scope_id.clone());
        } else {
            builder.push(" AND scope_id IS NULL");
        }
        if !query.hard_states.is_empty() {
            builder.push(" AND hard_state IN (");
            let mut separated = builder.separated(", ");
            for state in &query.hard_states {
                separated.push_bind(state.as_database());
            }
            separated.push_unseparated(")");
        }
        if let Some(statuses) = &query.probe_statuses {
            if !statuses.is_empty() {
                builder.push(" AND probe_status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.as_database());
                }
                separated.push_unseparated(")");
            }
        }
        builder
            .push(" ORDER BY score DESC, last_ranked_at DESC, member_id ASC, id ASC")
            .push(" LIMIT ")
            .push_bind(i64_from_usize(query.limit.max(1), "pool score limit")?)
            .push(" OFFSET ")
            .push_bind(i64_from_usize(query.offset, "pool score offset")?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn list_pool_member_scores(
        &self,
        query: &ListPoolMemberScoresQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(SCORE_COLUMNS);
        builder
            .push(" WHERE pool_kind = ")
            .push_bind(query.pool_kind.clone())
            .push(" AND pool_id = ")
            .push_bind(query.pool_id.clone());
        if let Some(capability) = &query.capability {
            builder
                .push(" AND capability = ")
                .push_bind(capability.clone());
        }
        if let Some(scope_kind) = &query.scope_kind {
            builder
                .push(" AND scope_kind = ")
                .push_bind(scope_kind.clone());
        }
        if let Some(scope_id) = &query.scope_id {
            builder.push(" AND scope_id = ").push_bind(scope_id.clone());
        }
        if !query.hard_states.is_empty() {
            builder.push(" AND hard_state IN (");
            let mut separated = builder.separated(", ");
            for state in &query.hard_states {
                separated.push_bind(state.as_database());
            }
            separated.push_unseparated(")");
        }
        if let Some(statuses) = &query.probe_statuses {
            if !statuses.is_empty() {
                builder.push(" AND probe_status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.as_database());
                }
                separated.push_unseparated(")");
            }
        }
        builder
            .push(" ORDER BY score DESC, last_ranked_at DESC, member_id ASC, id ASC")
            .push(" LIMIT ")
            .push_bind(i64_from_usize(query.limit.max(1), "pool score limit")?)
            .push(" OFFSET ")
            .push_bind(i64_from_usize(query.offset, "pool score offset")?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn list_pool_member_probe_candidates(
        &self,
        query: &ListPoolMemberProbeCandidatesQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(SCORE_COLUMNS);
        builder
            .push(" WHERE pool_kind = ")
            .push_bind(query.pool_kind.clone())
            .push(" AND pool_id = ")
            .push_bind(query.pool_id.clone());
        if let Some(capability) = &query.capability {
            builder
                .push(" AND capability = ")
                .push_bind(capability.clone());
        }
        builder
            .push(" AND hard_state IN ('available','unknown','cooldown','quota_exhausted')")
            .push(" AND (probe_status IN ('never','failed','stale')")
            .push(" OR (probe_status = 'ok' AND (last_probe_success_at IS NULL OR last_probe_success_at <= ")
            .push_bind(i64_from_u64(
                query.stale_before_unix_secs,
                "pool probe stale_before_unix_secs",
            )?)
            .push("))")
            .push(" OR (probe_status = 'in_progress' AND (last_probe_attempt_at IS NULL OR last_probe_attempt_at <= ")
            .push_bind(i64_from_u64(
                query.stale_before_unix_secs,
                "pool probe stale_before_unix_secs",
            )?)
            .push(")))")
            .push(
                r#"
 ORDER BY
   CASE
     WHEN last_scheduled_at IS NOT NULL AND probe_status <> 'ok' THEN 0
     WHEN hard_state = 'quota_exhausted' THEN 1
     WHEN hard_state = 'unknown' THEN 2
     WHEN probe_status = 'stale' THEN 3
     ELSE 4
   END ASC,
   probe_failure_count DESC,
   COALESCE(last_probe_success_at, 0) ASC,
   COALESCE(last_scheduled_at, 0) DESC,
   member_id ASC
"#,
            )
            .push(" LIMIT ")
            .push_bind(i64_from_usize(
                query.limit.max(1),
                "pool probe candidate limit",
            )?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn get_pool_member_scores_by_ids(
        &self,
        query: &GetPoolMemberScoresByIdsQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        if query.ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(SCORE_COLUMNS);
        builder.push(" WHERE id IN (");
        let mut separated = builder.separated(", ");
        for id in &query.ids {
            separated.push_bind(id.clone());
        }
        separated.push_unseparated(")");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }
}

#[async_trait]
impl PoolMemberScoreWriteRepository for MysqlPoolMemberScoreRepository {
    async fn upsert_pool_member_score_with_mode(
        &self,
        score: UpsertPoolMemberScore,
        mode: PoolMemberScoreUpsertMode,
    ) -> Result<StoredPoolMemberScore, DataLayerError> {
        score.validate()?;
        let stored = score.into_stored();
        let score_reason = serde_json::to_string(&stored.score_reason)
            .map_err(|err| DataLayerError::InvalidInput(err.to_string()))?;
        sqlx::query(pool_member_score_upsert_sql(mode))
            .bind(stored.id.as_str())
            .bind(stored.pool_kind.as_str())
            .bind(stored.pool_id.as_str())
            .bind(stored.member_kind.as_str())
            .bind(stored.member_id.as_str())
            .bind(stored.capability.as_str())
            .bind(stored.scope_kind.as_str())
            .bind(stored.scope_id.as_deref())
            .bind(stored.score)
            .bind(stored.hard_state.as_database())
            .bind(i64_from_u64(stored.score_version, "pool score version")?)
            .bind(score_reason)
            .bind(i64_opt_from_u64(
                stored.last_ranked_at,
                "pool score last_ranked_at",
            )?)
            .bind(i64_opt_from_u64(
                stored.last_scheduled_at,
                "pool score last_scheduled_at",
            )?)
            .bind(i64_opt_from_u64(
                stored.last_success_at,
                "pool score last_success_at",
            )?)
            .bind(i64_opt_from_u64(
                stored.last_failure_at,
                "pool score last_failure_at",
            )?)
            .bind(i64_from_u64(
                stored.failure_count,
                "pool score failure_count",
            )?)
            .bind(i64_opt_from_u64(
                stored.last_probe_attempt_at,
                "pool score last_probe_attempt_at",
            )?)
            .bind(i64_opt_from_u64(
                stored.last_probe_success_at,
                "pool score last_probe_success_at",
            )?)
            .bind(i64_opt_from_u64(
                stored.last_probe_failure_at,
                "pool score last_probe_failure_at",
            )?)
            .bind(i64_from_u64(
                stored.probe_failure_count,
                "pool score probe_failure_count",
            )?)
            .bind(stored.probe_status.as_database())
            .bind(i64_from_u64(stored.updated_at, "pool score updated_at")?)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        Ok(stored)
    }

    async fn mark_pool_member_probe_in_progress(
        &self,
        attempt: PoolMemberProbeAttempt,
    ) -> Result<usize, DataLayerError> {
        let rows = self
            .find_scores_by_identity(&attempt.identity, attempt.scope.as_ref())
            .await?;
        let count = rows.len();
        for mut row in rows {
            row.last_probe_attempt_at = Some(attempt.attempted_at);
            row.probe_status = PoolMemberProbeStatus::InProgress;
            row.score_reason =
                merge_score_reason_patch(row.score_reason, attempt.score_reason_patch.clone());
            row.updated_at = attempt.attempted_at;
            self.upsert_pool_member_score(upsert_from_stored(row))
                .await?;
        }
        Ok(count)
    }

    async fn record_pool_member_probe_result(
        &self,
        result: PoolMemberProbeResult,
    ) -> Result<usize, DataLayerError> {
        let rows = self
            .find_scores_by_identity(&result.identity, result.scope.as_ref())
            .await?;
        let count = rows.len();
        for mut row in rows {
            row.last_probe_attempt_at = Some(result.attempted_at);
            row.probe_status = result.probe_status;
            if result.succeeded {
                row.last_probe_success_at = Some(result.attempted_at);
                row.probe_failure_count = 0;
            } else {
                row.last_probe_failure_at = Some(result.attempted_at);
                row.probe_failure_count = row.probe_failure_count.saturating_add(1);
            }
            if let Some(hard_state) = result.hard_state {
                row.hard_state = hard_state;
            }
            row.score_reason =
                merge_score_reason_patch(row.score_reason, result.score_reason_patch.clone());
            row.updated_at = result.attempted_at;
            self.upsert_pool_member_score(upsert_from_stored(row))
                .await?;
        }
        Ok(count)
    }

    async fn record_pool_member_schedule_feedback(
        &self,
        feedback: PoolMemberScheduleFeedback,
    ) -> Result<usize, DataLayerError> {
        let rows = self
            .find_scores_by_identity(&feedback.identity, feedback.scope.as_ref())
            .await?;
        let count = rows.len();
        for mut row in rows {
            row.last_scheduled_at = Some(feedback.scheduled_at);
            match feedback.succeeded {
                Some(true) => row.last_success_at = Some(feedback.scheduled_at),
                Some(false) => {
                    row.last_failure_at = Some(feedback.scheduled_at);
                    row.failure_count = row.failure_count.saturating_add(1);
                }
                None => {}
            }
            if let Some(hard_state) = feedback.hard_state {
                row.hard_state = hard_state;
            }
            row.score = score_with_delta(row.score, feedback.score_delta);
            row.score_reason =
                merge_score_reason_patch(row.score_reason, feedback.score_reason_patch.clone());
            row.updated_at = feedback.scheduled_at;
            self.upsert_pool_member_score(upsert_from_stored(row))
                .await?;
        }
        Ok(count)
    }

    async fn mark_pool_member_hard_state(
        &self,
        identity: &PoolMemberIdentity,
        scope: Option<&PoolScoreScope>,
        hard_state: PoolMemberHardState,
        updated_at: u64,
    ) -> Result<usize, DataLayerError> {
        let rows = self.find_scores_by_identity(identity, scope).await?;
        let count = rows.len();
        for mut row in rows {
            row.hard_state = hard_state;
            row.updated_at = updated_at;
            self.upsert_pool_member_score(upsert_from_stored(row))
                .await?;
        }
        Ok(count)
    }

    async fn delete_pool_member_scores_for_member(
        &self,
        identity: &PoolMemberIdentity,
    ) -> Result<usize, DataLayerError> {
        let result = sqlx::query(
            r#"
DELETE FROM pool_member_scores
WHERE pool_kind = ? AND pool_id = ? AND member_kind = ? AND member_id = ?
"#,
        )
        .bind(identity.pool_kind.as_str())
        .bind(identity.pool_id.as_str())
        .bind(identity.member_kind.as_str())
        .bind(identity.member_id.as_str())
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() as usize)
    }
}

fn map_score_row(row: &MySqlRow) -> Result<StoredPoolMemberScore, DataLayerError> {
    let score_reason_raw: String = row.try_get("score_reason").map_sql_err()?;
    Ok(StoredPoolMemberScore {
        id: row.try_get("id").map_sql_err()?,
        pool_kind: row.try_get("pool_kind").map_sql_err()?,
        pool_id: row.try_get("pool_id").map_sql_err()?,
        member_kind: row.try_get("member_kind").map_sql_err()?,
        member_id: row.try_get("member_id").map_sql_err()?,
        capability: row.try_get("capability").map_sql_err()?,
        scope_kind: row.try_get("scope_kind").map_sql_err()?,
        scope_id: row.try_get("scope_id").map_sql_err()?,
        score: row.try_get("score").map_sql_err()?,
        hard_state: PoolMemberHardState::from_database(
            row.try_get::<String, _>("hard_state")
                .map_sql_err()?
                .as_str(),
        )?,
        score_version: u64_from_i64(
            row.try_get("score_version").map_sql_err()?,
            "pool_member_scores.score_version",
        )?,
        score_reason: serde_json::from_str(&score_reason_raw).unwrap_or(serde_json::Value::Null),
        last_ranked_at: u64_opt_from_i64(
            row.try_get("last_ranked_at").map_sql_err()?,
            "pool_member_scores.last_ranked_at",
        )?,
        last_scheduled_at: u64_opt_from_i64(
            row.try_get("last_scheduled_at").map_sql_err()?,
            "pool_member_scores.last_scheduled_at",
        )?,
        last_success_at: u64_opt_from_i64(
            row.try_get("last_success_at").map_sql_err()?,
            "pool_member_scores.last_success_at",
        )?,
        last_failure_at: u64_opt_from_i64(
            row.try_get("last_failure_at").map_sql_err()?,
            "pool_member_scores.last_failure_at",
        )?,
        failure_count: u64_from_i64(
            row.try_get("failure_count").map_sql_err()?,
            "pool_member_scores.failure_count",
        )?,
        last_probe_attempt_at: u64_opt_from_i64(
            row.try_get("last_probe_attempt_at").map_sql_err()?,
            "pool_member_scores.last_probe_attempt_at",
        )?,
        last_probe_success_at: u64_opt_from_i64(
            row.try_get("last_probe_success_at").map_sql_err()?,
            "pool_member_scores.last_probe_success_at",
        )?,
        last_probe_failure_at: u64_opt_from_i64(
            row.try_get("last_probe_failure_at").map_sql_err()?,
            "pool_member_scores.last_probe_failure_at",
        )?,
        probe_failure_count: u64_from_i64(
            row.try_get("probe_failure_count").map_sql_err()?,
            "pool_member_scores.probe_failure_count",
        )?,
        probe_status: PoolMemberProbeStatus::from_database(
            row.try_get::<String, _>("probe_status")
                .map_sql_err()?
                .as_str(),
        )?,
        updated_at: u64_from_i64(
            row.try_get("updated_at").map_sql_err()?,
            "pool_member_scores.updated_at",
        )?,
    })
}

fn upsert_from_stored(score: StoredPoolMemberScore) -> UpsertPoolMemberScore {
    UpsertPoolMemberScore {
        id: score.id,
        identity: PoolMemberIdentity {
            pool_kind: score.pool_kind,
            pool_id: score.pool_id,
            member_kind: score.member_kind,
            member_id: score.member_id,
        },
        scope: PoolScoreScope {
            capability: score.capability,
            scope_kind: score.scope_kind,
            scope_id: score.scope_id,
        },
        score: score.score,
        hard_state: score.hard_state,
        score_version: score.score_version,
        score_reason: score.score_reason,
        last_ranked_at: score.last_ranked_at,
        last_scheduled_at: score.last_scheduled_at,
        last_success_at: score.last_success_at,
        last_failure_at: score.last_failure_at,
        failure_count: score.failure_count,
        last_probe_attempt_at: score.last_probe_attempt_at,
        last_probe_success_at: score.last_probe_success_at,
        last_probe_failure_at: score.last_probe_failure_at,
        probe_failure_count: score.probe_failure_count,
        probe_status: score.probe_status,
        updated_at: score.updated_at,
    }
}

fn i64_from_usize(value: usize, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field} exceeds signed 64-bit range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mysql_upsert_mode_selects_nullable_timestamp_semantics() {
        let preserving = pool_member_score_upsert_sql(
            PoolMemberScoreUpsertMode::PreserveExistingNullableTimestamps,
        );
        let recovering = pool_member_score_upsert_sql(PoolMemberScoreUpsertMode::OAuthRecovery);

        for field in [
            "last_scheduled_at",
            "last_success_at",
            "last_failure_at",
            "last_probe_attempt_at",
            "last_probe_success_at",
            "last_probe_failure_at",
        ] {
            assert!(preserving.contains(&format!("{field} = COALESCE(VALUES({field}), {field})")));
        }
        for field in [
            "last_scheduled_at",
            "last_success_at",
            "last_probe_attempt_at",
            "last_probe_success_at",
        ] {
            assert!(!recovering.contains(&format!("\n  {field} =")));
        }
        for field in [
            "score",
            "hard_state",
            "score_version",
            "score_reason",
            "last_ranked_at",
        ] {
            assert!(recovering.contains(&format!(
                "{field} = IF(updated_at <= VALUES(updated_at), VALUES({field}), {field})"
            )));
        }
        assert!(
            recovering.contains("last_failure_at IS NULL OR last_failure_at <= VALUES(updated_at)")
        );
        assert!(recovering.contains("failure_count = IF("));
        assert!(recovering.contains(
            "last_probe_failure_at IS NULL OR last_probe_failure_at <= VALUES(updated_at)"
        ));
        assert!(recovering.contains("probe_failure_count = IF("));
        assert!(recovering.contains("updated_at = GREATEST(updated_at, VALUES(updated_at))"));
        assert!(
            recovering.find("failure_count =").unwrap()
                < recovering.find("last_failure_at =").unwrap()
        );
        assert!(
            recovering.find("probe_status =").unwrap()
                < recovering.find("last_probe_failure_at =").unwrap()
        );
        assert_eq!(preserving.matches('?').count(), 23);
        assert_eq!(recovering.matches('?').count(), 23);
    }
}
