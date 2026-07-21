pub(super) const SELECT_LATEST_STATS_HOURLY_HOUR_SQL: &str = r#"
SELECT MAX(hour_utc) AS latest_hour
FROM stats_hourly
WHERE is_complete IS TRUE
"#;
pub(super) const SELECT_NEXT_STATS_HOURLY_BUCKET_SQL: &str = r#"
SELECT date_trunc('hour', MIN(created_at)) AS next_bucket
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;
pub(super) const SELECT_STATS_HOURLY_AGGREGATE_SQL: &str = r#"
SELECT
    CAST(COUNT(usage.id) AS BIGINT) AS cache_hit_total_requests,
    CAST(
        COUNT(usage.id) FILTER (
            WHERE GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) > 0
        ) AS BIGINT
    ) AS cache_hit_requests,
    CAST(
        COUNT(usage.id) FILTER (WHERE usage.status = 'completed') AS BIGINT
    ) AS completed_total_requests,
    CAST(
        COUNT(usage.id) FILTER (
            WHERE usage.status = 'completed'
              AND GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) > 0
        ) AS BIGINT
    ) AS completed_cache_hit_requests,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0))
                FILTER (WHERE usage.status = 'completed'),
            0
        ) AS BIGINT
    ) AS completed_input_tokens,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0))
                FILTER (WHERE usage.status = 'completed'),
            0
        ) AS BIGINT
    ) AS completed_cache_creation_tokens,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0))
                FILTER (WHERE usage.status = 'completed'),
            0
        ) AS BIGINT
    ) AS completed_cache_read_tokens,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.total_input_context, 0), 0))
                FILTER (WHERE usage.status = 'completed'),
            0
        ) AS BIGINT
    ) AS completed_total_input_context,
    CAST(
        COALESCE(
            SUM(
                COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)
            ) FILTER (WHERE usage.status = 'completed'),
            0
        ) AS DOUBLE PRECISION
    ) AS completed_cache_creation_cost,
    CAST(
        COALESCE(
            SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0))
                FILTER (WHERE usage.status = 'completed'),
            0
        ) AS DOUBLE PRECISION
    ) AS completed_cache_read_cost,
    CAST(
        COALESCE(
            SUM(COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0)) FILTER (
                WHERE usage.billing_status = 'settled'
                  AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
            ),
            0
        ) AS DOUBLE PRECISION
    ) AS settled_total_cost,
    CAST(
        COUNT(usage.id) FILTER (
            WHERE usage.billing_status = 'settled'
              AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
        ) AS BIGINT
    ) AS settled_total_requests,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)) FILTER (
                WHERE usage.billing_status = 'settled'
                  AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
            ),
            0
        ) AS BIGINT
    ) AS settled_input_tokens,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.output_tokens, 0), 0)) FILTER (
                WHERE usage.billing_status = 'settled'
                  AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
            ),
            0
        ) AS BIGINT
    ) AS settled_output_tokens,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0)) FILTER (
                WHERE usage.billing_status = 'settled'
                  AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
            ),
            0
        ) AS BIGINT
    ) AS settled_cache_creation_tokens,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)) FILTER (
                WHERE usage.billing_status = 'settled'
                  AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
            ),
            0
        ) AS BIGINT
    ) AS settled_cache_read_tokens,
    MIN(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT)) FILTER (
        WHERE usage.billing_status = 'settled'
          AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_first_finalized_at_unix_secs,
    MAX(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT)) FILTER (
        WHERE usage.billing_status = 'settled'
          AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_last_finalized_at_unix_secs,
    CAST(
        COUNT(usage.id) FILTER (
            WHERE usage.status NOT IN ('pending', 'streaming')
              AND usage.provider_name NOT IN ('unknown', 'pending')
        ) AS BIGINT
    ) AS total_requests,
    CAST(COALESCE(
        SUM(
            CASE
                WHEN usage.status_code >= 400
                     OR lower(COALESCE(usage.status, '')) = 'failed'
                     OR usage.error_message IS NOT NULL THEN 1
                ELSE 0
            END
        ) FILTER (
            WHERE usage.status NOT IN ('pending', 'streaming')
              AND usage.provider_name NOT IN ('unknown', 'pending')
        ),
        0
    ) AS BIGINT) AS error_requests,
    CAST(
        COALESCE(
            SUM(usage.input_tokens) FILTER (
                WHERE usage.status NOT IN ('pending', 'streaming')
                  AND usage.provider_name NOT IN ('unknown', 'pending')
            ),
            0
        ) AS BIGINT
    ) AS input_tokens,
    CAST(
        COALESCE(
            SUM(usage.output_tokens) FILTER (
                WHERE usage.status NOT IN ('pending', 'streaming')
                  AND usage.provider_name NOT IN ('unknown', 'pending')
            ),
            0
        ) AS BIGINT
    ) AS output_tokens,
    CAST(
        COALESCE(
            SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0)) FILTER (
                WHERE usage.status NOT IN ('pending', 'streaming')
                  AND usage.provider_name NOT IN ('unknown', 'pending')
            ),
            0
        ) AS BIGINT
    ) AS cache_creation_tokens,
    CAST(
        COALESCE(
            SUM(usage.cache_read_input_tokens) FILTER (
                WHERE usage.status NOT IN ('pending', 'streaming')
                  AND usage.provider_name NOT IN ('unknown', 'pending')
            ),
            0
        ) AS BIGINT
    ) AS cache_read_tokens,
    CAST(
        COALESCE(
            SUM(usage.total_cost_usd) FILTER (
                WHERE usage.status NOT IN ('pending', 'streaming')
                  AND usage.provider_name NOT IN ('unknown', 'pending')
            ),
            0
        ) AS DOUBLE PRECISION
    ) AS total_cost,
    CAST(
        COALESCE(
            SUM(usage.actual_total_cost_usd) FILTER (
                WHERE usage.status NOT IN ('pending', 'streaming')
                  AND usage.provider_name NOT IN ('unknown', 'pending')
            ),
            0
        ) AS DOUBLE PRECISION
    ) AS actual_total_cost,
    COALESCE(
        SUM(
            CASE
                WHEN usage.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ) FILTER (
            WHERE usage.status NOT IN ('pending', 'streaming')
              AND usage.provider_name NOT IN ('unknown', 'pending')
        ),
        0
    ) AS response_time_sum_ms,
    CAST(COALESCE(
        SUM(
            CASE
                WHEN usage.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ) FILTER (
            WHERE usage.status NOT IN ('pending', 'streaming')
              AND usage.provider_name NOT IN ('unknown', 'pending')
        ),
        0
    ) AS BIGINT) AS response_time_samples,
    CAST(
        COALESCE(
            AVG(usage.response_time_ms) FILTER (
                WHERE usage.status NOT IN ('pending', 'streaming')
                  AND usage.provider_name NOT IN ('unknown', 'pending')
            ),
            0
        ) AS DOUBLE PRECISION
    ) AS avg_response_time_ms
FROM usage_billing_facts AS usage
WHERE usage.created_at >= $1
  AND usage.created_at < $2
"#;
pub(super) const UPSERT_STATS_HOURLY_SQL: &str = r#"
INSERT INTO stats_hourly (
    id,
    hour_utc,
    total_requests,
    cache_hit_total_requests,
    cache_hit_requests,
    completed_total_requests,
    completed_cache_hit_requests,
    completed_input_tokens,
    completed_cache_creation_tokens,
    completed_cache_read_tokens,
    completed_total_input_context,
    completed_cache_creation_cost,
    completed_cache_read_cost,
    settled_total_cost,
    settled_total_requests,
    settled_input_tokens,
    settled_output_tokens,
    settled_cache_creation_tokens,
    settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    is_complete,
    aggregated_at,
    created_at,
    updated_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    $9, $10, $11, $12, $13, $14, $15, $16,
    $17, $18, $19, $20, $21, $22, $23, $24,
    $25, $26, $27, $28, $29, $30, $31, $32,
    $33, $34, $35, $36
)
ON CONFLICT (hour_utc)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    cache_hit_total_requests = EXCLUDED.cache_hit_total_requests,
    cache_hit_requests = EXCLUDED.cache_hit_requests,
    completed_total_requests = EXCLUDED.completed_total_requests,
    completed_cache_hit_requests = EXCLUDED.completed_cache_hit_requests,
    completed_input_tokens = EXCLUDED.completed_input_tokens,
    completed_cache_creation_tokens = EXCLUDED.completed_cache_creation_tokens,
    completed_cache_read_tokens = EXCLUDED.completed_cache_read_tokens,
    completed_total_input_context = EXCLUDED.completed_total_input_context,
    completed_cache_creation_cost = EXCLUDED.completed_cache_creation_cost,
    completed_cache_read_cost = EXCLUDED.completed_cache_read_cost,
    settled_total_cost = EXCLUDED.settled_total_cost,
    settled_total_requests = EXCLUDED.settled_total_requests,
    settled_input_tokens = EXCLUDED.settled_input_tokens,
    settled_output_tokens = EXCLUDED.settled_output_tokens,
    settled_cache_creation_tokens = EXCLUDED.settled_cache_creation_tokens,
    settled_cache_read_tokens = EXCLUDED.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = EXCLUDED.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = EXCLUDED.settled_last_finalized_at_unix_secs,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    is_complete = EXCLUDED.is_complete,
    aggregated_at = EXCLUDED.aggregated_at,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_HOURLY_USER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN status_code >= 400
                         OR lower(COALESCE(status, '')) = 'failed'
                         OR error_message IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(
            SUM(GREATEST(COALESCE(cache_creation_input_tokens, 0), 0)),
            0
        ) AS BIGINT) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS settled_total_cost,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_total_requests,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(input_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_input_tokens,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(output_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_output_tokens,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(cache_creation_input_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_cache_creation_tokens,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_cache_read_tokens,
        MIN(
            CASE
                WHEN billing_status = 'settled'
                     AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                     AND finalized_at IS NOT NULL
                THEN CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT)
                ELSE NULL
            END
        ) AS settled_first_finalized_at_unix_secs,
        MAX(
            CASE
                WHEN billing_status = 'settled'
                     AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                     AND finalized_at IS NOT NULL
                THEN CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT)
                ELSE NULL
            END
        ) AS settled_last_finalized_at_unix_secs,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id
)
INSERT INTO stats_hourly_user (
    id,
    hour_utc,
    user_id,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    settled_total_cost,
    settled_total_requests,
    settled_input_tokens,
    settled_output_tokens,
    settled_cache_creation_tokens,
    settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-user:', aggregated.user_id, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.user_id,
    aggregated.total_requests,
    GREATEST(aggregated.total_requests - aggregated.error_requests, 0),
    aggregated.error_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.settled_total_cost,
    aggregated.settled_total_requests,
    aggregated.settled_input_tokens,
    aggregated.settled_output_tokens,
    aggregated.settled_cache_creation_tokens,
    aggregated.settled_cache_read_tokens,
    aggregated.settled_first_finalized_at_unix_secs,
    aggregated.settled_last_finalized_at_unix_secs,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (hour_utc, user_id)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    settled_total_cost = EXCLUDED.settled_total_cost,
    settled_total_requests = EXCLUDED.settled_total_requests,
    settled_input_tokens = EXCLUDED.settled_input_tokens,
    settled_output_tokens = EXCLUDED.settled_output_tokens,
    settled_cache_creation_tokens = EXCLUDED.settled_cache_creation_tokens,
    settled_cache_read_tokens = EXCLUDED.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = EXCLUDED.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = EXCLUDED.settled_last_finalized_at_unix_secs,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_HOURLY_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS response_time_samples,
        CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND model IS NOT NULL
      AND model <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY model
)
INSERT INTO stats_hourly_model (
    id,
    hour_utc,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-model:', aggregated.model, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.model,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.avg_response_time_ms,
    $3,
    $3
FROM aggregated
ON CONFLICT (hour_utc, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_HOURLY_USER_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND model IS NOT NULL
      AND model <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, model
)
INSERT INTO stats_hourly_user_model (
    id,
    hour_utc,
    user_id,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-user-model:', aggregated.user_id, ':', aggregated.model, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.user_id,
    aggregated.model,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (hour_utc, user_id, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_HOURLY_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY provider_name
)
INSERT INTO stats_hourly_provider (
    id,
    hour_utc,
    provider_name,
    total_requests,
    input_tokens,
    output_tokens,
    total_cost,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-provider:', aggregated.provider_name, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.total_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (hour_utc, provider_name)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;

#[cfg(test)]
mod tests {
    use super::{SELECT_STATS_HOURLY_AGGREGATE_SQL, UPSERT_STATS_HOURLY_USER_SQL};

    #[test]
    fn hourly_aggregate_uses_one_time_bounded_base_table_scan() {
        let sql = SELECT_STATS_HOURLY_AGGREGATE_SQL;

        assert_eq!(sql.matches("FROM usage_billing_facts AS usage").count(), 1);
        assert_eq!(sql.matches("WHERE usage.created_at >= $1").count(), 1);
        assert_eq!(sql.matches("AND usage.created_at < $2").count(), 1);
        assert!(!sql.contains("MATERIALIZED"));
        assert!(!sql.contains("bucket_usage"));
    }

    #[test]
    fn hourly_aggregate_keeps_independent_row_populations() {
        let sql = SELECT_STATS_HOURLY_AGGREGATE_SQL;

        assert_eq!(sql.matches("usage.status = 'completed'").count(), 8);
        assert_eq!(sql.matches("usage.billing_status = 'settled'").count(), 8);
        assert_eq!(
            sql.matches("COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0")
                .count(),
            8
        );
        assert_eq!(
            sql.matches("usage.status NOT IN ('pending', 'streaming')")
                .count(),
            11
        );
        assert_eq!(
            sql.matches("usage.provider_name NOT IN ('unknown', 'pending')")
                .count(),
            11
        );

        let first_aggregate = sql
            .trim_start()
            .strip_prefix("SELECT\n")
            .expect("hourly aggregate should remain a direct SELECT");
        assert!(first_aggregate
            .starts_with("    CAST(COUNT(usage.id) AS BIGINT) AS cache_hit_total_requests,"));
        assert!(
            sql.contains("MIN(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT)) FILTER (")
        );
        assert!(
            sql.contains("MAX(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT)) FILTER (")
        );
    }

    #[test]
    fn hourly_aggregate_preserves_the_decoder_column_contract() {
        let sql = SELECT_STATS_HOURLY_AGGREGATE_SQL;
        let aliases = [
            "cache_hit_total_requests",
            "cache_hit_requests",
            "completed_total_requests",
            "completed_cache_hit_requests",
            "completed_input_tokens",
            "completed_cache_creation_tokens",
            "completed_cache_read_tokens",
            "completed_total_input_context",
            "completed_cache_creation_cost",
            "completed_cache_read_cost",
            "settled_total_cost",
            "settled_total_requests",
            "settled_input_tokens",
            "settled_output_tokens",
            "settled_cache_creation_tokens",
            "settled_cache_read_tokens",
            "settled_first_finalized_at_unix_secs",
            "settled_last_finalized_at_unix_secs",
            "total_requests",
            "error_requests",
            "input_tokens",
            "output_tokens",
            "cache_creation_tokens",
            "cache_read_tokens",
            "total_cost",
            "actual_total_cost",
            "response_time_sum_ms",
            "response_time_samples",
            "avg_response_time_ms",
        ];

        for alias in aliases {
            assert_eq!(
                sql.matches(&format!(" AS {alias}")).count(),
                1,
                "hourly aggregate alias {alias} must appear exactly once"
            );
        }
    }

    #[test]
    fn hourly_aggregates_do_not_renormalize_billing_fact_tokens() {
        let aggregate_sql = SELECT_STATS_HOURLY_AGGREGATE_SQL;
        let user_sql = UPSERT_STATS_HOURLY_USER_SQL;

        assert_eq!(
            aggregate_sql.matches("usage.total_input_context").count(),
            1
        );
        assert!(aggregate_sql.contains(
            "SUM(GREATEST(COALESCE(usage.total_input_context, 0), 0))\n                FILTER (WHERE usage.status = 'completed')"
        ));
        assert_eq!(
            aggregate_sql
                .matches("SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0))")
                .count(),
            3
        );
        assert!(user_sql.contains("SUM(GREATEST(COALESCE(cache_creation_input_tokens, 0), 0))"));

        for sql in [aggregate_sql, user_sql] {
            assert!(!sql.contains("split_part("));
            assert!(!sql.contains("cache_creation_input_tokens_5m"));
            assert!(!sql.contains("cache_creation_input_tokens_1h"));
            assert!(!sql.contains("- GREATEST(COALESCE("));
        }
    }
}
