use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use aether_data_contracts::repository::pool_scores::*;
use aether_data_query::{push_eq, push_in, push_limit, push_limit_offset, WhereClause};

use crate::error::SqlResultExt;
use crate::{DataLayerError, SqlitePool};

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
ON CONFLICT(id) DO UPDATE SET
  pool_kind = excluded.pool_kind,
  pool_id = excluded.pool_id,
  member_kind = excluded.member_kind,
  member_id = excluded.member_id,
  capability = excluded.capability,
  scope_kind = excluded.scope_kind,
  scope_id = excluded.scope_id,
  score = excluded.score,
  hard_state = excluded.hard_state,
  score_version = excluded.score_version,
  score_reason = excluded.score_reason,
  last_ranked_at = excluded.last_ranked_at,
  last_scheduled_at = COALESCE(excluded.last_scheduled_at, pool_member_scores.last_scheduled_at),
  last_success_at = COALESCE(excluded.last_success_at, pool_member_scores.last_success_at),
  last_failure_at = COALESCE(excluded.last_failure_at, pool_member_scores.last_failure_at),
  failure_count = excluded.failure_count,
  last_probe_attempt_at = COALESCE(excluded.last_probe_attempt_at, pool_member_scores.last_probe_attempt_at),
  last_probe_success_at = COALESCE(excluded.last_probe_success_at, pool_member_scores.last_probe_success_at),
  last_probe_failure_at = COALESCE(excluded.last_probe_failure_at, pool_member_scores.last_probe_failure_at),
  probe_failure_count = excluded.probe_failure_count,
  probe_status = excluded.probe_status,
  updated_at = excluded.updated_at
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
ON CONFLICT(id) DO UPDATE SET
  pool_kind = excluded.pool_kind,
  pool_id = excluded.pool_id,
  member_kind = excluded.member_kind,
  member_id = excluded.member_id,
  capability = excluded.capability,
  scope_kind = excluded.scope_kind,
  scope_id = excluded.scope_id,
  score = CASE WHEN pool_member_scores.updated_at <= excluded.updated_at THEN excluded.score ELSE pool_member_scores.score END,
  hard_state = CASE WHEN pool_member_scores.updated_at <= excluded.updated_at THEN excluded.hard_state ELSE pool_member_scores.hard_state END,
  score_version = CASE WHEN pool_member_scores.updated_at <= excluded.updated_at THEN excluded.score_version ELSE pool_member_scores.score_version END,
  score_reason = CASE WHEN pool_member_scores.updated_at <= excluded.updated_at THEN excluded.score_reason ELSE pool_member_scores.score_reason END,
  last_ranked_at = CASE WHEN pool_member_scores.updated_at <= excluded.updated_at THEN excluded.last_ranked_at ELSE pool_member_scores.last_ranked_at END,
  last_failure_at = CASE
    WHEN pool_member_scores.last_failure_at IS NULL OR pool_member_scores.last_failure_at <= excluded.updated_at
    THEN excluded.last_failure_at ELSE pool_member_scores.last_failure_at END,
  failure_count = CASE
    WHEN pool_member_scores.last_failure_at IS NULL OR pool_member_scores.last_failure_at <= excluded.updated_at
    THEN excluded.failure_count ELSE pool_member_scores.failure_count END,
  last_probe_failure_at = CASE
    WHEN pool_member_scores.last_probe_failure_at IS NULL OR pool_member_scores.last_probe_failure_at <= excluded.updated_at
    THEN excluded.last_probe_failure_at ELSE pool_member_scores.last_probe_failure_at END,
  probe_failure_count = CASE
    WHEN pool_member_scores.last_probe_failure_at IS NULL OR pool_member_scores.last_probe_failure_at <= excluded.updated_at
    THEN excluded.probe_failure_count ELSE pool_member_scores.probe_failure_count END,
  probe_status = CASE
    WHEN (pool_member_scores.last_probe_attempt_at IS NOT NULL AND pool_member_scores.last_probe_attempt_at > excluded.updated_at)
      OR (pool_member_scores.last_probe_success_at IS NOT NULL AND pool_member_scores.last_probe_success_at > excluded.updated_at)
      OR (pool_member_scores.last_probe_failure_at IS NOT NULL AND pool_member_scores.last_probe_failure_at > excluded.updated_at)
    THEN pool_member_scores.probe_status ELSE excluded.probe_status END,
  updated_at = MAX(pool_member_scores.updated_at, excluded.updated_at)
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
pub struct SqlitePoolMemberScoreRepository {
    pool: SqlitePool,
}

impl SqlitePoolMemberScoreRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn find_scores_by_identity(
        &self,
        identity: &PoolMemberIdentity,
        scope: Option<&PoolScoreScope>,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(SCORE_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_kind",
            identity.pool_kind.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_id",
            identity.pool_id.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "member_kind",
            identity.member_kind.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "member_id",
            identity.member_id.clone(),
        );
        if let Some(scope) = scope {
            push_eq(
                &mut builder,
                &mut where_clause,
                "capability",
                scope.capability.clone(),
            );
            push_eq(
                &mut builder,
                &mut where_clause,
                "scope_kind",
                scope.scope_kind.clone(),
            );
            if let Some(scope_id) = &scope.scope_id {
                push_eq(
                    &mut builder,
                    &mut where_clause,
                    "scope_id",
                    scope_id.clone(),
                );
            } else {
                where_clause.push_next(&mut builder);
                builder.push("scope_id IS NULL");
            }
        }
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }
}

#[async_trait]
impl PoolScoreReadRepository for SqlitePoolMemberScoreRepository {
    async fn list_ranked_pool_members(
        &self,
        query: &ListRankedPoolMembersQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(SCORE_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_kind",
            query.pool_kind.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_id",
            query.pool_id.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "capability",
            query.capability.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "scope_kind",
            query.scope_kind.clone(),
        );
        if let Some(scope_id) = &query.scope_id {
            push_eq(
                &mut builder,
                &mut where_clause,
                "scope_id",
                scope_id.clone(),
            );
        } else {
            where_clause.push_next(&mut builder);
            builder.push("scope_id IS NULL");
        }
        if !query.hard_states.is_empty() {
            let states = query
                .hard_states
                .iter()
                .map(|state| state.as_database())
                .collect::<Vec<_>>();
            push_in(&mut builder, &mut where_clause, "hard_state", &states);
        }
        if let Some(statuses) = &query.probe_statuses {
            if !statuses.is_empty() {
                let statuses = statuses
                    .iter()
                    .map(|status| status.as_database())
                    .collect::<Vec<_>>();
                push_in(&mut builder, &mut where_clause, "probe_status", &statuses);
            }
        }
        builder.push(" ORDER BY score DESC, last_ranked_at DESC, member_id ASC, id ASC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(query.limit.max(1), "pool score limit")?,
            i64_from_usize(query.offset, "pool score offset")?,
        );
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn list_pool_member_scores(
        &self,
        query: &ListPoolMemberScoresQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(SCORE_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_kind",
            query.pool_kind.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_id",
            query.pool_id.clone(),
        );
        if let Some(capability) = &query.capability {
            push_eq(
                &mut builder,
                &mut where_clause,
                "capability",
                capability.clone(),
            );
        }
        if let Some(scope_kind) = &query.scope_kind {
            push_eq(
                &mut builder,
                &mut where_clause,
                "scope_kind",
                scope_kind.clone(),
            );
        }
        if let Some(scope_id) = &query.scope_id {
            push_eq(
                &mut builder,
                &mut where_clause,
                "scope_id",
                scope_id.clone(),
            );
        }
        if !query.hard_states.is_empty() {
            let states = query
                .hard_states
                .iter()
                .map(|state| state.as_database())
                .collect::<Vec<_>>();
            push_in(&mut builder, &mut where_clause, "hard_state", &states);
        }
        if let Some(statuses) = &query.probe_statuses {
            if !statuses.is_empty() {
                let statuses = statuses
                    .iter()
                    .map(|status| status.as_database())
                    .collect::<Vec<_>>();
                push_in(&mut builder, &mut where_clause, "probe_status", &statuses);
            }
        }
        builder.push(" ORDER BY score DESC, last_ranked_at DESC, member_id ASC, id ASC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(query.limit.max(1), "pool score limit")?,
            i64_from_usize(query.offset, "pool score offset")?,
        );
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn list_pool_member_probe_candidates(
        &self,
        query: &ListPoolMemberProbeCandidatesQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(SCORE_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_kind",
            query.pool_kind.clone(),
        );
        push_eq(
            &mut builder,
            &mut where_clause,
            "pool_id",
            query.pool_id.clone(),
        );
        if let Some(capability) = &query.capability {
            push_eq(
                &mut builder,
                &mut where_clause,
                "capability",
                capability.clone(),
            );
        }
        where_clause.push_next(&mut builder);
        builder
            .push("hard_state IN ('available','unknown','cooldown','quota_exhausted')")
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
            );
        push_limit(
            &mut builder,
            i64_from_usize(query.limit.max(1), "pool probe candidate limit")?,
        );
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
        let mut builder = QueryBuilder::<Sqlite>::new(SCORE_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_in(&mut builder, &mut where_clause, "id", &query.ids);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_score_row).collect()
    }
}

#[async_trait]
impl PoolMemberScoreWriteRepository for SqlitePoolMemberScoreRepository {
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

fn map_score_row(row: &SqliteRow) -> Result<StoredPoolMemberScore, DataLayerError> {
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
    use crate::run_migrations;

    fn score(timestamp_base: Option<u64>) -> UpsertPoolMemberScore {
        UpsertPoolMemberScore {
            id: "score-1".to_string(),
            identity: PoolMemberIdentity::provider_api_key("provider-1", "key-1"),
            scope: PoolScoreScope {
                capability: POOL_SCORE_CAPABILITY_ACCOUNT.to_string(),
                scope_kind: POOL_SCORE_SCOPE_KIND_ACCOUNT.to_string(),
                scope_id: None,
            },
            score: 0.75,
            hard_state: PoolMemberHardState::Available,
            score_version: 1,
            score_reason: serde_json::json!({}),
            last_ranked_at: Some(20),
            last_scheduled_at: timestamp_base,
            last_success_at: timestamp_base.map(|value| value + 1),
            last_failure_at: timestamp_base.map(|value| value + 2),
            failure_count: 0,
            last_probe_attempt_at: timestamp_base.map(|value| value + 3),
            last_probe_success_at: timestamp_base.map(|value| value + 4),
            last_probe_failure_at: timestamp_base.map(|value| value + 5),
            probe_failure_count: 0,
            probe_status: PoolMemberProbeStatus::Never,
            updated_at: 20,
        }
    }

    async fn load_score(repository: &SqlitePoolMemberScoreRepository) -> StoredPoolMemberScore {
        repository
            .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery {
                ids: vec!["score-1".to_string()],
            })
            .await
            .expect("pool score should load")
            .pop()
            .expect("pool score should exist")
    }

    async fn repository() -> SqlitePoolMemberScoreRepository {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        SqlitePoolMemberScoreRepository::new(pool)
    }

    #[tokio::test]
    async fn sqlite_ordinary_upsert_preserves_nullable_timestamps() {
        let repository = repository().await;

        repository
            .upsert_pool_member_score(score(Some(100)))
            .await
            .expect("initial pool score should insert");
        repository
            .upsert_pool_member_score(score(None))
            .await
            .expect("ordinary upsert should succeed");
        let preserved = load_score(&repository).await;
        assert_eq!(preserved.last_scheduled_at, Some(100));
        assert_eq!(preserved.last_success_at, Some(101));
        assert_eq!(preserved.last_failure_at, Some(102));
        assert_eq!(preserved.last_probe_attempt_at, Some(103));
        assert_eq!(preserved.last_probe_success_at, Some(104));
        assert_eq!(preserved.last_probe_failure_at, Some(105));
    }

    #[tokio::test]
    async fn sqlite_oauth_recovery_clears_old_failures_and_preserves_success_history() {
        let repository = repository().await;
        let mut invalid = score(None);
        invalid.score = 0.2;
        invalid.hard_state = PoolMemberHardState::AuthInvalid;
        invalid.score_reason = serde_json::json!({"state": "invalid"});
        invalid.last_ranked_at = Some(90);
        invalid.last_scheduled_at = Some(80);
        invalid.last_success_at = Some(81);
        invalid.last_failure_at = Some(90);
        invalid.failure_count = 9;
        invalid.last_probe_attempt_at = Some(82);
        invalid.last_probe_success_at = Some(83);
        invalid.last_probe_failure_at = Some(91);
        invalid.probe_failure_count = 4;
        invalid.probe_status = PoolMemberProbeStatus::Failed;
        invalid.updated_at = 91;
        repository
            .upsert_pool_member_score(invalid)
            .await
            .expect("invalid score should insert");
        let mut recovery = score(None);
        recovery.score = 0.9;
        recovery.score_reason = serde_json::json!({"state": "recovered"});
        recovery.last_ranked_at = Some(100);
        recovery.updated_at = 100;

        repository
            .upsert_pool_member_score_with_mode(recovery, PoolMemberScoreUpsertMode::OAuthRecovery)
            .await
            .expect("OAuth recovery should succeed");
        let recovered = load_score(&repository).await;
        assert_eq!(recovered.score, 0.9);
        assert_eq!(recovered.hard_state, PoolMemberHardState::Available);
        assert_eq!(
            recovered.score_reason,
            serde_json::json!({"state": "recovered"})
        );
        assert_eq!(recovered.last_ranked_at, Some(100));
        assert_eq!(recovered.last_scheduled_at, Some(80));
        assert_eq!(recovered.last_success_at, Some(81));
        assert_eq!(recovered.last_failure_at, None);
        assert_eq!(recovered.failure_count, 0);
        assert_eq!(recovered.last_probe_attempt_at, Some(82));
        assert_eq!(recovered.last_probe_success_at, Some(83));
        assert_eq!(recovered.last_probe_failure_at, None);
        assert_eq!(recovered.probe_failure_count, 0);
        assert_eq!(recovered.probe_status, PoolMemberProbeStatus::Never);
        assert_eq!(recovered.updated_at, 100);
    }

    #[tokio::test]
    async fn sqlite_oauth_recovery_preserves_newer_feedback() {
        let repository = repository().await;
        let mut current = score(None);
        current.score = 0.3;
        current.hard_state = PoolMemberHardState::AuthInvalid;
        current.score_version = 7;
        current.score_reason = serde_json::json!({"state": "newer_failure"});
        current.last_ranked_at = Some(120);
        current.last_scheduled_at = Some(120);
        current.last_success_at = Some(80);
        current.last_failure_at = Some(120);
        current.failure_count = 3;
        current.last_probe_attempt_at = Some(130);
        current.last_probe_success_at = Some(85);
        current.last_probe_failure_at = Some(130);
        current.probe_failure_count = 2;
        current.probe_status = PoolMemberProbeStatus::Failed;
        current.updated_at = 130;
        repository
            .upsert_pool_member_score(current.clone())
            .await
            .expect("newer score should insert");
        let mut stale_recovery = score(None);
        stale_recovery.score = 1.0;
        stale_recovery.score_version = 8;
        stale_recovery.score_reason = serde_json::json!({"state": "recovered"});
        stale_recovery.last_ranked_at = Some(100);
        stale_recovery.updated_at = 100;

        repository
            .upsert_pool_member_score_with_mode(
                stale_recovery,
                PoolMemberScoreUpsertMode::OAuthRecovery,
            )
            .await
            .expect("stale OAuth recovery should succeed");
        let preserved = load_score(&repository).await;
        let current = current.into_stored();
        assert_eq!(preserved.score, current.score);
        assert_eq!(preserved.hard_state, current.hard_state);
        assert_eq!(preserved.score_version, current.score_version);
        assert_eq!(preserved.score_reason, current.score_reason);
        assert_eq!(preserved.last_ranked_at, current.last_ranked_at);
        assert_eq!(preserved.last_scheduled_at, current.last_scheduled_at);
        assert_eq!(preserved.last_success_at, current.last_success_at);
        assert_eq!(preserved.last_failure_at, current.last_failure_at);
        assert_eq!(preserved.failure_count, current.failure_count);
        assert_eq!(
            preserved.last_probe_attempt_at,
            current.last_probe_attempt_at
        );
        assert_eq!(
            preserved.last_probe_success_at,
            current.last_probe_success_at
        );
        assert_eq!(
            preserved.last_probe_failure_at,
            current.last_probe_failure_at
        );
        assert_eq!(preserved.probe_failure_count, current.probe_failure_count);
        assert_eq!(preserved.probe_status, current.probe_status);
        assert_eq!(preserved.updated_at, current.updated_at);
    }
}
