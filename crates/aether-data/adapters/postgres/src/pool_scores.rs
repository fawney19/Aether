use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::pool_scores::*;
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_eq, push_in, push_limit, push_limit_offset, WhereClause};

use crate::error::SqlxResultExt;

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

#[derive(Debug, Clone)]
pub struct PostgresPoolMemberScoreRepository {
    pool: PgPool,
}

impl PostgresPoolMemberScoreRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn find_scores_by_identity(
        &self,
        identity: &PoolMemberIdentity,
        scope: Option<&PoolScoreScope>,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(SCORE_COLUMNS);
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
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        rows.iter().map(map_score_row).collect()
    }
}

#[async_trait]
impl PoolScoreReadRepository for PostgresPoolMemberScoreRepository {
    async fn list_ranked_pool_members(
        &self,
        query: &ListRankedPoolMembersQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(SCORE_COLUMNS);
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
        builder.push(" ORDER BY score DESC, last_ranked_at DESC NULLS LAST, member_id ASC, id ASC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(query.limit.max(1), "pool score limit")?,
            i64_from_usize(query.offset, "pool score offset")?,
        );
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn list_pool_member_scores(
        &self,
        query: &ListPoolMemberScoresQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(SCORE_COLUMNS);
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
        builder.push(" ORDER BY score DESC, last_ranked_at DESC NULLS LAST, member_id ASC, id ASC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(query.limit.max(1), "pool score limit")?,
            i64_from_usize(query.offset, "pool score offset")?,
        );
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn list_pool_member_probe_candidates(
        &self,
        query: &ListPoolMemberProbeCandidatesQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(SCORE_COLUMNS);
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
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        rows.iter().map(map_score_row).collect()
    }

    async fn get_pool_member_scores_by_ids(
        &self,
        query: &GetPoolMemberScoresByIdsQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, DataLayerError> {
        if query.ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(&format!(
            "{SCORE_COLUMNS} WHERE id = ANY($1) ORDER BY id ASC"
        ))
        .bind(&query.ids)
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        rows.iter().map(map_score_row).collect()
    }
}

#[async_trait]
impl PoolMemberScoreWriteRepository for PostgresPoolMemberScoreRepository {
    async fn upsert_pool_member_score(
        &self,
        score: UpsertPoolMemberScore,
    ) -> Result<StoredPoolMemberScore, DataLayerError> {
        score.validate()?;
        let stored = score.clone().into_stored();
        sqlx::query(
            r#"
INSERT INTO pool_member_scores (
  id, pool_kind, pool_id, member_kind, member_id, capability, scope_kind, scope_id,
  score, hard_state, score_version, score_reason, last_ranked_at, last_scheduled_at,
  last_success_at, last_failure_at, failure_count, last_probe_attempt_at,
  last_probe_success_at, last_probe_failure_at, probe_failure_count, probe_status, updated_at
) VALUES (
  $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23
)
ON CONFLICT(id) DO UPDATE SET
  pool_kind = EXCLUDED.pool_kind,
  pool_id = EXCLUDED.pool_id,
  member_kind = EXCLUDED.member_kind,
  member_id = EXCLUDED.member_id,
  capability = EXCLUDED.capability,
  scope_kind = EXCLUDED.scope_kind,
  scope_id = EXCLUDED.scope_id,
  score = EXCLUDED.score,
  hard_state = EXCLUDED.hard_state,
  score_version = EXCLUDED.score_version,
  score_reason = EXCLUDED.score_reason,
  last_ranked_at = EXCLUDED.last_ranked_at,
  last_scheduled_at = COALESCE(EXCLUDED.last_scheduled_at, pool_member_scores.last_scheduled_at),
  last_success_at = COALESCE(EXCLUDED.last_success_at, pool_member_scores.last_success_at),
  last_failure_at = COALESCE(EXCLUDED.last_failure_at, pool_member_scores.last_failure_at),
  failure_count = EXCLUDED.failure_count,
  last_probe_attempt_at = COALESCE(EXCLUDED.last_probe_attempt_at, pool_member_scores.last_probe_attempt_at),
  last_probe_success_at = COALESCE(EXCLUDED.last_probe_success_at, pool_member_scores.last_probe_success_at),
  last_probe_failure_at = COALESCE(EXCLUDED.last_probe_failure_at, pool_member_scores.last_probe_failure_at),
  probe_failure_count = EXCLUDED.probe_failure_count,
  probe_status = EXCLUDED.probe_status,
  updated_at = EXCLUDED.updated_at
"#,
        )
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
        .bind(&stored.score_reason)
        .bind(i64_opt_from_u64(stored.last_ranked_at, "pool score last_ranked_at")?)
        .bind(i64_opt_from_u64(stored.last_scheduled_at, "pool score last_scheduled_at")?)
        .bind(i64_opt_from_u64(stored.last_success_at, "pool score last_success_at")?)
        .bind(i64_opt_from_u64(stored.last_failure_at, "pool score last_failure_at")?)
        .bind(i64_from_u64(stored.failure_count, "pool score failure_count")?)
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
        .map_postgres_err()?;
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
WHERE pool_kind = $1 AND pool_id = $2 AND member_kind = $3 AND member_id = $4
"#,
        )
        .bind(identity.pool_kind.as_str())
        .bind(identity.pool_id.as_str())
        .bind(identity.member_kind.as_str())
        .bind(identity.member_id.as_str())
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(result.rows_affected() as usize)
    }
}

fn map_score_row(row: &PgRow) -> Result<StoredPoolMemberScore, DataLayerError> {
    Ok(StoredPoolMemberScore {
        id: row.try_get("id").map_postgres_err()?,
        pool_kind: row.try_get("pool_kind").map_postgres_err()?,
        pool_id: row.try_get("pool_id").map_postgres_err()?,
        member_kind: row.try_get("member_kind").map_postgres_err()?,
        member_id: row.try_get("member_id").map_postgres_err()?,
        capability: row.try_get("capability").map_postgres_err()?,
        scope_kind: row.try_get("scope_kind").map_postgres_err()?,
        scope_id: row.try_get("scope_id").map_postgres_err()?,
        score: row.try_get("score").map_postgres_err()?,
        hard_state: PoolMemberHardState::from_database(
            row.try_get::<String, _>("hard_state")
                .map_postgres_err()?
                .as_str(),
        )?,
        score_version: u64_from_i64(
            row.try_get("score_version").map_postgres_err()?,
            "pool_member_scores.score_version",
        )?,
        score_reason: row.try_get("score_reason").map_postgres_err()?,
        last_ranked_at: u64_opt_from_i64(
            row.try_get("last_ranked_at").map_postgres_err()?,
            "pool_member_scores.last_ranked_at",
        )?,
        last_scheduled_at: u64_opt_from_i64(
            row.try_get("last_scheduled_at").map_postgres_err()?,
            "pool_member_scores.last_scheduled_at",
        )?,
        last_success_at: u64_opt_from_i64(
            row.try_get("last_success_at").map_postgres_err()?,
            "pool_member_scores.last_success_at",
        )?,
        last_failure_at: u64_opt_from_i64(
            row.try_get("last_failure_at").map_postgres_err()?,
            "pool_member_scores.last_failure_at",
        )?,
        failure_count: u64_from_i64(
            row.try_get("failure_count").map_postgres_err()?,
            "pool_member_scores.failure_count",
        )?,
        last_probe_attempt_at: u64_opt_from_i64(
            row.try_get("last_probe_attempt_at").map_postgres_err()?,
            "pool_member_scores.last_probe_attempt_at",
        )?,
        last_probe_success_at: u64_opt_from_i64(
            row.try_get("last_probe_success_at").map_postgres_err()?,
            "pool_member_scores.last_probe_success_at",
        )?,
        last_probe_failure_at: u64_opt_from_i64(
            row.try_get("last_probe_failure_at").map_postgres_err()?,
            "pool_member_scores.last_probe_failure_at",
        )?,
        probe_failure_count: u64_from_i64(
            row.try_get("probe_failure_count").map_postgres_err()?,
            "pool_member_scores.probe_failure_count",
        )?,
        probe_status: PoolMemberProbeStatus::from_database(
            row.try_get::<String, _>("probe_status")
                .map_postgres_err()?
                .as_str(),
        )?,
        updated_at: u64_from_i64(
            row.try_get("updated_at").map_postgres_err()?,
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
