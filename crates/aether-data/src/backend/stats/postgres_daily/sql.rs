pub(super) const SELECT_STATS_DAILY_AGGREGATE_SQL: &str = r#"
WITH bucket_usage AS MATERIALIZED (
    SELECT *
    FROM usage_billing_facts
    WHERE created_at >= $1
      AND created_at < $2
)
SELECT
    (
        SELECT CAST(COUNT(cache_hit_usage.id) AS BIGINT)
        FROM bucket_usage AS cache_hit_usage
        WHERE cache_hit_usage.created_at >= $1
          AND cache_hit_usage.created_at < $2
    ) AS cache_hit_total_requests,
    (
        SELECT CAST(
            COUNT(cache_hit_usage.id) FILTER (
                WHERE GREATEST(COALESCE(cache_hit_usage.cache_read_input_tokens, 0), 0) > 0
            ) AS BIGINT
        )
        FROM bucket_usage AS cache_hit_usage
        WHERE cache_hit_usage.created_at >= $1
          AND cache_hit_usage.created_at < $2
    ) AS cache_hit_requests,
    (
        SELECT CAST(COUNT(completed_usage.id) AS BIGINT)
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_total_requests,
    (
        SELECT CAST(
            COUNT(completed_usage.id) FILTER (
                WHERE GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0) > 0
            ) AS BIGINT
        )
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_hit_requests,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_input_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(completed_usage.cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                           + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(completed_usage.cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        )
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_creation_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_read_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(
                            lower(
                                COALESCE(
                                    COALESCE(
                                        completed_usage.endpoint_api_format,
                                        completed_usage.api_format
                                    ),
                                    ''
                                )
                            ),
                            ':',
                            1
                        ) IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(completed_usage.cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                        + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                    + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(completed_usage.cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        WHEN split_part(
                            lower(
                                COALESCE(
                                    COALESCE(
                                        completed_usage.endpoint_api_format,
                                        completed_usage.api_format
                                    ),
                                    ''
                                )
                            ),
                            ':',
                            1
                        ) IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0) <= 0
                                THEN 0
                                WHEN GREATEST(
                                    COALESCE(completed_usage.cache_read_input_tokens, 0),
                                    0
                                ) <= 0
                                THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                        - GREATEST(
                                            COALESCE(completed_usage.cache_read_input_tokens, 0),
                                            0
                                        ),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(
                                        completed_usage.cache_creation_input_tokens,
                                        0
                                    ) = 0
                                         AND (
                                            COALESCE(
                                                completed_usage.cache_creation_input_tokens_5m,
                                                0
                                            )
                                            + COALESCE(
                                                completed_usage.cache_creation_input_tokens_1h,
                                                0
                                            )
                                         ) > 0
                                    THEN COALESCE(
                                        completed_usage.cache_creation_input_tokens_5m,
                                        0
                                    )
                                        + COALESCE(
                                            completed_usage.cache_creation_input_tokens_1h,
                                            0
                                        )
                                    ELSE COALESCE(
                                        completed_usage.cache_creation_input_tokens,
                                        0
                                    )
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(
                                            completed_usage.cache_creation_input_tokens,
                                            0
                                        ) = 0
                                             AND (
                                                COALESCE(
                                                    completed_usage.cache_creation_input_tokens_5m,
                                                    0
                                                )
                                                + COALESCE(
                                                    completed_usage.cache_creation_input_tokens_1h,
                                                    0
                                                )
                                             ) > 0
                                        THEN COALESCE(
                                            completed_usage.cache_creation_input_tokens_5m,
                                            0
                                        )
                                            + COALESCE(
                                                completed_usage.cache_creation_input_tokens_1h,
                                                0
                                            )
                                        ELSE COALESCE(
                                            completed_usage.cache_creation_input_tokens,
                                            0
                                        )
                                    END
                                )
                                + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        )
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_total_input_context,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(completed_usage.cache_creation_cost_usd AS DOUBLE PRECISION),
                        0
                    )
                ),
                0
            ) AS DOUBLE PRECISION
        )
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_creation_cost,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    COALESCE(CAST(completed_usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)
                ),
                0
            ) AS DOUBLE PRECISION
        )
        FROM bucket_usage AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_read_cost,
    (
        SELECT CAST(
            COALESCE(SUM(COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0)), 0)
                AS DOUBLE PRECISION
        )
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_total_cost,
    (
        SELECT CAST(COUNT(settled_usage.id) AS BIGINT)
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_total_requests,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(settled_usage.input_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_input_tokens,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(settled_usage.output_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_output_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(settled_usage.cache_creation_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_cache_creation_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(settled_usage.cache_read_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_cache_read_tokens,
    (
        SELECT MIN(CAST(EXTRACT(EPOCH FROM settled_usage.finalized_at) AS BIGINT))
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_first_finalized_at_unix_secs,
    (
        SELECT MAX(CAST(EXTRACT(EPOCH FROM settled_usage.finalized_at) AS BIGINT))
        FROM bucket_usage AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_last_finalized_at_unix_secs,
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN status_code >= 400
                         OR lower(COALESCE(status, '')) = 'failed'
                         OR error_message IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT
    ) AS error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                    WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                    THEN GREATEST(COALESCE(input_tokens, 0), 0)
                    WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                         IN ('openai', 'gemini', 'google')
                    THEN GREATEST(
                        GREATEST(COALESCE(input_tokens, 0), 0)
                            - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                        0
                    )
                    ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                END
            ),
            0
        ) AS BIGINT
    ) AS effective_input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                         AND (
                            COALESCE(cache_creation_input_tokens_5m, 0)
                            + COALESCE(cache_creation_input_tokens_1h, 0)
                         ) > 0
                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                       + COALESCE(cache_creation_input_tokens_1h, 0)
                    ELSE COALESCE(cache_creation_input_tokens, 0)
                END
            ),
            0
        ) AS BIGINT
    ) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
        AS cache_creation_ephemeral_5m_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
        AS cache_creation_ephemeral_1h_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                         IN ('claude', 'anthropic')
                    THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        + CASE
                            WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                 AND (
                                    COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                 ) > 0
                            THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                            ELSE COALESCE(cache_creation_input_tokens, 0)
                          END
                        + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                         IN ('openai', 'gemini', 'google')
                    THEN (
                        CASE
                            WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                            WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            ELSE GREATEST(
                                GREATEST(COALESCE(input_tokens, 0), 0)
                                    - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                0
                            )
                        END
                    ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    ELSE CASE
                        WHEN (
                            CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                            END
                        ) > 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                              )
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    END
                END
            ),
            0
        ) AS BIGINT
    ) AS total_input_context,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
    CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
    CAST(COALESCE(SUM(input_cost_usd), 0) AS DOUBLE PRECISION) AS input_cost,
    CAST(COALESCE(SUM(output_cost_usd), 0) AS DOUBLE PRECISION) AS output_cost,
    CAST(COALESCE(SUM(cache_creation_cost_usd), 0) AS DOUBLE PRECISION) AS cache_creation_cost,
    CAST(COALESCE(SUM(cache_read_cost_usd), 0) AS DOUBLE PRECISION) AS cache_read_cost,
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
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT
    ) AS response_time_samples,
    CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms,
    CAST(COUNT(DISTINCT model) AS BIGINT) AS unique_models,
    CAST(COUNT(DISTINCT provider_name) AS BIGINT) AS unique_providers
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;
pub(super) const SELECT_STATS_DAILY_FALLBACK_COUNT_SQL: &str = r#"
SELECT CAST(COUNT(*) AS BIGINT) AS fallback_count
FROM (
    SELECT request_id
    FROM request_candidates
    WHERE created_at >= $1
      AND created_at < $2
      AND status = ANY($3)
    GROUP BY request_id
    HAVING COUNT(id) > 1
) AS fallback_requests
"#;
pub(super) const SELECT_STATS_DAILY_RESPONSE_TIME_PERCENTILES_SQL: &str = r#"
SELECT
    CAST(COUNT(*) AS BIGINT) AS sample_count,
    CAST(percentile_cont(0.5) WITHIN GROUP (ORDER BY response_time_ms) AS DOUBLE PRECISION) AS p50,
    CAST(percentile_cont(0.9) WITHIN GROUP (ORDER BY response_time_ms) AS DOUBLE PRECISION) AS p90,
    CAST(percentile_cont(0.99) WITHIN GROUP (ORDER BY response_time_ms) AS DOUBLE PRECISION) AS p99
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status = 'completed'
  AND provider_name NOT IN ('unknown', 'pending')
  AND response_time_ms IS NOT NULL
"#;
pub(super) const SELECT_STATS_DAILY_FIRST_BYTE_PERCENTILES_SQL: &str = r#"
SELECT
    CAST(COUNT(*) AS BIGINT) AS sample_count,
    CAST(percentile_cont(0.5) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p50,
    CAST(percentile_cont(0.9) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p90,
    CAST(percentile_cont(0.99) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p99
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status = 'completed'
  AND provider_name NOT IN ('unknown', 'pending')
  AND first_byte_time_ms IS NOT NULL
"#;
pub(super) const UPSERT_STATS_DAILY_SQL: &str = r#"
INSERT INTO stats_daily (
    id,
    date,
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
    effective_input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_input_context,
    total_cost,
    actual_total_cost,
    input_cost,
    output_cost,
    cache_creation_cost,
    cache_read_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    p50_response_time_ms,
    p90_response_time_ms,
    p99_response_time_ms,
    p50_first_byte_time_ms,
    p90_first_byte_time_ms,
    p99_first_byte_time_ms,
    fallback_count,
    unique_models,
    unique_providers,
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
    $33, $34, $35, $36, $37, $38, $39, $40,
    $41, $42, $43, $44, $45, $46, $47, $48,
    $49, $50, $51, $52, $53
)
ON CONFLICT (date)
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
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_input_context = EXCLUDED.total_input_context,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    input_cost = EXCLUDED.input_cost,
    output_cost = EXCLUDED.output_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    cache_read_cost = EXCLUDED.cache_read_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    p50_response_time_ms = EXCLUDED.p50_response_time_ms,
    p90_response_time_ms = EXCLUDED.p90_response_time_ms,
    p99_response_time_ms = EXCLUDED.p99_response_time_ms,
    p50_first_byte_time_ms = EXCLUDED.p50_first_byte_time_ms,
    p90_first_byte_time_ms = EXCLUDED.p90_first_byte_time_ms,
    p99_first_byte_time_ms = EXCLUDED.p99_first_byte_time_ms,
    fallback_count = EXCLUDED.fallback_count,
    unique_models = EXCLUDED.unique_models,
    unique_providers = EXCLUDED.unique_providers,
    is_complete = EXCLUDED.is_complete,
    aggregated_at = EXCLUDED.aggregated_at,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples,
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
INSERT INTO stats_daily_model (
    id,
    date,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-daily-model:', aggregated.model, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.model,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.avg_response_time_ms,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(provider_name, 'Unknown') AS provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY COALESCE(provider_name, 'Unknown')
)
INSERT INTO stats_daily_provider (
    id,
    date,
    provider_name,
    total_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-daily-provider:', aggregated.provider_name, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, provider_name)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        model,
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND model IS NOT NULL
      AND model <> ''
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY model, provider_name
)
INSERT INTO stats_daily_model_provider (
    id,
    date,
    model,
    provider_name,
    total_requests,
    total_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-model-provider:',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.total_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model, provider_name)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    total_tokens = EXCLUDED.total_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_COST_SAVINGS_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
)
INSERT INTO stats_daily_cost_savings (
    id,
    date,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-daily-cost-savings:', CAST($1 AS TEXT))),
    $1,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_COST_SAVINGS_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
    GROUP BY COALESCE(usage.provider_name, '')
)
INSERT INTO stats_daily_cost_savings_provider (
    id,
    date,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-cost-savings-provider:',
            CAST($1 AS TEXT),
            ':',
            aggregated.provider_name
        )
    ),
    $1,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, provider_name)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_COST_SAVINGS_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(usage.model, '') AS model,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
    GROUP BY COALESCE(usage.model, '')
)
INSERT INTO stats_daily_cost_savings_model (
    id,
    date,
    model,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-cost-savings-model:',
            CAST($1 AS TEXT),
            ':',
            aggregated.model
        )
    ),
    $1,
    aggregated.model,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_COST_SAVINGS_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(usage.model, '') AS model,
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
    GROUP BY COALESCE(usage.model, ''), COALESCE(usage.provider_name, '')
)
INSERT INTO stats_daily_cost_savings_model_provider (
    id,
    date,
    model,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-cost-savings-model-provider:',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model, provider_name)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_DAILY_API_KEY_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        api_key_id,
        MAX(api_key_name) AS api_key_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status_code >= 400 OR error_message IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND api_key_id IS NOT NULL
    GROUP BY api_key_id
)
INSERT INTO stats_daily_api_key (
    id,
    api_key_id,
    api_key_name,
    date,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-daily-api-key:', aggregated.api_key_id, ':', CAST($1 AS TEXT))),
    aggregated.api_key_id,
    aggregated.api_key_name,
    $1,
    aggregated.total_requests,
    GREATEST(aggregated.total_requests - aggregated.error_requests, 0),
    aggregated.error_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (api_key_id, date)
DO UPDATE SET
    api_key_name = COALESCE(EXCLUDED.api_key_name, stats_daily_api_key.api_key_name),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const DELETE_STATS_DAILY_ERRORS_FOR_DATE_SQL: &str = r#"
DELETE FROM stats_daily_error
WHERE date = $1
"#;
pub(super) const INSERT_STATS_DAILY_ERROR_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        error_category,
        provider_name,
        model,
        CAST(COUNT(id) AS BIGINT) AS total_count
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND error_category IS NOT NULL
    GROUP BY error_category, provider_name, model
)
INSERT INTO stats_daily_error (
    id,
    date,
    error_category,
    provider_name,
    model,
    count,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-error:',
            CAST($1 AS TEXT),
            ':',
            aggregated.error_category,
            ':',
            COALESCE(aggregated.provider_name, ''),
            ':',
            COALESCE(aggregated.model, '')
        )
    ),
    $1,
    aggregated.error_category,
    aggregated.provider_name,
    aggregated.model,
    aggregated.total_count,
    $3,
    $3
FROM aggregated
"#;
pub(super) const UPSERT_STATS_USER_DAILY_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status_code >= 400
                             OR lower(COALESCE(status, '')) = 'failed'
                             OR error_message IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(cache_creation_cost_usd), 0) AS DOUBLE PRECISION) AS cache_creation_cost,
        CAST(COALESCE(SUM(cache_read_cost_usd), 0) AS DOUBLE PRECISION) AS cache_read_cost,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(output_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_output_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(cache_creation_input_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_cache_creation_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_cache_read_tokens,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id
)
INSERT INTO stats_user_daily (
    id,
    user_id,
    username,
    date,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_input_context,
    total_cost,
    cache_creation_cost,
    cache_read_cost,
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
    md5(CONCAT('stats-user-daily:', aggregated.user_id, ':', CAST($1 AS TEXT))),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.total_requests,
    GREATEST(aggregated.total_requests - aggregated.error_requests, 0),
    aggregated.error_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_input_context,
    aggregated.total_cost,
    aggregated.cache_creation_cost,
    aggregated.cache_read_cost,
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
ON CONFLICT (user_id, date)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_input_context = EXCLUDED.total_input_context,
    total_cost = EXCLUDED.total_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    cache_read_cost = EXCLUDED.cache_read_cost,
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
pub(super) const UPSERT_STATS_USER_DAILY_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS success_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
        ,
        COALESCE(
            SUM(
                CASE
                    WHEN status <> 'failed'
                         AND (status_code IS NULL OR status_code < 400)
                         AND error_message IS NULL
                         AND response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS successful_response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                             AND response_time_ms IS NOT NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS successful_response_time_samples
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
INSERT INTO stats_user_daily_model (
    id,
    user_id,
    username,
    date,
    model,
    total_requests,
    success_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    total_tokens,
    total_input_context,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    successful_response_time_sum_ms,
    successful_response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily-model:', aggregated.user_id, ':', CAST($1 AS TEXT), ':', aggregated.model)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.total_requests,
    aggregated.success_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.total_tokens,
    aggregated.total_input_context,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.successful_response_time_sum_ms,
    aggregated.successful_response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_model.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    total_input_context = EXCLUDED.total_input_context,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    successful_response_time_sum_ms = EXCLUDED.successful_response_time_sum_ms,
    successful_response_time_samples = EXCLUDED.successful_response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_USER_DAILY_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS success_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples,
        COALESCE(
            SUM(
                CASE
                    WHEN status <> 'failed'
                         AND (status_code IS NULL OR status_code < 400)
                         AND error_message IS NULL
                         AND response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS successful_response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                             AND response_time_ms IS NOT NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS successful_response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, provider_name
)
INSERT INTO stats_user_daily_provider (
    id,
    user_id,
    username,
    date,
    provider_name,
    total_requests,
    success_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    total_tokens,
    total_input_context,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    successful_response_time_sum_ms,
    successful_response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily-provider:', aggregated.user_id, ':', CAST($1 AS TEXT), ':', aggregated.provider_name)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.success_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.total_tokens,
    aggregated.total_input_context,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.successful_response_time_sum_ms,
    aggregated.successful_response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, provider_name)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_provider.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    total_input_context = EXCLUDED.total_input_context,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    successful_response_time_sum_ms = EXCLUDED.successful_response_time_sum_ms,
    successful_response_time_samples = EXCLUDED.successful_response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_USER_DAILY_API_FORMAT_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        api_format,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS success_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples,
        COALESCE(
            SUM(
                CASE
                    WHEN status <> 'failed'
                         AND (status_code IS NULL OR status_code < 400)
                         AND error_message IS NULL
                         AND response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS successful_response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                             AND response_time_ms IS NOT NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS successful_response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND api_format IS NOT NULL
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, api_format
)
INSERT INTO stats_user_daily_api_format (
    id,
    user_id,
    username,
    date,
    api_format,
    total_requests,
    success_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    total_tokens,
    total_input_context,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    successful_response_time_sum_ms,
    successful_response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily-api-format:', aggregated.user_id, ':', CAST($1 AS TEXT), ':', aggregated.api_format)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.api_format,
    aggregated.total_requests,
    aggregated.success_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.total_tokens,
    aggregated.total_input_context,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.successful_response_time_sum_ms,
    aggregated.successful_response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, api_format)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_api_format.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    total_input_context = EXCLUDED.total_input_context,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    successful_response_time_sum_ms = EXCLUDED.successful_response_time_sum_ms,
    successful_response_time_samples = EXCLUDED.successful_response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_USER_DAILY_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        model,
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
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
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND model IS NOT NULL
      AND model <> ''
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, model, provider_name
)
INSERT INTO stats_user_daily_model_provider (
    id,
    user_id,
    username,
    date,
    model,
    provider_name,
    total_requests,
    total_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-model-provider:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.total_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model, provider_name)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_model_provider.username),
    total_requests = EXCLUDED.total_requests,
    total_tokens = EXCLUDED.total_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_USER_DAILY_COST_SAVINGS_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id
)
INSERT INTO stats_user_daily_cost_savings (
    id,
    user_id,
    username,
    date,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT)
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_cost_savings.username),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_USER_DAILY_COST_SAVINGS_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id, COALESCE(usage.provider_name, '')
)
INSERT INTO stats_user_daily_cost_savings_provider (
    id,
    user_id,
    username,
    date,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings-provider:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.provider_name
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, provider_name)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_cost_savings_provider.username),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_USER_DAILY_COST_SAVINGS_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        COALESCE(usage.model, '') AS model,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id, COALESCE(usage.model, '')
)
INSERT INTO stats_user_daily_cost_savings_model (
    id,
    user_id,
    username,
    date,
    model,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings-model:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.model
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_cost_savings_model.username),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const UPSERT_STATS_USER_DAILY_COST_SAVINGS_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        COALESCE(usage.model, '') AS model,
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id, COALESCE(usage.model, ''), COALESCE(usage.provider_name, '')
)
INSERT INTO stats_user_daily_cost_savings_model_provider (
    id,
    user_id,
    username,
    date,
    model,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings-model-provider:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model, provider_name)
DO UPDATE SET
    username = COALESCE(
        EXCLUDED.username,
        stats_user_daily_cost_savings_model_provider.username
    ),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const SELECT_EXISTING_STATS_SUMMARY_ID_SQL: &str = r#"
SELECT id
FROM stats_summary
ORDER BY created_at ASC, id ASC
LIMIT 1
"#;
pub(super) const SELECT_STATS_SUMMARY_TOTALS_SQL: &str = r#"
SELECT
    CAST(COALESCE(SUM(total_requests), 0) AS BIGINT) AS all_time_requests,
    CAST(COALESCE(SUM(success_requests), 0) AS BIGINT) AS all_time_success_requests,
    CAST(COALESCE(SUM(error_requests), 0) AS BIGINT) AS all_time_error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS all_time_input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS all_time_output_tokens,
    CAST(COALESCE(SUM(cache_creation_tokens), 0) AS BIGINT) AS all_time_cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_tokens), 0) AS BIGINT) AS all_time_cache_read_tokens,
    CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS all_time_cost,
    CAST(COALESCE(SUM(actual_total_cost), 0) AS DOUBLE PRECISION) AS all_time_actual_cost
FROM stats_daily
WHERE date < $1
"#;
pub(super) const SELECT_STATS_SUMMARY_ENTITY_COUNTS_SQL: &str = r#"
SELECT
    CAST((SELECT COUNT(id) FROM users) AS BIGINT) AS total_users,
    CAST((SELECT COUNT(id) FROM users WHERE is_active IS TRUE) AS BIGINT) AS active_users,
    CAST((SELECT COUNT(id) FROM api_keys) AS BIGINT) AS total_api_keys,
    CAST((SELECT COUNT(id) FROM api_keys WHERE is_active IS TRUE) AS BIGINT) AS active_api_keys
"#;
pub(super) const INSERT_STATS_SUMMARY_SQL: &str = r#"
INSERT INTO stats_summary (
    id,
    cutoff_date,
    all_time_requests,
    all_time_success_requests,
    all_time_error_requests,
    all_time_input_tokens,
    all_time_output_tokens,
    all_time_cache_creation_tokens,
    all_time_cache_read_tokens,
    all_time_cost,
    all_time_actual_cost,
    total_users,
    active_users,
    total_api_keys,
    active_api_keys,
    created_at,
    updated_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    $9, $10, $11, $12, $13, $14, $15, $16, $17
)
"#;
pub(super) const UPDATE_STATS_SUMMARY_SQL: &str = r#"
UPDATE stats_summary
SET cutoff_date = $2,
    all_time_requests = $3,
    all_time_success_requests = $4,
    all_time_error_requests = $5,
    all_time_input_tokens = $6,
    all_time_output_tokens = $7,
    all_time_cache_creation_tokens = $8,
    all_time_cache_read_tokens = $9,
    all_time_cost = $10,
    all_time_actual_cost = $11,
    total_users = $12,
    active_users = $13,
    total_api_keys = $14,
    active_api_keys = $15,
    updated_at = $16
WHERE id = $1
"#;
pub(super) const UPSERT_STATS_USER_SUMMARY_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        CAST(COALESCE(SUM(total_requests), 0) AS BIGINT) AS all_time_requests,
        CAST(COALESCE(SUM(success_requests), 0) AS BIGINT) AS all_time_success_requests,
        CAST(COALESCE(SUM(error_requests), 0) AS BIGINT) AS all_time_error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS all_time_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS all_time_output_tokens,
        CAST(COALESCE(SUM(cache_creation_tokens), 0) AS BIGINT) AS all_time_cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_tokens), 0) AS BIGINT) AS all_time_cache_read_tokens,
        CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS all_time_cost,
        CAST(COALESCE(SUM(actual_total_cost), 0) AS DOUBLE PRECISION) AS all_time_actual_cost,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN total_requests > 0 THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS active_days,
        MIN(CASE WHEN total_requests > 0 THEN date ELSE NULL END) AS first_active_date,
        MAX(CASE WHEN total_requests > 0 THEN date ELSE NULL END) AS last_active_date
    FROM stats_user_daily
    WHERE user_id IS NOT NULL
      AND date < $1
    GROUP BY user_id
)
INSERT INTO stats_user_summary (
    id,
    user_id,
    username,
    cutoff_date,
    all_time_requests,
    all_time_success_requests,
    all_time_error_requests,
    all_time_input_tokens,
    all_time_output_tokens,
    all_time_cache_creation_tokens,
    all_time_cache_read_tokens,
    all_time_cost,
    all_time_actual_cost,
    active_days,
    first_active_date,
    last_active_date,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-summary:', aggregated.user_id)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.all_time_requests,
    aggregated.all_time_success_requests,
    aggregated.all_time_error_requests,
    aggregated.all_time_input_tokens,
    aggregated.all_time_output_tokens,
    aggregated.all_time_cache_creation_tokens,
    aggregated.all_time_cache_read_tokens,
    aggregated.all_time_cost,
    aggregated.all_time_actual_cost,
    aggregated.active_days,
    aggregated.first_active_date,
    aggregated.last_active_date,
    $2,
    $2
FROM aggregated
ON CONFLICT (user_id)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_summary.username),
    cutoff_date = EXCLUDED.cutoff_date,
    all_time_requests = EXCLUDED.all_time_requests,
    all_time_success_requests = EXCLUDED.all_time_success_requests,
    all_time_error_requests = EXCLUDED.all_time_error_requests,
    all_time_input_tokens = EXCLUDED.all_time_input_tokens,
    all_time_output_tokens = EXCLUDED.all_time_output_tokens,
    all_time_cache_creation_tokens = EXCLUDED.all_time_cache_creation_tokens,
    all_time_cache_read_tokens = EXCLUDED.all_time_cache_read_tokens,
    all_time_cost = EXCLUDED.all_time_cost,
    all_time_actual_cost = EXCLUDED.all_time_actual_cost,
    active_days = EXCLUDED.active_days,
    first_active_date = EXCLUDED.first_active_date,
    last_active_date = EXCLUDED.last_active_date,
    updated_at = EXCLUDED.updated_at
"#;
pub(super) const SELECT_LATEST_STATS_DAILY_DATE_SQL: &str = r#"
SELECT MAX(date) AS latest_date
FROM stats_daily
WHERE is_complete IS TRUE
"#;
pub(super) const SELECT_NEXT_STATS_DAILY_BUCKET_SQL: &str = r#"
SELECT date_trunc('day', MIN(created_at)) AS next_bucket
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;
