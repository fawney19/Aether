use super::SqlxUsageReadRepository;
use crate::error::SqlxResultExt;
use aether_data_contracts::repository::usage::*;
use aether_data_contracts::DataLayerError;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row};

pub(super) fn analytics_metrics_sql(slow_threshold_ms: u64) -> String {
    format!(
        "{},{}",
        analytics_additive_metrics_sql(slow_threshold_ms),
        ANALYTICS_NON_ADDITIVE_METRICS_SQL
    )
}

const ANALYTICS_NON_ADDITIVE_METRICS_SQL: &str = r#"
count(DISTINCT actor_user_id)::bigint AS usage_active_users,
percentile_cont(0.5) WITHIN GROUP (ORDER BY response_time_ms) AS latency_p50_ms,
percentile_cont(0.90) WITHIN GROUP (ORDER BY response_time_ms) AS latency_p90_ms,
percentile_cont(0.95) WITHIN GROUP (ORDER BY response_time_ms) AS latency_p95_ms,
percentile_cont(0.99) WITHIN GROUP (ORDER BY response_time_ms) AS latency_p99_ms,
percentile_cont(0.90) WITHIN GROUP (ORDER BY first_byte_time_ms) AS first_byte_p90_ms,
percentile_cont(0.99) WITHIN GROUP (ORDER BY first_byte_time_ms) AS first_byte_p99_ms
"#;

// Full analytics also describe attribution and token provenance. Lifetime cards
// only need usage, pricing, settlement and allocation coverage below.
const ANALYTICS_COVERAGE_METRICS_SQL: &str = r#"
count(*) FILTER (WHERE usage_available)::bigint AS usage_available_count,
count(*) FILTER (WHERE usage_available AND token_source='reported')::bigint AS reported_usage_count,
count(*) FILTER (WHERE usage_available AND token_source='estimated')::bigint AS estimated_usage_count,
count(*) FILTER (WHERE usage_available AND token_source='mixed')::bigint AS mixed_usage_count,
count(*) FILTER (WHERE NOT usage_available OR token_source='unknown')::bigint AS unknown_usage_count,
count(*) FILTER (WHERE pricing_available)::bigint AS pricing_available_count,
count(*) FILTER (WHERE settlement_status = 'settled')::bigint AS settled_count,
count(*) FILTER (WHERE allocation_status = 'complete')::bigint AS allocation_available_count,
count(*) FILTER (WHERE actor_user_id IS NOT NULL)::bigint AS trusted_attribution_count,
count(*) FILTER (WHERE status = 'failed' AND failure_origin IS NOT NULL AND failure_origin <> 'unknown')::bigint AS classified_failure_count
"#;

// 提供商分组的展示标签：分组键仍然是 provider_id，但页面上要展示“提供商名称”。
// 解析顺序与用量审计聚合保持一致：提供商目录中的当前名称 → 使用记录里的名称快照 → 原始 provider_id。
// 'unknown' / 'unknow' / 'pending' 是历史占位值，不能当成名称展示；
// provider_id 为空说明这条记录本身无法归属，保持空标签让前端显示“未归属提供商”。
pub(super) const ANALYTICS_PROVIDER_LABEL_SQL: &str = r#"COALESCE(
        NULLIF(BTRIM(provider_catalog.name), ''),
        CASE
          WHEN page.group_id IS NULL THEN NULL
          WHEN lower(BTRIM(COALESCE(page.provider_name, ''))) IN ('', 'unknown', 'unknow', 'pending') THEN NULL
          ELSE BTRIM(page.provider_name)
        END,
        page.group_id::text
      )"#;

pub(super) fn dashboard_total_metrics_sql() -> &'static str {
    r#"count(*)::bigint AS request_count,
COALESCE(sum(total_tokens),0)::bigint AS total_tokens,
sum(billable_amount)::text AS billable_amount,
count(*) FILTER (WHERE usage_available)::bigint AS usage_available_count,
count(*) FILTER (WHERE pricing_available)::bigint AS pricing_available_count,
count(*) FILTER (WHERE settlement_status='settled')::bigint AS settled_count,
count(*) FILTER (WHERE allocation_status='complete')::bigint AS allocation_available_count"#
}

pub(super) fn analytics_additive_metrics_sql(slow_threshold_ms: u64) -> String {
    format!(
        r#"
count(*)::bigint AS request_count,
count(*) FILTER (WHERE status = 'completed')::bigint AS successful_request_count,
count(*) FILTER (WHERE status = 'failed')::bigint AS failed_request_count,
count(*) FILTER (WHERE status = 'cancelled')::bigint AS cancelled_request_count,
count(*) FILTER (WHERE status NOT IN ('completed','failed','cancelled'))::bigint AS in_flight_request_count,
COALESCE(sum(input_tokens),0)::bigint AS input_tokens,
COALESCE(sum(output_tokens),0)::bigint AS output_tokens,
COALESCE(sum(total_tokens),0)::bigint AS total_tokens,
COALESCE(sum(cache_read_input_tokens),0)::bigint AS cache_read_input_tokens,
COALESCE(sum(cache_creation_input_tokens),0)::bigint AS cache_creation_input_tokens,
count(cache_estimated_full_cost_amount)::bigint AS cache_pricing_available_count,
sum(cache_read_cost_amount)::text AS cache_read_cost_amount,
sum(cache_creation_cost_amount)::text AS cache_creation_cost_amount,
sum(cache_estimated_full_cost_amount)::text AS cache_estimated_full_cost_amount,
{ANALYTICS_COVERAGE_METRICS_SQL},
count(response_time_ms)::bigint AS latency_sample_count,
count(*) FILTER (WHERE response_time_ms >= {slow_threshold_ms})::bigint AS slow_request_count,
COALESCE(sum(response_time_ms),0)::double precision AS latency_sum_ms,
count(first_byte_time_ms)::bigint AS first_byte_sample_count,
COALESCE(sum(first_byte_time_ms),0)::double precision AS first_byte_sum_ms,
count(*) FILTER (WHERE upstream_is_stream AND output_tokens > 0 AND response_time_ms > first_byte_time_ms)::bigint AS output_tps_sample_count,
COALESCE(sum(output_tokens::double precision * 1000 / NULLIF(response_time_ms - first_byte_time_ms, 0)) FILTER (WHERE upstream_is_stream AND output_tokens > 0 AND response_time_ms > first_byte_time_ms),0)::double precision AS output_tps_sum,
sum(rated_amount)::text AS rated_amount,
sum(billable_amount)::text AS billable_amount,
sum(quota_covered_amount)::text AS quota_covered_amount,
sum(wallet_consumed_amount)::text AS wallet_consumed_amount,
sum(wallet_debit_amount)::text AS wallet_debit_amount,
sum(wallet_recharge_debit_amount)::text AS wallet_recharge_debit_amount,
sum(wallet_gift_debit_amount)::text AS wallet_gift_debit_amount,
sum(wallet_overdraft_amount)::text AS wallet_overdraft_amount
"#
    )
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T, DataLayerError> {
    serde_json::from_value(value).map_err(|error| {
        DataLayerError::UnexpectedValue(format!("invalid analytics query result: {error}"))
    })
}

pub(super) fn push_analytics_filter(
    builder: &mut QueryBuilder<'_, Postgres>,
    query: &UsageAnalyticsQuery,
) {
    push_analytics_source_filter(builder, query, "public.usage_analytics_facts_v1");
}

pub(super) fn push_analytics_source_filter(
    builder: &mut QueryBuilder<'_, Postgres>,
    query: &UsageAnalyticsQuery,
    source: &str,
) {
    builder
        .push(" FROM ")
        .push(source)
        .push(" WHERE created_at >= ")
        .push_bind(
            DateTime::<Utc>::from_timestamp_millis(query.from_unix_ms as i64)
                .expect("validated timestamp"),
        )
        .push(" AND created_at < ")
        .push_bind(
            DateTime::<Utc>::from_timestamp_millis(query.to_unix_ms as i64)
                .expect("validated timestamp"),
        )
        .push(" AND record_kind <> 'session'");
    for (column, value) in [
        ("actor_user_id", &query.actor_user_id),
        ("credential_owner_id", &query.credential_owner_id),
        ("attribution_kind", &query.attribution_kind),
        ("api_key_id", &query.api_key_id),
        ("model", &query.model),
        ("provider_id", &query.provider_id),
        ("api_format", &query.api_format),
        ("endpoint_kind", &query.endpoint_kind),
        ("request_type", &query.request_type),
        ("status", &query.status),
    ] {
        if let Some(value) = value {
            builder
                .push(" AND ")
                .push(column)
                .push(" = ")
                .push_bind(value.clone());
        }
    }
    for (column, value) in [
        ("is_stream", query.is_stream),
        ("has_format_conversion", query.has_format_conversion),
    ] {
        if let Some(value) = value {
            builder
                .push(" AND ")
                .push(column)
                .push(" = ")
                .push_bind(value);
        }
    }
}

fn push_user_filter(builder: &mut QueryBuilder<'_, Postgres>, query: &UsageAnalyticsQuery) {
    builder.push(" WHERE NOT u.is_deleted");
    if let Some(user_id) = query
        .actor_user_id
        .as_ref()
        .or(query.credential_owner_id.as_ref())
    {
        builder.push(" AND u.id = ").push_bind(user_id.clone());
    }
    if let Some(value) = query.user_is_active {
        builder.push(" AND u.is_active = ").push_bind(value);
    }
    if let Some(search) = query.search.as_ref().filter(|value| !value.is_empty()) {
        let pattern = format!(
            "%{}%",
            search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        builder
            .push(" AND (u.username ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR u.email ILIKE ")
            .push_bind(pattern)
            .push(")");
    }
    if let Some(value) = query.has_usage {
        builder.push(if value {
            " AND a.user_id IS NOT NULL"
        } else {
            " AND a.user_id IS NULL"
        });
    }
}

fn push_user_finance_ctes(builder: &mut QueryBuilder<'_, Postgres>, query: &UsageAnalyticsQuery) {
    // Aggregate each financial source before joining the roster. Joining orders
    // directly to request facts would multiply both usage and payment amounts.
    builder.push(r#", wallet_finance AS (
        SELECT w.user_id,
          CASE WHEN bool_and(w.currency = 'USD') THEN round(sum(w.balance::numeric + w.gift_balance::numeric), 8)::text END AS wallet_balance,
          CASE WHEN bool_and(w.currency = 'USD') THEN round(sum(w.balance::numeric), 8)::text END AS recharge_balance,
          CASE WHEN bool_and(w.currency = 'USD') THEN round(sum(w.gift_balance::numeric), 8)::text END AS gift_balance
        FROM wallets w JOIN roster r ON r.user_id = w.user_id GROUP BY w.user_id
      ), credited_orders AS MATERIALIZED (
        SELECT o.id, o.user_id, o.order_no, o.amount_usd, o.payment_method, o.credited_at,
          CASE WHEN o.payment_method IN ('gift_code', 'admin_grant') THEN 'gift_credit'
               WHEN o.order_kind = 'plan_purchase' THEN 'plan_purchase'
               ELSE 'wallet_recharge' END AS kind
        FROM payment_orders o JOIN roster r ON r.user_id = o.user_id
        WHERE o.status = 'credited' AND o.credited_at IS NOT NULL
          AND o.order_kind IN ('wallet_recharge', 'plan_purchase') AND o.credited_at >= "#)
        .push_bind(DateTime::<Utc>::from_timestamp_millis(query.from_unix_ms as i64).expect("validated"))
        .push(" AND o.credited_at < ")
        .push_bind(DateTime::<Utc>::from_timestamp_millis(query.to_unix_ms as i64).expect("validated"))
        .push(r#"), payment_finance AS (
        SELECT user_id,
          round(COALESCE(sum(amount_usd::numeric) FILTER (WHERE kind = 'wallet_recharge'), 0), 8)::text AS recharge_amount,
          count(*) FILTER (WHERE kind = 'wallet_recharge')::bigint AS recharge_count,
          round(COALESCE(sum(amount_usd::numeric) FILTER (WHERE kind = 'plan_purchase'), 0), 8)::text AS plan_purchase_amount,
          count(*) FILTER (WHERE kind = 'plan_purchase')::bigint AS plan_purchase_count,
          round(COALESCE(sum(amount_usd::numeric) FILTER (WHERE kind = 'gift_credit'), 0), 8)::text AS gift_credit_amount,
          count(*) FILTER (WHERE kind = 'gift_credit')::bigint AS gift_credit_count
        FROM credited_orders GROUP BY user_id
      ), enriched AS (
        SELECT r.*, jsonb_build_object(
          'wallet_balance', CASE WHEN w.user_id IS NULL THEN '0.00000000' ELSE w.wallet_balance END,
          'recharge_balance', CASE WHEN w.user_id IS NULL THEN '0.00000000' ELSE w.recharge_balance END,
          'gift_balance', CASE WHEN w.user_id IS NULL THEN '0.00000000' ELSE w.gift_balance END,
          'recharge_amount', COALESCE(p.recharge_amount, '0.00000000'),
          'recharge_count', COALESCE(p.recharge_count, 0),
          'plan_purchase_amount', COALESCE(p.plan_purchase_amount, '0.00000000'),
          'plan_purchase_count', COALESCE(p.plan_purchase_count, 0),
          'gift_credit_amount', COALESCE(p.gift_credit_amount, '0.00000000'),
          'gift_credit_count', COALESCE(p.gift_credit_count, 0)
        ) AS finance
        FROM roster r LEFT JOIN wallet_finance w ON w.user_id = r.user_id
        LEFT JOIN payment_finance p ON p.user_id = r.user_id
      )"#);
}

fn user_finance_summary_sql() -> String {
    let mut fields = Vec::new();
    for field in [
        "wallet_balance",
        "recharge_balance",
        "gift_balance",
        "recharge_amount",
        "plan_purchase_amount",
        "gift_credit_amount",
    ] {
        // If a wallet has an unsupported currency, never disguise the known USD
        // subtotal as a complete balance. An empty supported roster is zero.
        fields.push(format!("'{field}', CASE WHEN count(*) FILTER (WHERE finance->>'{field}' IS NULL) = 0 THEN round(COALESCE(sum((finance->>'{field}')::numeric), 0), 8)::text END"));
    }
    for field in ["recharge_count", "plan_purchase_count", "gift_credit_count"] {
        fields.push(format!(
            "'{field}', COALESCE(sum((finance->>'{field}')::bigint), 0)::bigint"
        ));
    }
    format!("jsonb_build_object({})", fields.join(", "))
}

pub(super) async fn read_analytics_metrics(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    query: &UsageAnalyticsQuery,
    projection_reads: bool,
) -> Result<UsageAnalyticsMetrics, DataLayerError> {
    let metrics_sql = analytics_metrics_sql(query.slow_threshold_ms.unwrap_or(5000));
    let projected = projection_reads && super::projection_reader::supports_projection(query);
    let mut builder = QueryBuilder::<Postgres>::new("SELECT to_jsonb(m) AS metrics FROM (SELECT ");
    builder.push(if projected {
        ANALYTICS_NON_ADDITIVE_METRICS_SQL
    } else {
        &metrics_sql
    });
    push_analytics_filter(&mut builder, query);
    builder.push(") m");
    let row = builder
        .build()
        .fetch_one(&mut **tx)
        .await
        .map_postgres_err()?;
    let mut summary: UsageAnalyticsMetrics = decode(row.try_get("metrics").map_postgres_err()?)?;
    if projected {
        let mut additive = super::projection_reader::read_additive_summary(tx, query).await?;
        additive.usage_active_users = summary.usage_active_users;
        additive.latency_p50_ms = summary.latency_p50_ms;
        additive.latency_p90_ms = summary.latency_p90_ms;
        additive.latency_p95_ms = summary.latency_p95_ms;
        additive.latency_p99_ms = summary.latency_p99_ms;
        additive.first_byte_p90_ms = summary.first_byte_p90_ms;
        additive.first_byte_p99_ms = summary.first_byte_p99_ms;
        summary = additive;
    }
    Ok(summary)
}

impl SqlxUsageReadRepository {
    pub async fn query_usage_analytics(
        &self,
        query: &UsageAnalyticsQuery,
    ) -> Result<StoredUsageAnalytics, DataLayerError> {
        query.validate()?;
        if query.view == UsageAnalyticsView::DashboardCharts {
            return self.query_dashboard_charts(query).await;
        }
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query("SET LOCAL statement_timeout = '15s'")
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        let state = sqlx::query("SELECT pg_current_snapshot()::text AS revision, NOW() AS generated_at, (SELECT count(*) FROM users WHERE is_active AND NOT is_deleted) AS enabled_users")
            .fetch_one(&mut *tx).await.map_postgres_err()?;
        let metrics_sql = analytics_metrics_sql(query.slow_threshold_ms.unwrap_or(5000));
        // The users branch computes a summary over its full filtered roster in
        // the same statement as the page. An installation-wide summary here
        // would be discarded and unnecessarily scan request facts again.
        let mut summary = if query.view == UsageAnalyticsView::Users {
            UsageAnalyticsMetrics::default()
        } else {
            read_analytics_metrics(&mut tx, query, self.overview_projection_reads).await?
        };
        summary.enabled_users = state
            .try_get::<i64, _>("enabled_users")
            .map_postgres_err()? as u64;
        let mut result = StoredUsageAnalytics {
            total: summary.request_count,
            summary,
            read_revision: state.try_get("revision").map_postgres_err()?,
            generated_at: state
                .try_get::<DateTime<Utc>, _>("generated_at")
                .map_postgres_err()?
                .to_rfc3339(),
            // Source freshness is unknown until a collector watermark is available.
            data_through: None,
            ..Default::default()
        };
        result.unrecoverable_bucket_count = sqlx::query_scalar::<_,i64>("SELECT count(DISTINCT bucket_start) FROM (SELECT bucket_start FROM stats_bucket_state WHERE projection_version IN ('overview-v1','overview-v2') AND granularity='hour' AND coverage_status='unrecoverable' UNION ALL SELECT bucket_start FROM stats_overview_dirty_events WHERE projection_version='overview-v2' AND granularity='hour' AND unrecoverable) lost WHERE bucket_start < $2 AND bucket_start + INTERVAL '1 hour' > $1")
            .bind(DateTime::<Utc>::from_timestamp_millis(query.from_unix_ms as i64).expect("validated"))
            .bind(DateTime::<Utc>::from_timestamp_millis(query.to_unix_ms as i64).expect("validated"))
            .fetch_one(&mut *tx).await.map_postgres_err()? as u64;
        result.coverage = super::projection_reader::read_projection_coverage(
            &mut tx,
            query,
            self.overview_projection_reads,
        )
        .await?;
        if query.view != UsageAnalyticsView::Summary {
            let mut builder =
                QueryBuilder::<Postgres>::new("WITH filtered AS MATERIALIZED (SELECT *");
            push_analytics_filter(&mut builder, query);
            builder.push(")");
            let order = if query.descending {
                " DESC NULLS LAST"
            } else {
                " ASC NULLS LAST"
            };
            match query.view {
                UsageAnalyticsView::Timeseries
                | UsageAnalyticsView::Performance
                | UsageAnalyticsView::Breakdown => {
                    let timeseries = query.view != UsageAnalyticsView::Breakdown;
                    // 提供商分组的行需要额外带出名称快照，并在最外层关联提供商目录解析展示名。
                    // 只有明细分组（Breakdown）才需要，时间序列仍然直接用 provider_id 作为标签。
                    let provider_labels =
                        !timeseries && query.group_by == UsageAnalyticsGroupBy::Provider;
                    let group = if timeseries {
                        let granularity = match query.granularity {
                            UsageAnalyticsGranularity::Hour => "hour",
                            UsageAnalyticsGranularity::Day => "day",
                        };
                        // The IANA name was parsed above; it is still bound as a SQL value.
                        let timezone = if query.granularity == UsageAnalyticsGranularity::Hour {
                            "UTC".into()
                        } else {
                            query.timezone.clone()
                        };
                        builder
                            .push(", dated AS (SELECT *, date_trunc('")
                            .push(granularity)
                            .push("', created_at AT TIME ZONE ")
                            .push_bind(timezone.clone())
                            .push(") AT TIME ZONE ")
                            .push_bind(timezone)
                            .push(" AS bucket FROM filtered)");
                        "bucket"
                    } else {
                        match query.group_by {
                            UsageAnalyticsGroupBy::Model => "model",
                            UsageAnalyticsGroupBy::Provider => "provider_id",
                            UsageAnalyticsGroupBy::ApiKey => "api_key_id",
                            UsageAnalyticsGroupBy::Attribution => "attribution_kind",
                            UsageAnalyticsGroupBy::ApiFormat => "api_format",
                            UsageAnalyticsGroupBy::RequestType => "request_type",
                        }
                    };
                    builder
                        .push(", grouped AS (SELECT ")
                        .push(group)
                        .push(" AS group_id, ")
                        .push(if provider_labels {
                            "max(provider_name) AS provider_name, "
                        } else {
                            ""
                        })
                        .push(&metrics_sql)
                        .push(if timeseries {
                            " FROM dated GROUP BY "
                        } else {
                            " FROM filtered GROUP BY "
                        })
                        .push(group)
                        .push("), page AS (SELECT * FROM grouped ORDER BY ");
                    if timeseries {
                        builder.push("group_id ASC");
                    } else {
                        builder
                            .push(match query.sort {
                                UsageAnalyticsSort::BillableAmount => "billable_amount::numeric",
                                UsageAnalyticsSort::Tokens => "total_tokens",
                                _ => "request_count",
                            })
                            .push(order)
                            .push(", group_id ASC NULLS LAST");
                    }
                    builder.push(" LIMIT ").push_bind(if timeseries { 10_001 } else { i64::from(query.limit) }).push(" OFFSET ").push_bind(if timeseries { 0 } else { query.offset as i64 })
                        .push(") SELECT (SELECT count(*) FROM grouped) AS total, COALESCE(jsonb_agg(jsonb_build_object('id', page.group_id::text, 'label', ")
                        .push(if provider_labels { ANALYTICS_PROVIDER_LABEL_SQL } else { "page.group_id::text" })
                        .push(", 'bucket_start', ")
                        .push(if timeseries { "to_char(group_id AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')" } else { "NULL" })
                        .push(", 'metrics', to_jsonb(page) - 'group_id'")
                        .push(if provider_labels {
                            " - 'provider_name'"
                        } else {
                            ""
                        })
                        .push(")), '[]'::jsonb) AS items FROM page")
                        .push(if provider_labels {
                            " LEFT JOIN public.providers AS provider_catalog ON provider_catalog.id = page.group_id"
                        } else {
                            ""
                        });
                    let row = builder
                        .build()
                        .fetch_one(&mut *tx)
                        .await
                        .map_postgres_err()?;
                    result.total = row.try_get::<i64, _>("total").map_postgres_err()? as u64;
                    result.rows = decode(row.try_get("items").map_postgres_err()?)?;
                }
                UsageAnalyticsView::Users => {
                    let scoped_payments =
                        query.actor_user_id.is_some() || query.credential_owner_id.is_some();
                    let subject = if query.actor_user_id.is_some()
                        || query.attribution_kind.as_deref() == Some("employee")
                    {
                        "actor_user_id"
                    } else {
                        "credential_owner_id"
                    };
                    builder.push(", aggregated AS (SELECT ").push(subject).push(" AS user_id, max(created_at) AS last_used_at, count(DISTINCT (created_at AT TIME ZONE ")
                        .push_bind(query.timezone.clone()).push(")::date)::bigint AS active_days, ").push(&metrics_sql)
                        .push(" FROM filtered GROUP BY ").push(subject).push("), roster AS (SELECT u.id AS user_id, u.username, u.email, u.is_active, a.last_used_at,")
                        .push(" COALESCE(a.active_days, 0) AS active_days, COALESCE(to_jsonb(a) - 'user_id' - 'last_used_at' - 'active_days', '{}'::jsonb) AS metrics, COALESCE(a.request_count, 0) AS requests, a.billable_amount::numeric AS billable FROM users u LEFT JOIN aggregated a ON a.user_id = u.id");
                    push_user_filter(&mut builder, query);
                    builder.push(")");
                    push_user_finance_ctes(&mut builder, query);
                    builder.push(", page AS (SELECT * FROM enriched ORDER BY ")
                        .push(match query.sort { UsageAnalyticsSort::Requests => "requests", UsageAnalyticsSort::BillableAmount => "billable", UsageAnalyticsSort::LastUsed | UsageAnalyticsSort::StartedAt => "last_used_at", UsageAnalyticsSort::Username => "username", UsageAnalyticsSort::Tokens => "(metrics->>'total_tokens')::bigint", UsageAnalyticsSort::ActiveDays => "active_days" })
                        .push(order).push(", user_id ASC LIMIT ").push_bind(i64::from(query.limit)).push(" OFFSET ").push_bind(query.offset as i64)
                        .push(") SELECT (SELECT count(*) FROM roster) AS total, (SELECT count(*) FROM roster WHERE requests > 0) AS active_user_count, (SELECT count(*) FROM roster WHERE is_active) AS enabled_user_count, ")
                        .push("(SELECT to_jsonb(m) FROM (SELECT ").push(&metrics_sql)
                        .push(" FROM filtered WHERE ").push(subject).push(" IN (SELECT user_id FROM roster)) m) AS summary, ")
                        .push("(SELECT ").push(user_finance_summary_sql()).push(" FROM enriched) AS finance_summary, ")
                        .push("(SELECT COALESCE(jsonb_agg(to_jsonb(p)), '[]'::jsonb) FROM (SELECT id, order_no, kind, round(amount_usd::numeric, 8)::text AS amount, payment_method, to_char(credited_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS credited_at FROM credited_orders ORDER BY credited_at DESC, id ASC LIMIT ")
                        .push_bind(if scoped_payments { i64::from(query.payment_limit.unwrap_or(10)) } else { 0 })
                        .push(" OFFSET ").push_bind(query.payment_offset.unwrap_or(0) as i64)
                        .push(") p) AS payments, (SELECT count(*) FROM credited_orders) AS payment_total, ")
                        .push("COALESCE(jsonb_agg(to_jsonb(page) - 'requests' - 'billable'), '[]'::jsonb) AS items FROM page");
                    let row = builder
                        .build()
                        .fetch_one(&mut *tx)
                        .await
                        .map_postgres_err()?;
                    result.total = row.try_get::<i64, _>("total").map_postgres_err()? as u64;
                    result.users = decode(row.try_get("items").map_postgres_err()?)?;
                    result.summary = decode(row.try_get("summary").map_postgres_err()?)?;
                    result.summary.enabled_users =
                        row.try_get::<i64, _>("enabled_user_count")
                            .map_postgres_err()? as u64;
                    result.user_summary = Some(UsageAnalyticsUserSummary {
                        user_count: result.total,
                        active_user_count: row
                            .try_get::<i64, _>("active_user_count")
                            .map_postgres_err()? as u64,
                        metrics: result.summary.clone(),
                    });
                    result.user_finance_summary =
                        Some(decode(row.try_get("finance_summary").map_postgres_err()?)?);
                    if scoped_payments {
                        result.user_payments = Some(UsageAnalyticsUserPayments {
                            items: decode(row.try_get("payments").map_postgres_err()?)?,
                            total: row.try_get::<i64, _>("payment_total").map_postgres_err()?
                                as u64,
                            limit: query.payment_limit.unwrap_or(10),
                            offset: query.payment_offset.unwrap_or(0),
                        });
                    }
                }
                UsageAnalyticsView::Consumption => {
                    builder.push(", page AS (SELECT id, request_id, created_at AS started_at, actor_user_id AS user_id, credential_owner_id, model, provider_name AS provider, provider_id, api_key_id, status, settlement_status, attribution_kind, attribution_source, rated_amount::text, billable_amount::text, quota_covered_amount::text, wallet_consumed_amount::text, wallet_debit_amount::text FROM filtered ORDER BY created_at ").push(order).push(", request_id ASC LIMIT ")
                        .push_bind(i64::from(query.limit)).push(" OFFSET ").push_bind(query.offset as i64)
                        .push(") SELECT COALESCE(jsonb_agg(to_jsonb(page)), '[]'::jsonb) AS items FROM page");
                    let row = builder
                        .build()
                        .fetch_one(&mut *tx)
                        .await
                        .map_postgres_err()?;
                    result.consumption = decode(row.try_get("items").map_postgres_err()?)?;
                }
                UsageAnalyticsView::Summary | UsageAnalyticsView::DashboardCharts => unreachable!(),
            }
        }
        if matches!(
            query.view,
            UsageAnalyticsView::Timeseries | UsageAnalyticsView::Performance
        ) {
            fill_usage_analytics_timeseries(query, &mut result.rows);
            result.total = result.rows.len() as u64;
        }
        if query.view == UsageAnalyticsView::Performance {
            let mut providers = QueryBuilder::<Postgres>::new("SELECT COALESCE(jsonb_agg(jsonb_build_object('id',provider_id,'label',provider_label,'bucket_start',NULL,'metrics',to_jsonb(m)-'provider_id'-'provider_label')), '[]'::jsonb) AS items FROM (SELECT provider_id, max(provider_name) AS provider_label, ");
            providers.push(&metrics_sql);
            push_analytics_filter(&mut providers, query);
            providers.push(" GROUP BY provider_id ORDER BY count(*) DESC,provider_id");
            providers.push(") m");
            let row = providers
                .build()
                .fetch_one(&mut *tx)
                .await
                .map_postgres_err()?;
            result.provider_rows = decode(row.try_get("items").map_postgres_err()?)?;
        }
        if query.view == UsageAnalyticsView::Performance {
            // Aggregate requested models across providers in the same read snapshot.
            // Like provider rows, retain raw samples for exact percentiles even when
            // the summary combines verified hourly projections and raw gaps.
            let mut models = QueryBuilder::<Postgres>::new("SELECT COALESCE(jsonb_agg(jsonb_build_object('id',model,'label',model,'bucket_start',NULL,'metrics',to_jsonb(m)-'model')), '[]'::jsonb) AS items FROM (SELECT model, ");
            models.push(&metrics_sql);
            push_analytics_filter(&mut models, query);
            models.push(" GROUP BY model ORDER BY count(*) DESC,model NULLS LAST) m");
            let row = models
                .build()
                .fetch_one(&mut *tx)
                .await
                .map_postgres_err()?;
            result.model_rows = decode(row.try_get("items").map_postgres_err()?)?;

            let mut timeline = QueryBuilder::<Postgres>::new("WITH filtered AS (SELECT *");
            push_analytics_filter(&mut timeline, query);
            timeline.push("), dated AS (SELECT *, date_trunc(")
                .push_bind(if query.granularity == UsageAnalyticsGranularity::Hour { "hour" } else { "day" })
                .push(", created_at AT TIME ZONE ").push_bind(if query.granularity == UsageAnalyticsGranularity::Hour { "UTC".into() } else { query.timezone.clone() })
                .push(") AT TIME ZONE ").push_bind(if query.granularity == UsageAnalyticsGranularity::Hour { "UTC".into() } else { query.timezone.clone() })
                .push(" AS bucket FROM filtered) SELECT COALESCE(jsonb_agg(jsonb_build_object('id',provider_id,'label',provider_label,'bucket_start',to_char(bucket AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'),'metrics',to_jsonb(m)-'provider_id'-'provider_label'-'bucket')),'[]'::jsonb) AS items FROM (SELECT provider_id,max(provider_name) AS provider_label,bucket,")
                .push(&metrics_sql).push(" FROM dated GROUP BY provider_id,bucket ORDER BY bucket,provider_id) m");
            let row = timeline
                .build()
                .fetch_one(&mut *tx)
                .await
                .map_postgres_err()?;
            result.provider_timeline_rows = decode(row.try_get("items").map_postgres_err()?)?;
            let mut errors = QueryBuilder::<Postgres>::new("SELECT COALESCE(jsonb_agg(to_jsonb(m)), '[]'::jsonb) AS items FROM (SELECT COALESCE(failure_reason,error_category,'unknown') AS reason,count(*)::bigint AS count");
            push_analytics_filter(&mut errors, query);
            errors.push(" AND status='failed' GROUP BY COALESCE(failure_reason,error_category,'unknown') ORDER BY count(*) DESC) m");
            let row = errors
                .build()
                .fetch_one(&mut *tx)
                .await
                .map_postgres_err()?;
            result.errors = decode(row.try_get("items").map_postgres_err()?)?;
        }
        tx.commit().await.map_postgres_err()?;
        Ok(result)
    }
}
