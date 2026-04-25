use std::collections::BTreeMap;

use aether_data::postgres::PostgresPool;
use aether_data_contracts::repository::usage::StoredUsageUserTotals;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use sqlx::Row;

use crate::query::usage_heatmap::read_stats_daily_cutoff_date;
use crate::GatewayError;

fn internal(err: impl ToString) -> GatewayError {
    GatewayError::Internal(err.to_string())
}

pub(crate) async fn list_user_usage_totals_from_stats_summary(
    pool: &PostgresPool,
    user_ids: &[String],
) -> Result<Option<Vec<StoredUsageUserTotals>>, GatewayError> {
    if user_ids.is_empty() {
        return Ok(Some(Vec::new()));
    }

    let Some(cutoff_date) = read_stats_daily_cutoff_date(pool).await? else {
        return Ok(None);
    };

    let mut totals = load_stats_user_summary_rows(pool, user_ids).await?;
    absorb_stats_user_summary_tail(pool, cutoff_date, user_ids, &mut totals).await?;

    let mut items = user_ids
        .iter()
        .map(|user_id| {
            totals
                .remove(user_id)
                .unwrap_or_else(|| StoredUsageUserTotals {
                    user_id: user_id.clone(),
                    request_count: 0,
                    total_tokens: 0,
                })
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.user_id.cmp(&right.user_id));
    Ok(Some(items))
}

async fn load_stats_user_summary_rows(
    pool: &PostgresPool,
    user_ids: &[String],
) -> Result<BTreeMap<String, StoredUsageUserTotals>, GatewayError> {
    let mut rows = sqlx::query(
        r#"
SELECT
  user_id,
  COALESCE(all_time_requests, 0)::BIGINT AS request_count,
  COALESCE(
    all_time_input_tokens
      + all_time_output_tokens
      + all_time_cache_creation_tokens
      + all_time_cache_read_tokens,
    0
  )::BIGINT AS total_tokens
FROM stats_user_summary
WHERE user_id = ANY($1::TEXT[])
ORDER BY user_id ASC
"#,
    )
    .bind(user_ids)
    .fetch(pool);

    let mut items = BTreeMap::new();
    while let Some(row) = rows
        .try_next()
        .await
        .map_err(|err| internal(format!("stats_user_summary lookup failed: {err}")))?
    {
        let user_id = row
            .try_get::<String, _>("user_id")
            .map_err(|err| internal(format!("stats_user_summary decode failed: {err}")))?;
        let request_count = row
            .try_get::<i64, _>("request_count")
            .map_err(|err| internal(format!("stats_user_summary decode failed: {err}")))?
            .max(0) as u64;
        let total_tokens = row
            .try_get::<i64, _>("total_tokens")
            .map_err(|err| internal(format!("stats_user_summary decode failed: {err}")))?
            .max(0) as u64;
        items.insert(
            user_id.clone(),
            StoredUsageUserTotals {
                user_id,
                request_count,
                total_tokens,
            },
        );
    }

    Ok(items)
}

async fn absorb_stats_user_summary_tail(
    pool: &PostgresPool,
    cutoff_date: DateTime<Utc>,
    user_ids: &[String],
    totals: &mut BTreeMap<String, StoredUsageUserTotals>,
) -> Result<(), GatewayError> {
    let mut rows = sqlx::query(
        r#"
SELECT
  "usage".user_id,
  COUNT(*)::BIGINT AS request_count,
  COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)), 0)::BIGINT AS total_tokens
FROM usage_billing_facts AS "usage"
WHERE "usage".user_id = ANY($1::TEXT[])
  AND "usage".created_at >= $2
  AND "usage".status NOT IN ('pending', 'streaming')
  AND "usage".provider_name NOT IN ('unknown', 'pending')
GROUP BY "usage".user_id
ORDER BY "usage".user_id ASC
"#,
    )
    .bind(user_ids)
    .bind(cutoff_date)
    .fetch(pool);

    while let Some(row) = rows
        .try_next()
        .await
        .map_err(|err| internal(format!("stats_user_summary tail lookup failed: {err}")))?
    {
        let user_id = row
            .try_get::<String, _>("user_id")
            .map_err(|err| internal(format!("stats_user_summary tail decode failed: {err}")))?;
        let request_count = row
            .try_get::<i64, _>("request_count")
            .map_err(|err| internal(format!("stats_user_summary tail decode failed: {err}")))?
            .max(0) as u64;
        let total_tokens = row
            .try_get::<i64, _>("total_tokens")
            .map_err(|err| internal(format!("stats_user_summary tail decode failed: {err}")))?
            .max(0) as u64;
        let entry = totals
            .entry(user_id.clone())
            .or_insert_with(|| StoredUsageUserTotals {
                user_id,
                request_count: 0,
                total_tokens: 0,
            });
        entry.request_count = entry.request_count.saturating_add(request_count);
        entry.total_tokens = entry.total_tokens.saturating_add(total_tokens);
    }

    Ok(())
}
