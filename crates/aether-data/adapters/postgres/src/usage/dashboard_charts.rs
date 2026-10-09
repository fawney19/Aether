use super::{
    analytics::{analytics_metrics_sql, push_analytics_source_filter},
    projection_reader::read_projection_coverage,
    SqlxUsageReadRepository,
};
use crate::error::SqlxResultExt;
use aether_data_contracts::{repository::usage::*, DataLayerError};
use chrono::{DateTime, Duration, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row};

// Committed request contributions are updated in the same transaction as the
// top dashboard counters. Reuse them without detoasting large request metadata.
// Rows predating that ledger retain DASHBOARD_TOTAL_FACTS_SQL token precedence.
const DASHBOARD_CHART_FACTS_SQL: &str = r#"(
SELECT u.created_at,u.model,u.provider_id,u.provider_name,u.status,u.response_time_ms,
  COALESCE(a.record_kind,'request') AS record_kind,
  COALESCE(s.billing_status,u.billing_status) AS settlement_status,s.allocation_status,
  CASE WHEN c.request_id IS NOT NULL THEN (c.metrics->>'usage_available_count')::bigint>0
    ELSE COALESCE(metadata.value->'usage_available','true'::jsonb)<>'false'::jsonb END AS usage_available,
  CASE WHEN c.request_id IS NOT NULL THEN
    CASE WHEN (c.metrics->>'usage_available_count')::bigint>0 THEN (c.metrics->>'total_tokens')::bigint END
  WHEN COALESCE(metadata.value->'usage_available', 'true'::jsonb) <> 'false'::jsonb THEN
  GREATEST(
    COALESCE(
      CASE
        WHEN s.billing_effective_input_tokens IS NOT NULL
        THEN GREATEST(s.billing_effective_input_tokens, 0)
          + GREATEST(COALESCE(s.billing_output_tokens, u.output_tokens, 0), 0)
          + GREATEST(
              COALESCE(
                s.billing_cache_creation_tokens,
                CASE
                  WHEN s.billing_cache_creation_5m_tokens IS NOT NULL
                    OR s.billing_cache_creation_1h_tokens IS NOT NULL
                  THEN COALESCE(s.billing_cache_creation_5m_tokens, 0)
                    + COALESCE(s.billing_cache_creation_1h_tokens, 0)
                END,
                CASE
                  WHEN COALESCE(u.cache_creation_input_tokens, 0) = 0
                       AND (
                         COALESCE(u.cache_creation_input_tokens_5m, 0)
                         + COALESCE(u.cache_creation_input_tokens_1h, 0)
                       ) > 0
                  THEN COALESCE(u.cache_creation_input_tokens_5m, 0)
                    + COALESCE(u.cache_creation_input_tokens_1h, 0)
                  ELSE COALESCE(u.cache_creation_input_tokens, 0)
                END,
                0
              ),
              0
            )
          + GREATEST(
              COALESCE(
                s.billing_cache_read_tokens,
                u.cache_read_input_tokens,
                0
              ),
              0
            )
        WHEN s.billing_total_input_context IS NOT NULL
        THEN GREATEST(s.billing_total_input_context, 0)
          + GREATEST(COALESCE(s.billing_output_tokens, u.output_tokens, 0), 0)
      END,
      NULLIF(GREATEST(COALESCE(u.total_tokens, 0), 0), 0),
      CASE
        WHEN split_part(lower(COALESCE(COALESCE(u.endpoint_api_format, u.api_format), '')), ':', 1)
             IN ('openai', 'gemini', 'google')
        THEN GREATEST(COALESCE(u.input_tokens, 0), 0)
          + GREATEST(COALESCE(u.output_tokens, 0), 0)
        ELSE GREATEST(COALESCE(u.input_tokens, 0), 0)
          + GREATEST(COALESCE(u.output_tokens, 0), 0)
          + GREATEST(
              CASE
                WHEN COALESCE(u.cache_creation_input_tokens, 0) = 0
                     AND (
                       COALESCE(u.cache_creation_input_tokens_5m, 0)
                       + COALESCE(u.cache_creation_input_tokens_1h, 0)
                     ) > 0
                THEN COALESCE(u.cache_creation_input_tokens_5m, 0)
                  + COALESCE(u.cache_creation_input_tokens_1h, 0)
                ELSE COALESCE(u.cache_creation_input_tokens, 0)
              END,
              0
            )
          + GREATEST(COALESCE(u.cache_read_input_tokens, 0), 0)
      END,
      0
    ),
    0
  )::bigint END AS total_tokens,
  CASE WHEN CASE WHEN c.request_id IS NOT NULL THEN (c.metrics->>'pricing_available_count')::bigint>0
    ELSE COALESCE(metadata.value->'usage_pricing_available','true'::jsonb)<>'false'::jsonb END
    AND (s.billing_total_cost_usd IS NOT NULL OR COALESCE(s.billing_status,u.billing_status)='settled')
    THEN round(COALESCE(s.billing_total_cost_usd::numeric,u.total_cost_usd::numeric),8) END AS rated_amount,
  CASE WHEN c.request_id IS NOT NULL THEN
    CASE WHEN (c.metrics->>'pricing_available_count')::bigint>0
      THEN round((c.metrics->>'billable_amount')::numeric,8) END
  WHEN COALESCE(metadata.value->'usage_pricing_available','true'::jsonb)<>'false'::jsonb
    AND (s.billing_actual_total_cost_usd IS NOT NULL OR COALESCE(s.billing_status,u.billing_status)='settled')
    THEN public.usage_customer_billable_amount(metadata.value,
      COALESCE(s.billing_total_cost_usd::numeric,u.total_cost_usd::numeric),
      COALESCE(s.billing_actual_total_cost_usd::numeric,u.actual_total_cost_usd::numeric)) END AS billable_amount
FROM public.usage u
LEFT JOIN public.dashboard_request_contributions c USING(request_id)
LEFT JOIN public.usage_settlement_snapshots s USING(request_id)
LEFT JOIN public.usage_attribution_snapshots a USING(request_id)
LEFT JOIN LATERAL (SELECT u.request_metadata::jsonb AS value
  WHERE c.request_id IS NULL OFFSET 0) metadata ON true
) AS chart_facts"#;

const DASHBOARD_CHART_METRICS_SQL: &str = r#"
count(*)::bigint AS request_count,
count(*) FILTER (WHERE status='completed')::bigint AS successful_request_count,
count(*) FILTER (WHERE status='failed')::bigint AS failed_request_count,
count(*) FILTER (WHERE status='cancelled')::bigint AS cancelled_request_count,
count(*) FILTER (WHERE status NOT IN ('completed','failed','cancelled'))::bigint AS in_flight_request_count,
COALESCE(sum(total_tokens),0)::bigint AS total_tokens,
count(*) FILTER (WHERE usage_available)::bigint AS usage_available_count,
count(billable_amount)::bigint AS pricing_available_count,
count(*) FILTER (WHERE settlement_status='settled')::bigint AS settled_count,
count(*) FILTER (WHERE allocation_status='complete')::bigint AS allocation_available_count,
sum(rated_amount)::text AS rated_amount,
sum(billable_amount)::text AS billable_amount,
count(response_time_ms)::bigint AS latency_sample_count,
COALESCE(sum(response_time_ms),0)::double precision AS latency_sum_ms
"#;

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, DataLayerError> {
    serde_json::from_value(value)
        .map_err(|error| DataLayerError::UnexpectedValue(error.to_string()))
}

fn can_compare_daily_history(query: &UsageAnalyticsQuery) -> bool {
    query.actor_user_id.is_none()
        && query.credential_owner_id.is_none()
        && query.attribution_kind.is_none()
        && query.api_key_id.is_none()
        && query.model.is_none()
        && query.provider_id.is_none()
        && query.api_format.is_none()
        && query.endpoint_kind.is_none()
        && query.request_type.is_none()
        && query.status.is_none()
        && query.is_stream.is_none()
        && query.has_format_conversion.is_none()
        && query.search.is_none()
        && query.user_is_active.is_none()
        && query.has_usage.is_none()
}

impl SqlxUsageReadRepository {
    /// Keep every chart and its summary on the same bounded canonical fact scan.
    /// Historical projections can outlive raw facts and must not be mixed into
    /// this summary while its model/provider breakdown still reads raw rows.
    pub(super) async fn query_dashboard_charts(
        &self,
        query: &UsageAnalyticsQuery,
    ) -> Result<StoredUsageAnalytics, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query("SET LOCAL statement_timeout = '15s'")
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query("SET LOCAL jit = off")
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        let state = sqlx::query("SELECT pg_current_snapshot()::text AS revision, NOW() AS generated_at, (SELECT count(*) FROM users WHERE is_active AND NOT is_deleted) AS enabled_users")
            .fetch_one(&mut *tx).await.map_postgres_err()?;
        let granularity = match query.granularity {
            UsageAnalyticsGranularity::Hour => "hour",
            UsageAnalyticsGranularity::Day => "day",
        };
        let timezone = match query.granularity {
            UsageAnalyticsGranularity::Hour => "UTC",
            UsageAnalyticsGranularity::Day => query.timezone.as_str(),
        };
        let compare_history = can_compare_daily_history(query);
        // Like dashboard_request_fact, coverage describes customer charges;
        // a rated request with an invalid factor snapshot is still unpriced.
        let metrics_sql = if compare_history {
            DASHBOARD_CHART_METRICS_SQL.to_string()
        } else {
            analytics_metrics_sql(query.slow_threshold_ms.unwrap_or(5000)).replace(
                "count(*) FILTER (WHERE pricing_available)::bigint AS pricing_available_count",
                "count(billable_amount)::bigint AS pricing_available_count",
            )
        };
        let from = DateTime::<Utc>::from_timestamp_millis(query.from_unix_ms as i64)
            .expect("validated timestamp");
        let to = DateTime::<Utc>::from_timestamp_millis(query.to_unix_ms as i64)
            .expect("validated timestamp");
        let source_from = from
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("UTC midnight")
            .and_utc();
        let source_to_floor = to
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("UTC midnight")
            .and_utc();
        let source_to = if source_to_floor == to {
            to
        } else {
            source_to_floor + Duration::days(1)
        };
        let mut builder = QueryBuilder::<Postgres>::new("WITH ");
        if compare_history {
            // Legacy daily totals include sessions. Keep them in this one raw
            // scan for the coverage check, then exclude them from every chart.
            builder
                .push("source AS MATERIALIZED (SELECT * FROM ")
                .push(DASHBOARD_CHART_FACTS_SQL)
                .push(" WHERE created_at >= ")
                .push_bind(source_from)
                .push(" AND created_at < ")
                .push_bind(source_to)
                .push("), ");
        }
        builder.push("filtered AS MATERIALIZED (SELECT *, date_trunc(");
        builder
            .push_bind(granularity)
            .push(", created_at AT TIME ZONE ")
            .push_bind(timezone)
            .push(") AT TIME ZONE ")
            .push_bind(timezone)
            .push(" AS bucket, COALESCE(NULLIF(provider_id,''),NULLIF(provider_name,'')) AS chart_provider_id");
        push_analytics_source_filter(
            &mut builder,
            query,
            if compare_history {
                "source"
            } else {
                "public.usage_analytics_facts_v1"
            },
        );
        builder
            .push("), summary AS (SELECT ")
            .push(&metrics_sql)
            .push(" FROM filtered), series AS (SELECT bucket, ")
            .push(&metrics_sql)
            .push(", count(DISTINCT COALESCE(NULLIF(provider_id,''),NULLIF(provider_name,''))) FILTER (WHERE NULLIF(provider_id,'') IS NOT NULL OR provider_name NOT IN ('unknown','pending'))::bigint AS unique_providers FROM filtered GROUP BY bucket ORDER BY bucket LIMIT 10001), models AS (SELECT model, bucket, ")
            .push(&metrics_sql)
            .push(" FROM filtered GROUP BY model,bucket ORDER BY bucket,model LIMIT 10001), providers AS (SELECT chart_provider_id, max(provider_name) AS provider_label, ")
            .push(&metrics_sql)
            .push(" FROM filtered GROUP BY chart_provider_id ORDER BY count(*) DESC,chart_provider_id LIMIT 10001)");
        if compare_history {
            builder.push(r#", historical_cutoff AS (
              SELECT CASE WHEN pg_typeof(cutoff_date) IN ('bigint'::regtype,'integer'::regtype)
                THEN to_timestamp(cutoff_date::text::double precision)
                ELSE cutoff_date::text::timestamptz END AS cutoff
              FROM stats_summary ORDER BY updated_at DESC,created_at DESC LIMIT 1
            ), historical_days AS (
              SELECT CASE WHEN pg_typeof(date) IN ('bigint'::regtype,'integer'::regtype)
                THEN to_timestamp(date::text::double precision)
                ELSE date::text::timestamptz END AS day,total_requests
              FROM stats_daily WHERE is_complete AND total_requests>0
            ), retained_days AS (
              SELECT date_trunc('day',created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS day,
                count(*) AS requests
              FROM source
              WHERE status NOT IN ('pending','streaming') AND provider_name NOT IN ('unknown','pending')
              GROUP BY 1
            ), lost_days AS (
              SELECT d.day FROM historical_days d CROSS JOIN historical_cutoff c
              LEFT JOIN retained_days r ON r.day=d.day
              WHERE d.day=date_trunc('day',d.day AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                AND d.day < "#)
                .push_bind(to)
                .push(" AND d.day+INTERVAL '24 hours' > ")
                .push_bind(from)
                .push(" AND d.day+INTERVAL '24 hours' <= c.cutoff AND d.total_requests>COALESCE(r.requests,0))");
        }
        // An archived day with missing details has unknown coverage in each
        // constituent hour. Union with existing markers to avoid double-counting.
        builder.push(
            r#", lost_hours AS (
          SELECT bucket_start FROM stats_bucket_state
          WHERE projection_version IN ('overview-v1','overview-v2')
            AND granularity='hour' AND coverage_status='unrecoverable'
          UNION SELECT bucket_start FROM stats_overview_dirty_events
          WHERE projection_version='overview-v2' AND granularity='hour' AND unrecoverable
        "#,
        );
        if compare_history {
            builder.push(" UNION SELECT hour FROM lost_days CROSS JOIN LATERAL generate_series(day,day+INTERVAL '23 hours',INTERVAL '1 hour') AS hours(hour)");
        }
        builder.push("), lost AS (SELECT count(*)::bigint AS hours FROM lost_hours WHERE bucket_start < ")
            .push_bind(to)
            .push(" AND bucket_start+INTERVAL '1 hour' > ")
            .push_bind(from)
            .push(")")
            .push(r#"
SELECT
  (SELECT hours FROM lost) AS unrecoverable_bucket_count,
  (SELECT to_jsonb(s) FROM summary s) AS summary,
  (SELECT COALESCE(jsonb_agg(jsonb_build_object(
    'id', bucket::text, 'label', bucket::text,
    'bucket_start', to_char(bucket AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'),
    'metrics', to_jsonb(s)-'bucket') ORDER BY bucket),'[]'::jsonb) FROM series s) AS series,
  (SELECT COALESCE(jsonb_agg(jsonb_build_object(
    'id',model,'label',model,
    'bucket_start',to_char(bucket AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"'),
    'metrics',to_jsonb(m)-'model'-'bucket') ORDER BY bucket,model),'[]'::jsonb) FROM models m) AS models,
  (SELECT COALESCE(jsonb_agg(jsonb_build_object(
    'id',chart_provider_id,'label',provider_label,'bucket_start',NULL,
    'metrics',to_jsonb(p)-'chart_provider_id'-'provider_label') ORDER BY request_count DESC,chart_provider_id),'[]'::jsonb) FROM providers p) AS providers
"#);
        let row = builder
            .build()
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
        let mut result = StoredUsageAnalytics {
            summary: decode(row.try_get("summary").map_postgres_err()?)?,
            rows: decode(row.try_get("series").map_postgres_err()?)?,
            model_rows: decode(row.try_get("models").map_postgres_err()?)?,
            provider_rows: decode(row.try_get("providers").map_postgres_err()?)?,
            unrecoverable_bucket_count: row
                .try_get::<i64, _>("unrecoverable_bucket_count")
                .map_postgres_err()? as u64,
            read_revision: state.try_get("revision").map_postgres_err()?,
            generated_at: state
                .try_get::<DateTime<Utc>, _>("generated_at")
                .map_postgres_err()?
                .to_rfc3339(),
            ..Default::default()
        };
        result.summary.enabled_users = state
            .try_get::<i64, _>("enabled_users")
            .map_postgres_err()? as u64;
        for (dimension, count) in [
            ("daily", result.rows.len()),
            ("model", result.model_rows.len()),
            ("provider", result.provider_rows.len()),
        ] {
            if count > USAGE_DASHBOARD_CHART_ROW_LIMIT {
                return Err(DataLayerError::InvalidInput(format!(
                    "dashboard {dimension} chart exceeds 10000 groups; narrow the range"
                )));
            }
        }
        result.coverage = read_projection_coverage(&mut tx, query, false).await?;
        fill_usage_analytics_timeseries(query, &mut result.rows);
        // Missing raw history is not evidence of zero spend. Keep populated
        // subtotals, but do not present filled empty buckets as measured zeros.
        if result.unrecoverable_bucket_count > 0 {
            for row in &mut result.rows {
                if row.metrics.request_count == 0 {
                    row.metrics.rated_amount = None;
                    row.metrics.billable_amount = None;
                }
            }
        } else if result.summary.request_count == 0 {
            result.summary.rated_amount = Some("0.00000000".into());
            result.summary.billable_amount = Some("0.00000000".into());
        }
        result.total = result.rows.len() as u64;
        tx.commit().await.map_postgres_err()?;
        Ok(result)
    }
}
