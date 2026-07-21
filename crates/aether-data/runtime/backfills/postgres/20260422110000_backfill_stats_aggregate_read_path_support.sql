CREATE TEMP TABLE tmp_stats_backfill_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc,
    (date_trunc('hour', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_hour_utc;

CREATE TEMP TABLE tmp_stats_backfill_usage_base ON COMMIT DROP AS
WITH raw AS (
    SELECT
        usage.id,
        usage.request_id,
        usage.user_id,
        usage.username,
        usage.api_key_id,
        usage.api_key_name,
        usage.provider_name,
        usage.model,
        usage.error_category,
        usage.status,
        usage.status_code,
        usage.error_message,
        usage.response_time_ms,
        usage.first_byte_time_ms,
        usage.created_at,
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        (date_trunc('hour', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
        split_part(lower(COALESCE(COALESCE(usage.endpoint_api_format, usage.api_format), '')), ':', 1)
            AS normalized_api_format,
        GREATEST(COALESCE(usage.input_tokens, 0), 0)::BIGINT AS input_tokens_safe,
        GREATEST(COALESCE(usage.output_tokens, 0), 0)::BIGINT AS output_tokens_safe,
        CASE
            WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                 AND (
                    COALESCE(usage.cache_creation_input_tokens_5m, 0)
                    + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                 ) > 0
            THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
               + COALESCE(usage.cache_creation_input_tokens_1h, 0)
            ELSE COALESCE(usage.cache_creation_input_tokens, 0)
        END::BIGINT AS cache_creation_tokens_safe,
        GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::BIGINT AS cache_read_tokens_safe,
        COALESCE(usage.total_cost_usd, 0)::DOUBLE PRECISION AS total_cost_safe,
        COALESCE(usage.actual_total_cost_usd, 0)::DOUBLE PRECISION AS actual_total_cost_safe,
        COALESCE(usage.input_cost_usd, 0)::DOUBLE PRECISION AS input_cost_safe,
        COALESCE(usage.output_cost_usd, 0)::DOUBLE PRECISION AS output_cost_safe,
        COALESCE(usage.cache_creation_cost_usd, 0)::DOUBLE PRECISION AS cache_creation_cost_safe,
        COALESCE(usage.cache_read_cost_usd, 0)::DOUBLE PRECISION AS cache_read_cost_safe,
        CASE
            WHEN usage.status_code >= 400
                 OR lower(COALESCE(usage.status, '')) = 'failed'
                 OR usage.error_message IS NOT NULL THEN 1
            ELSE 0
        END::BIGINT AS error_flag
    FROM usage
),
derived AS (
    SELECT
        raw.*,
        CASE
            WHEN raw.input_tokens_safe <= 0 THEN 0
            WHEN raw.cache_read_tokens_safe <= 0 THEN raw.input_tokens_safe
            WHEN raw.normalized_api_format IN ('openai', 'gemini', 'google') THEN GREATEST(
                raw.input_tokens_safe - raw.cache_read_tokens_safe,
                0
            )
            ELSE raw.input_tokens_safe
        END::BIGINT AS effective_input_tokens_safe
    FROM raw
)
SELECT
    derived.*,
    CASE
        WHEN derived.normalized_api_format IN ('claude', 'anthropic') THEN
            derived.input_tokens_safe
            + derived.cache_creation_tokens_safe
            + derived.cache_read_tokens_safe
        WHEN derived.normalized_api_format IN ('openai', 'gemini', 'google') THEN
            derived.effective_input_tokens_safe + derived.cache_read_tokens_safe
        WHEN derived.cache_creation_tokens_safe > 0 THEN
            derived.input_tokens_safe
            + derived.cache_creation_tokens_safe
            + derived.cache_read_tokens_safe
        ELSE derived.input_tokens_safe + derived.cache_read_tokens_safe
    END::BIGINT AS total_input_context_safe,
    (
        derived.status NOT IN ('pending', 'streaming')
        AND derived.provider_name NOT IN ('unknown', 'pending')
    ) AS is_aggregatable
FROM derived;

CREATE TEMP TABLE tmp_stats_backfill_daily_fallback ON COMMIT DROP AS
SELECT
    fallback_requests.day_utc,
    COUNT(*)::BIGINT AS fallback_count
FROM (
    SELECT
        request_candidates.request_id,
        (date_trunc('day', request_candidates.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
            AS day_utc
    FROM request_candidates
    WHERE request_candidates.status = ANY(ARRAY['success', 'failed'])
    GROUP BY request_candidates.request_id, day_utc
    HAVING COUNT(request_candidates.id) > 1
) AS fallback_requests
GROUP BY fallback_requests.day_utc;

CREATE TEMP TABLE tmp_stats_backfill_daily_response_percentiles ON COMMIT DROP AS
SELECT
    usage_base.day_utc,
    COUNT(*)::BIGINT AS sample_count,
    percentile_cont(0.5) WITHIN GROUP (ORDER BY usage_base.response_time_ms)::DOUBLE PRECISION AS p50,
    percentile_cont(0.9) WITHIN GROUP (ORDER BY usage_base.response_time_ms)::DOUBLE PRECISION AS p90,
    percentile_cont(0.99) WITHIN GROUP (ORDER BY usage_base.response_time_ms)::DOUBLE PRECISION AS p99
FROM tmp_stats_backfill_usage_base AS usage_base
WHERE usage_base.status = 'completed'
  AND usage_base.provider_name NOT IN ('unknown', 'pending')
  AND usage_base.response_time_ms IS NOT NULL
GROUP BY usage_base.day_utc;

CREATE TEMP TABLE tmp_stats_backfill_daily_first_byte_percentiles ON COMMIT DROP AS
SELECT
    usage_base.day_utc,
    COUNT(*)::BIGINT AS sample_count,
    percentile_cont(0.5) WITHIN GROUP (ORDER BY usage_base.first_byte_time_ms)::DOUBLE PRECISION AS p50,
    percentile_cont(0.9) WITHIN GROUP (ORDER BY usage_base.first_byte_time_ms)::DOUBLE PRECISION AS p90,
    percentile_cont(0.99) WITHIN GROUP (ORDER BY usage_base.first_byte_time_ms)::DOUBLE PRECISION AS p99
FROM tmp_stats_backfill_usage_base AS usage_base
WHERE usage_base.status = 'completed'
  AND usage_base.provider_name NOT IN ('unknown', 'pending')
  AND usage_base.first_byte_time_ms IS NOT NULL
GROUP BY usage_base.day_utc;

TRUNCATE TABLE
    stats_hourly_user_model,
    stats_hourly_provider,
    stats_hourly_model,
    stats_hourly_user,
    stats_hourly,
    stats_user_daily_model,
    stats_user_summary,
    stats_daily_error,
    stats_daily_api_key,
    stats_daily_provider,
    stats_daily_model,
    stats_user_daily,
    stats_daily,
    stats_summary;

INSERT INTO stats_hourly (
    id,
    hour_utc,
    total_requests,
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
SELECT
    md5(CONCAT('stats-hourly:', CAST(usage_base.hour_utc AS TEXT))),
    usage_base.hour_utc,
    COUNT(usage_base.id)::INTEGER,
    GREATEST(COUNT(usage_base.id)::BIGINT - COALESCE(SUM(usage_base.error_flag), 0), 0)::INTEGER,
    COALESCE(SUM(usage_base.error_flag), 0)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.actual_total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    COALESCE(
        AVG(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE NULL
            END
        ),
        0
    )::DOUBLE PRECISION AS avg_response_time_ms,
    TRUE,
    context.now_utc,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.hour_utc < context.current_hour_utc
GROUP BY usage_base.hour_utc, context.now_utc;

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
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-user:', usage_base.user_id, ':', CAST(usage_base.hour_utc AS TEXT))),
    usage_base.hour_utc,
    usage_base.user_id,
    COUNT(usage_base.id)::INTEGER,
    GREATEST(COUNT(usage_base.id)::BIGINT - COALESCE(SUM(usage_base.error_flag), 0), 0)::INTEGER,
    COALESCE(SUM(usage_base.error_flag), 0)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.actual_total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.user_id IS NOT NULL
  AND usage_base.hour_utc < context.current_hour_utc
GROUP BY usage_base.hour_utc, usage_base.user_id, context.now_utc;

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
    md5(CONCAT('stats-hourly-model:', usage_base.model, ':', CAST(usage_base.hour_utc AS TEXT))),
    usage_base.hour_utc,
    usage_base.model,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    COALESCE(
        AVG(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE NULL
            END
        ),
        0
    )::DOUBLE PRECISION AS avg_response_time_ms,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.model IS NOT NULL
  AND usage_base.model <> ''
  AND usage_base.hour_utc < context.current_hour_utc
GROUP BY usage_base.hour_utc, usage_base.model, context.now_utc;

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
    md5(
        CONCAT(
            'stats-hourly-user-model:',
            usage_base.user_id,
            ':',
            usage_base.model,
            ':',
            CAST(usage_base.hour_utc AS TEXT)
        )
    ),
    usage_base.hour_utc,
    usage_base.user_id,
    usage_base.model,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.user_id IS NOT NULL
  AND usage_base.model IS NOT NULL
  AND usage_base.model <> ''
  AND usage_base.hour_utc < context.current_hour_utc
GROUP BY usage_base.hour_utc, usage_base.user_id, usage_base.model, context.now_utc;

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
    md5(
        CONCAT(
            'stats-hourly-provider:',
            usage_base.provider_name,
            ':',
            CAST(usage_base.hour_utc AS TEXT)
        )
    ),
    usage_base.hour_utc,
    usage_base.provider_name,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.provider_name IS NOT NULL
  AND usage_base.provider_name <> ''
  AND usage_base.hour_utc < context.current_hour_utc
GROUP BY usage_base.hour_utc, usage_base.provider_name, context.now_utc;

INSERT INTO stats_daily (
    id,
    date,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    cache_creation_tokens,
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
WITH aggregated AS (
    SELECT
        usage_base.day_utc,
        COUNT(usage_base.id)::INTEGER AS total_requests,
        COALESCE(SUM(usage_base.error_flag), 0)::INTEGER AS error_requests,
        COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT AS input_tokens,
        COALESCE(SUM(usage_base.effective_input_tokens_safe), 0)::BIGINT AS effective_input_tokens,
        COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT AS output_tokens,
        COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT AS cache_creation_tokens,
        COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT AS cache_read_tokens,
        COALESCE(SUM(usage_base.total_input_context_safe), 0)::BIGINT AS total_input_context,
        COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION AS total_cost,
        COALESCE(SUM(usage_base.actual_total_cost_safe), 0)::DOUBLE PRECISION AS actual_total_cost,
        COALESCE(SUM(usage_base.input_cost_safe), 0)::DOUBLE PRECISION AS input_cost,
        COALESCE(SUM(usage_base.output_cost_safe), 0)::DOUBLE PRECISION AS output_cost,
        COALESCE(SUM(usage_base.cache_creation_cost_safe), 0)::DOUBLE PRECISION AS cache_creation_cost,
        COALESCE(SUM(usage_base.cache_read_cost_safe), 0)::DOUBLE PRECISION AS cache_read_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN usage_base.response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        COALESCE(
            SUM(
                CASE
                    WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        )::BIGINT AS response_time_samples,
        COALESCE(
            AVG(
                CASE
                    WHEN usage_base.response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE NULL
                END
            ),
            0
        )::DOUBLE PRECISION AS avg_response_time_ms,
        COUNT(DISTINCT usage_base.model)::INTEGER AS unique_models,
        COUNT(DISTINCT usage_base.provider_name)::INTEGER AS unique_providers
    FROM tmp_stats_backfill_usage_base AS usage_base
    CROSS JOIN tmp_stats_backfill_context AS context
    WHERE usage_base.is_aggregatable
      AND usage_base.day_utc < context.current_day_utc
    GROUP BY usage_base.day_utc
)
SELECT
    md5(CONCAT('stats-daily:', CAST(aggregated.day_utc AS TEXT))),
    aggregated.day_utc,
    aggregated.total_requests,
    GREATEST(aggregated.total_requests::BIGINT - aggregated.error_requests::BIGINT, 0)::INTEGER,
    aggregated.error_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_input_context,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.input_cost,
    aggregated.output_cost,
    aggregated.cache_creation_cost,
    aggregated.cache_read_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.avg_response_time_ms,
    CASE
        WHEN response_percentiles.sample_count >= 10 THEN floor(response_percentiles.p50)::INTEGER
        ELSE NULL
    END AS p50_response_time_ms,
    CASE
        WHEN response_percentiles.sample_count >= 10 THEN floor(response_percentiles.p90)::INTEGER
        ELSE NULL
    END AS p90_response_time_ms,
    CASE
        WHEN response_percentiles.sample_count >= 10 THEN floor(response_percentiles.p99)::INTEGER
        ELSE NULL
    END AS p99_response_time_ms,
    CASE
        WHEN first_byte_percentiles.sample_count >= 10 THEN floor(first_byte_percentiles.p50)::INTEGER
        ELSE NULL
    END AS p50_first_byte_time_ms,
    CASE
        WHEN first_byte_percentiles.sample_count >= 10 THEN floor(first_byte_percentiles.p90)::INTEGER
        ELSE NULL
    END AS p90_first_byte_time_ms,
    CASE
        WHEN first_byte_percentiles.sample_count >= 10 THEN floor(first_byte_percentiles.p99)::INTEGER
        ELSE NULL
    END AS p99_first_byte_time_ms,
    COALESCE(fallbacks.fallback_count, 0)::INTEGER,
    aggregated.unique_models,
    aggregated.unique_providers,
    TRUE,
    context.now_utc,
    context.now_utc,
    context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_backfill_context AS context
LEFT JOIN tmp_stats_backfill_daily_fallback AS fallbacks
    ON fallbacks.day_utc = aggregated.day_utc
LEFT JOIN tmp_stats_backfill_daily_response_percentiles AS response_percentiles
    ON response_percentiles.day_utc = aggregated.day_utc
LEFT JOIN tmp_stats_backfill_daily_first_byte_percentiles AS first_byte_percentiles
    ON first_byte_percentiles.day_utc = aggregated.day_utc;

INSERT INTO stats_daily_model (
    id,
    date,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-daily-model:', usage_base.model, ':', CAST(usage_base.day_utc AS TEXT))),
    usage_base.day_utc,
    usage_base.model,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    COALESCE(
        AVG(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE NULL
            END
        ),
        0
    )::DOUBLE PRECISION AS avg_response_time_ms,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.model IS NOT NULL
  AND usage_base.model <> ''
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.model, context.now_utc;

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
    md5(
        CONCAT(
            'stats-daily-provider:',
            usage_base.provider_name,
            ':',
            CAST(usage_base.day_utc AS TEXT)
        )
    ),
    usage_base.day_utc,
    usage_base.provider_name,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.provider_name, context.now_utc;

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
    md5(CONCAT('stats-daily-api-key:', usage_base.api_key_id, ':', CAST(usage_base.day_utc AS TEXT))),
    usage_base.api_key_id,
    MAX(usage_base.api_key_name),
    usage_base.day_utc,
    COUNT(usage_base.id)::INTEGER,
    GREATEST(
        COUNT(usage_base.id)::BIGINT
        - COALESCE(
            SUM(
                CASE
                    WHEN usage_base.status_code >= 400
                         OR usage_base.error_message IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ),
        0
    )::INTEGER,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.status_code >= 400
                     OR usage_base.error_message IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.api_key_id IS NOT NULL
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.api_key_id, context.now_utc;

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
            CAST(usage_base.day_utc AS TEXT),
            ':',
            usage_base.error_category,
            ':',
            COALESCE(usage_base.provider_name, ''),
            ':',
            COALESCE(usage_base.model, '')
        )
    ),
    usage_base.day_utc,
    usage_base.error_category,
    usage_base.provider_name,
    usage_base.model,
    COUNT(usage_base.id)::INTEGER,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.error_category IS NOT NULL
  AND usage_base.day_utc < context.current_day_utc
GROUP BY
    usage_base.day_utc,
    usage_base.error_category,
    usage_base.provider_name,
    usage_base.model,
    context.now_utc;

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
    cache_read_tokens,
    total_input_context,
    total_cost,
    cache_creation_cost,
    cache_read_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily:', usage_base.user_id, ':', CAST(usage_base.day_utc AS TEXT))),
    usage_base.user_id,
    MAX(usage_base.username),
    usage_base.day_utc,
    COUNT(usage_base.id)::INTEGER,
    GREATEST(COUNT(usage_base.id)::BIGINT - COALESCE(SUM(usage_base.error_flag), 0), 0)::INTEGER,
    COALESCE(SUM(usage_base.error_flag), 0)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.effective_input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_input_context_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.cache_creation_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.cache_read_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.actual_total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.user_id IS NOT NULL
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.user_id, context.now_utc;

INSERT INTO stats_user_daily_model (
    id,
    user_id,
    username,
    date,
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
    md5(
        CONCAT(
            'stats-user-daily-model:',
            usage_base.user_id,
            ':',
            CAST(usage_base.day_utc AS TEXT),
            ':',
            usage_base.model
        )
    ),
    usage_base.user_id,
    MAX(usage_base.username),
    usage_base.day_utc,
    usage_base.model,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_usage_base AS usage_base
CROSS JOIN tmp_stats_backfill_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.user_id IS NOT NULL
  AND usage_base.model IS NOT NULL
  AND usage_base.model <> ''
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.user_id, usage_base.model, context.now_utc;

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
SELECT
    md5('stats-summary'),
    context.current_day_utc,
    COALESCE((SELECT SUM(total_requests)::INTEGER FROM stats_daily), 0),
    COALESCE((SELECT SUM(success_requests)::INTEGER FROM stats_daily), 0),
    COALESCE((SELECT SUM(error_requests)::INTEGER FROM stats_daily), 0),
    COALESCE((SELECT SUM(input_tokens)::BIGINT FROM stats_daily), 0),
    COALESCE((SELECT SUM(output_tokens)::BIGINT FROM stats_daily), 0),
    COALESCE((SELECT SUM(cache_creation_tokens)::BIGINT FROM stats_daily), 0),
    COALESCE((SELECT SUM(cache_read_tokens)::BIGINT FROM stats_daily), 0)::BIGINT,
    COALESCE((SELECT SUM(total_cost)::DOUBLE PRECISION FROM stats_daily), 0)::DOUBLE PRECISION,
    COALESCE((SELECT SUM(actual_total_cost)::DOUBLE PRECISION FROM stats_daily), 0)::DOUBLE PRECISION,
    (SELECT COUNT(id)::INTEGER FROM users),
    (SELECT COUNT(id)::INTEGER FROM users WHERE is_active IS TRUE),
    (SELECT COUNT(id)::INTEGER FROM api_keys),
    (SELECT COUNT(id)::INTEGER FROM api_keys WHERE is_active IS TRUE),
    context.now_utc,
    context.now_utc
FROM tmp_stats_backfill_context AS context
WHERE EXISTS (SELECT 1 FROM stats_daily);

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
    md5(CONCAT('stats-user-summary:', stats_user_daily.user_id)),
    stats_user_daily.user_id,
    MAX(stats_user_daily.username),
    context.current_day_utc,
    COALESCE(SUM(stats_user_daily.total_requests), 0)::INTEGER,
    COALESCE(SUM(stats_user_daily.success_requests), 0)::INTEGER,
    COALESCE(SUM(stats_user_daily.error_requests), 0)::INTEGER,
    COALESCE(SUM(stats_user_daily.input_tokens), 0)::BIGINT,
    COALESCE(SUM(stats_user_daily.output_tokens), 0)::BIGINT,
    COALESCE(SUM(stats_user_daily.cache_creation_tokens), 0)::BIGINT,
    COALESCE(SUM(stats_user_daily.cache_read_tokens), 0)::BIGINT,
    COALESCE(SUM(stats_user_daily.total_cost), 0)::DOUBLE PRECISION,
    COALESCE(SUM(stats_user_daily.actual_total_cost), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN stats_user_daily.total_requests > 0 THEN 1
                ELSE 0
            END
        ),
        0
    )::INTEGER,
    MIN(CASE WHEN stats_user_daily.total_requests > 0 THEN stats_user_daily.date ELSE NULL END),
    MAX(CASE WHEN stats_user_daily.total_requests > 0 THEN stats_user_daily.date ELSE NULL END),
    context.now_utc,
    context.now_utc
FROM stats_user_daily
CROSS JOIN tmp_stats_backfill_context AS context
GROUP BY stats_user_daily.user_id, context.current_day_utc, context.now_utc;
CREATE TEMP TABLE tmp_stats_user_breakdown_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc;

CREATE TEMP TABLE tmp_stats_user_breakdown_usage_base ON COMMIT DROP AS
WITH raw AS (
    SELECT
        usage.id,
        usage.user_id,
        usage.username,
        usage.provider_name,
        usage.model,
        usage.api_format,
        usage.status,
        usage.status_code,
        usage.error_message,
        usage.response_time_ms,
        usage.created_at,
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        split_part(lower(COALESCE(COALESCE(usage.endpoint_api_format, usage.api_format), '')), ':', 1)
            AS normalized_api_format,
        GREATEST(COALESCE(usage.input_tokens, 0), 0)::BIGINT AS input_tokens_safe,
        GREATEST(COALESCE(usage.output_tokens, 0), 0)::BIGINT AS output_tokens_safe,
        GREATEST(COALESCE(usage.total_tokens, 0), 0)::BIGINT AS total_tokens_safe,
        CASE
            WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                 AND (
                    COALESCE(usage.cache_creation_input_tokens_5m, 0)
                    + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                 ) > 0
            THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
               + COALESCE(usage.cache_creation_input_tokens_1h, 0)
            ELSE COALESCE(usage.cache_creation_input_tokens, 0)
        END::BIGINT AS cache_creation_tokens_safe,
        GREATEST(COALESCE(usage.cache_creation_input_tokens_5m, 0), 0)::BIGINT
            AS cache_creation_ephemeral_5m_tokens_safe,
        GREATEST(COALESCE(usage.cache_creation_input_tokens_1h, 0), 0)::BIGINT
            AS cache_creation_ephemeral_1h_tokens_safe,
        GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::BIGINT AS cache_read_tokens_safe,
        COALESCE(usage.total_cost_usd, 0)::DOUBLE PRECISION AS total_cost_safe,
        COALESCE(usage.actual_total_cost_usd, 0)::DOUBLE PRECISION AS actual_total_cost_safe
    FROM usage
),
derived AS (
    SELECT
        raw.*,
        CASE
            WHEN raw.input_tokens_safe <= 0 THEN 0
            WHEN raw.cache_read_tokens_safe <= 0 THEN raw.input_tokens_safe
            WHEN raw.normalized_api_format IN ('openai', 'gemini', 'google') THEN GREATEST(
                raw.input_tokens_safe - raw.cache_read_tokens_safe,
                0
            )
            ELSE raw.input_tokens_safe
        END::BIGINT AS effective_input_tokens_safe,
        CASE
            WHEN raw.status <> 'failed'
                 AND (raw.status_code IS NULL OR raw.status_code < 400)
                 AND raw.error_message IS NULL
            THEN 1
            ELSE 0
        END::BIGINT AS success_flag,
        CASE
            WHEN raw.response_time_ms IS NOT NULL
            THEN GREATEST(COALESCE(raw.response_time_ms, 0), 0)::DOUBLE PRECISION
            ELSE 0
        END AS response_time_sum_ms_safe,
        CASE
            WHEN raw.response_time_ms IS NOT NULL THEN 1
            ELSE 0
        END::BIGINT AS response_time_samples_safe
    FROM raw
)
SELECT
    derived.*,
    CASE
        WHEN derived.normalized_api_format IN ('claude', 'anthropic') THEN
            derived.input_tokens_safe
            + derived.cache_creation_tokens_safe
            + derived.cache_read_tokens_safe
        WHEN derived.normalized_api_format IN ('openai', 'gemini', 'google') THEN
            derived.effective_input_tokens_safe + derived.cache_read_tokens_safe
        WHEN derived.cache_creation_tokens_safe > 0 THEN
            derived.input_tokens_safe
            + derived.cache_creation_tokens_safe
            + derived.cache_read_tokens_safe
        ELSE derived.input_tokens_safe + derived.cache_read_tokens_safe
    END::BIGINT AS total_input_context_safe,
    CASE
        WHEN derived.success_flag > 0 THEN derived.response_time_sum_ms_safe
        ELSE 0
    END AS successful_response_time_sum_ms_safe,
    CASE
        WHEN derived.success_flag > 0 THEN derived.response_time_samples_safe
        ELSE 0
    END::BIGINT AS successful_response_time_samples_safe,
    (
        derived.user_id IS NOT NULL
        AND derived.status NOT IN ('pending', 'streaming')
        AND derived.provider_name NOT IN ('unknown', 'pending')
    ) AS is_aggregatable
FROM derived;

TRUNCATE TABLE
    stats_user_daily_api_format,
    stats_user_daily_provider,
    stats_user_daily_model;

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
    md5(
        CONCAT(
            'stats-user-daily-model:',
            usage_base.user_id,
            ':',
            CAST(usage_base.day_utc AS TEXT),
            ':',
            usage_base.model
        )
    ),
    usage_base.user_id,
    MAX(usage_base.username),
    usage_base.day_utc,
    usage_base.model,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.success_flag), 0)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.effective_input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_input_context_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_ephemeral_5m_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_ephemeral_1h_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.actual_total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.response_time_sum_ms_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.response_time_samples_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.successful_response_time_sum_ms_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.successful_response_time_samples_safe), 0)::BIGINT,
    context.now_utc,
    context.now_utc
FROM tmp_stats_user_breakdown_usage_base AS usage_base
CROSS JOIN tmp_stats_user_breakdown_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.model IS NOT NULL
  AND usage_base.model <> ''
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.user_id, usage_base.model, context.now_utc;

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
    md5(
        CONCAT(
            'stats-user-daily-provider:',
            usage_base.user_id,
            ':',
            CAST(usage_base.day_utc AS TEXT),
            ':',
            usage_base.provider_name
        )
    ),
    usage_base.user_id,
    MAX(usage_base.username),
    usage_base.day_utc,
    usage_base.provider_name,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.success_flag), 0)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.effective_input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_input_context_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_ephemeral_5m_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_ephemeral_1h_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.actual_total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.response_time_sum_ms_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.response_time_samples_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.successful_response_time_sum_ms_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.successful_response_time_samples_safe), 0)::BIGINT,
    context.now_utc,
    context.now_utc
FROM tmp_stats_user_breakdown_usage_base AS usage_base
CROSS JOIN tmp_stats_user_breakdown_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.provider_name IS NOT NULL
  AND usage_base.provider_name <> ''
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.user_id, usage_base.provider_name, context.now_utc;

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
    md5(
        CONCAT(
            'stats-user-daily-api-format:',
            usage_base.user_id,
            ':',
            CAST(usage_base.day_utc AS TEXT),
            ':',
            usage_base.api_format
        )
    ),
    usage_base.user_id,
    MAX(usage_base.username),
    usage_base.day_utc,
    usage_base.api_format,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(usage_base.success_flag), 0)::INTEGER,
    COALESCE(SUM(usage_base.input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.effective_input_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.output_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_input_context_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_ephemeral_5m_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_creation_ephemeral_1h_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.cache_read_tokens_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.actual_total_cost_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.response_time_sum_ms_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.response_time_samples_safe), 0)::BIGINT,
    COALESCE(SUM(usage_base.successful_response_time_sum_ms_safe), 0)::DOUBLE PRECISION,
    COALESCE(SUM(usage_base.successful_response_time_samples_safe), 0)::BIGINT,
    context.now_utc,
    context.now_utc
FROM tmp_stats_user_breakdown_usage_base AS usage_base
CROSS JOIN tmp_stats_user_breakdown_context AS context
WHERE usage_base.is_aggregatable
  AND usage_base.api_format IS NOT NULL
  AND usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.user_id, usage_base.api_format, context.now_utc;
CREATE TEMP TABLE tmp_stats_daily_model_provider_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc;

TRUNCATE TABLE stats_daily_model_provider;

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
            CAST(day_utc AS TEXT),
            ':',
            model,
            ':',
            provider_name
        )
    ),
    day_utc,
    model,
    provider_name,
    COUNT(id)::INTEGER,
    COALESCE(SUM(GREATEST(COALESCE(total_tokens, 0), 0)), 0)::BIGINT,
    COALESCE(SUM(COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0)), 0)::DOUBLE PRECISION,
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
    COALESCE(
        SUM(
            CASE
                WHEN response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    context.now_utc,
    context.now_utc
FROM (
    SELECT
        usage.id,
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        usage.model,
        usage.provider_name,
        usage.total_tokens,
        usage.total_cost_usd,
        usage.response_time_ms
    FROM usage
    WHERE usage.status NOT IN ('pending', 'streaming')
      AND usage.provider_name IS NOT NULL
      AND usage.provider_name <> ''
      AND usage.provider_name NOT IN ('unknown', 'pending')
      AND usage.model IS NOT NULL
      AND usage.model <> ''
) AS usage_base
CROSS JOIN tmp_stats_daily_model_provider_context AS context
WHERE usage_base.day_utc < context.current_day_utc
GROUP BY usage_base.day_utc, usage_base.model, usage_base.provider_name, context.now_utc;
CREATE TEMP TABLE tmp_stats_user_daily_model_provider_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc;

TRUNCATE TABLE stats_user_daily_model_provider;

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
            usage_base.user_id,
            ':',
            CAST(usage_base.day_utc AS TEXT),
            ':',
            usage_base.model,
            ':',
            usage_base.provider_name
        )
    ),
    usage_base.user_id,
    MAX(usage_base.username),
    usage_base.day_utc,
    usage_base.model,
    usage_base.provider_name,
    COUNT(usage_base.id)::INTEGER,
    COALESCE(SUM(GREATEST(COALESCE(usage_base.total_tokens, 0), 0)), 0)::BIGINT,
    COALESCE(SUM(COALESCE(CAST(usage_base.total_cost_usd AS DOUBLE PRECISION), 0)), 0)::DOUBLE PRECISION,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(usage_base.response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    COALESCE(
        SUM(
            CASE
                WHEN usage_base.response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    )::BIGINT AS response_time_samples,
    context.now_utc,
    context.now_utc
FROM (
    SELECT
        usage.id,
        usage.user_id,
        usage.username,
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        usage.model,
        usage.provider_name,
        usage.total_tokens,
        usage.total_cost_usd,
        usage.response_time_ms
    FROM usage
    WHERE usage.user_id IS NOT NULL
      AND usage.status NOT IN ('pending', 'streaming')
      AND usage.provider_name IS NOT NULL
      AND usage.provider_name <> ''
      AND usage.provider_name NOT IN ('unknown', 'pending')
      AND usage.model IS NOT NULL
      AND usage.model <> ''
) AS usage_base
CROSS JOIN tmp_stats_user_daily_model_provider_context AS context
WHERE usage_base.day_utc < context.current_day_utc
GROUP BY
    usage_base.day_utc,
    usage_base.user_id,
    usage_base.model,
    usage_base.provider_name,
    context.now_utc;
CREATE TEMP TABLE tmp_stats_daily_ephemeral_cache_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc;

UPDATE stats_daily
SET
    cache_creation_ephemeral_5m_tokens = 0,
    cache_creation_ephemeral_1h_tokens = 0;

WITH aggregated AS (
    SELECT
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens_5m, 0), 0)), 0)::BIGINT
            AS cache_creation_ephemeral_5m_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens_1h, 0), 0)), 0)::BIGINT
            AS cache_creation_ephemeral_1h_tokens
    FROM usage
    CROSS JOIN tmp_stats_daily_ephemeral_cache_context AS context
    WHERE usage.created_at < context.current_day_utc
      AND usage.status NOT IN ('pending', 'streaming')
      AND usage.provider_name NOT IN ('unknown', 'pending')
    GROUP BY day_utc
)
UPDATE stats_daily
SET
    cache_creation_ephemeral_5m_tokens = aggregated.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = aggregated.cache_creation_ephemeral_1h_tokens,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_daily_ephemeral_cache_context AS context
WHERE stats_daily.date = aggregated.day_utc;

UPDATE stats_user_daily
SET
    cache_creation_ephemeral_5m_tokens = 0,
    cache_creation_ephemeral_1h_tokens = 0;

WITH aggregated AS (
    SELECT
        usage.user_id,
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens_5m, 0), 0)), 0)::BIGINT
            AS cache_creation_ephemeral_5m_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens_1h, 0), 0)), 0)::BIGINT
            AS cache_creation_ephemeral_1h_tokens
    FROM usage
    CROSS JOIN tmp_stats_daily_ephemeral_cache_context AS context
    WHERE usage.user_id IS NOT NULL
      AND usage.created_at < context.current_day_utc
      AND usage.status NOT IN ('pending', 'streaming')
      AND usage.provider_name NOT IN ('unknown', 'pending')
    GROUP BY usage.user_id, day_utc
)
UPDATE stats_user_daily
SET
    cache_creation_ephemeral_5m_tokens = aggregated.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = aggregated.cache_creation_ephemeral_1h_tokens,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_daily_ephemeral_cache_context AS context
WHERE stats_user_daily.user_id = aggregated.user_id
  AND stats_user_daily.date = aggregated.day_utc;
CREATE TEMP TABLE tmp_stats_cache_hit_request_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc,
    (date_trunc('hour', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_hour_utc;

UPDATE stats_daily
SET
    cache_hit_total_requests = 0,
    cache_hit_requests = 0;

WITH aggregated AS (
    SELECT
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        COUNT(*)::BIGINT AS cache_hit_total_requests,
        COUNT(*) FILTER (
            WHERE GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) > 0
        )::BIGINT AS cache_hit_requests
    FROM usage
    CROSS JOIN tmp_stats_cache_hit_request_context AS context
    WHERE usage.created_at < context.current_day_utc
    GROUP BY day_utc
)
UPDATE stats_daily
SET
    cache_hit_total_requests = aggregated.cache_hit_total_requests,
    cache_hit_requests = aggregated.cache_hit_requests,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_cache_hit_request_context AS context
WHERE stats_daily.date = aggregated.day_utc;

UPDATE stats_hourly
SET
    cache_hit_total_requests = 0,
    cache_hit_requests = 0;

WITH aggregated AS (
    SELECT
        (date_trunc('hour', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
        COUNT(*)::BIGINT AS cache_hit_total_requests,
        COUNT(*) FILTER (
            WHERE GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) > 0
        )::BIGINT AS cache_hit_requests
    FROM usage
    CROSS JOIN tmp_stats_cache_hit_request_context AS context
    WHERE usage.created_at < context.current_hour_utc
    GROUP BY hour_utc
)
UPDATE stats_hourly
SET
    cache_hit_total_requests = aggregated.cache_hit_total_requests,
    cache_hit_requests = aggregated.cache_hit_requests,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_cache_hit_request_context AS context
WHERE stats_hourly.hour_utc = aggregated.hour_utc;
CREATE TEMP TABLE tmp_stats_completed_cache_affinity_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc,
    (date_trunc('hour', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_hour_utc;

UPDATE stats_daily
SET
    completed_total_requests = 0,
    completed_cache_hit_requests = 0,
    completed_input_tokens = 0,
    completed_cache_creation_tokens = 0,
    completed_cache_read_tokens = 0,
    completed_total_input_context = 0,
    completed_cache_creation_cost = 0,
    completed_cache_read_cost = 0;

WITH aggregated AS (
    SELECT
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        COUNT(*)::BIGINT AS completed_total_requests,
        COUNT(*) FILTER (
            WHERE GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) > 0
        )::BIGINT AS completed_cache_hit_requests,
        COALESCE(SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)), 0)::BIGINT
            AS completed_input_tokens,
        COALESCE(SUM(
            CASE
                WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                     AND (
                        COALESCE(usage.cache_creation_input_tokens_5m, 0)
                        + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                     ) > 0
                THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                   + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                ELSE COALESCE(usage.cache_creation_input_tokens, 0)
            END
        ), 0)::BIGINT AS completed_cache_creation_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0)::BIGINT
            AS completed_cache_read_tokens,
        COALESCE(SUM(
            CASE
                WHEN split_part(lower(COALESCE(COALESCE(usage.endpoint_api_format, usage.api_format), '')), ':', 1)
                     IN ('claude', 'anthropic')
                THEN GREATEST(COALESCE(usage.input_tokens, 0), 0)
                   + CASE
                       WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                            AND (
                              COALESCE(usage.cache_creation_input_tokens_5m, 0)
                              + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                            ) > 0
                       THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                          + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                       ELSE COALESCE(usage.cache_creation_input_tokens, 0)
                     END
                   + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                WHEN split_part(lower(COALESCE(COALESCE(usage.endpoint_api_format, usage.api_format), '')), ':', 1)
                     IN ('openai', 'gemini', 'google')
                THEN (
                    CASE
                        WHEN GREATEST(COALESCE(usage.input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(usage.input_tokens, 0), 0)
                        ELSE GREATEST(
                            GREATEST(COALESCE(usage.input_tokens, 0), 0)
                                - GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0),
                            0
                        )
                    END
                ) + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                ELSE CASE
                    WHEN (
                        CASE
                            WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                                 AND (
                                    COALESCE(usage.cache_creation_input_tokens_5m, 0)
                                    + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                                 ) > 0
                            THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                               + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                            ELSE COALESCE(usage.cache_creation_input_tokens, 0)
                        END
                    ) > 0
                    THEN GREATEST(COALESCE(usage.input_tokens, 0), 0)
                       + (
                           CASE
                               WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                                    AND (
                                      COALESCE(usage.cache_creation_input_tokens_5m, 0)
                                      + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                                    ) > 0
                               THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                                  + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                               ELSE COALESCE(usage.cache_creation_input_tokens, 0)
                           END
                         )
                       + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                    ELSE GREATEST(COALESCE(usage.input_tokens, 0), 0)
                       + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                END
            END
        ), 0)::BIGINT AS completed_total_input_context,
        COALESCE(SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS completed_cache_creation_cost,
        COALESCE(SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS completed_cache_read_cost
    FROM usage
    CROSS JOIN tmp_stats_completed_cache_affinity_context AS context
    WHERE usage.created_at < context.current_day_utc
      AND usage.status = 'completed'
    GROUP BY day_utc
)
UPDATE stats_daily
SET
    completed_total_requests = aggregated.completed_total_requests,
    completed_cache_hit_requests = aggregated.completed_cache_hit_requests,
    completed_input_tokens = aggregated.completed_input_tokens,
    completed_cache_creation_tokens = aggregated.completed_cache_creation_tokens,
    completed_cache_read_tokens = aggregated.completed_cache_read_tokens,
    completed_total_input_context = aggregated.completed_total_input_context,
    completed_cache_creation_cost = aggregated.completed_cache_creation_cost,
    completed_cache_read_cost = aggregated.completed_cache_read_cost,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_completed_cache_affinity_context AS context
WHERE stats_daily.date = aggregated.day_utc;

UPDATE stats_hourly
SET
    completed_total_requests = 0,
    completed_cache_hit_requests = 0,
    completed_input_tokens = 0,
    completed_cache_creation_tokens = 0,
    completed_cache_read_tokens = 0,
    completed_total_input_context = 0,
    completed_cache_creation_cost = 0,
    completed_cache_read_cost = 0;

WITH aggregated AS (
    SELECT
        (date_trunc('hour', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
        COUNT(*)::BIGINT AS completed_total_requests,
        COUNT(*) FILTER (
            WHERE GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) > 0
        )::BIGINT AS completed_cache_hit_requests,
        COALESCE(SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)), 0)::BIGINT
            AS completed_input_tokens,
        COALESCE(SUM(
            CASE
                WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                     AND (
                        COALESCE(usage.cache_creation_input_tokens_5m, 0)
                        + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                     ) > 0
                THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                   + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                ELSE COALESCE(usage.cache_creation_input_tokens, 0)
            END
        ), 0)::BIGINT AS completed_cache_creation_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0)::BIGINT
            AS completed_cache_read_tokens,
        COALESCE(SUM(
            CASE
                WHEN split_part(lower(COALESCE(COALESCE(usage.endpoint_api_format, usage.api_format), '')), ':', 1)
                     IN ('claude', 'anthropic')
                THEN GREATEST(COALESCE(usage.input_tokens, 0), 0)
                   + CASE
                       WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                            AND (
                              COALESCE(usage.cache_creation_input_tokens_5m, 0)
                              + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                            ) > 0
                       THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                          + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                       ELSE COALESCE(usage.cache_creation_input_tokens, 0)
                     END
                   + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                WHEN split_part(lower(COALESCE(COALESCE(usage.endpoint_api_format, usage.api_format), '')), ':', 1)
                     IN ('openai', 'gemini', 'google')
                THEN (
                    CASE
                        WHEN GREATEST(COALESCE(usage.input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(usage.input_tokens, 0), 0)
                        ELSE GREATEST(
                            GREATEST(COALESCE(usage.input_tokens, 0), 0)
                                - GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0),
                            0
                        )
                    END
                ) + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                ELSE CASE
                    WHEN (
                        CASE
                            WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                                 AND (
                                    COALESCE(usage.cache_creation_input_tokens_5m, 0)
                                    + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                                 ) > 0
                            THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                               + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                            ELSE COALESCE(usage.cache_creation_input_tokens, 0)
                        END
                    ) > 0
                    THEN GREATEST(COALESCE(usage.input_tokens, 0), 0)
                       + (
                           CASE
                               WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
                                    AND (
                                      COALESCE(usage.cache_creation_input_tokens_5m, 0)
                                      + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                                    ) > 0
                               THEN COALESCE(usage.cache_creation_input_tokens_5m, 0)
                                  + COALESCE(usage.cache_creation_input_tokens_1h, 0)
                               ELSE COALESCE(usage.cache_creation_input_tokens, 0)
                           END
                         )
                       + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                    ELSE GREATEST(COALESCE(usage.input_tokens, 0), 0)
                       + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)
                END
            END
        ), 0)::BIGINT AS completed_total_input_context,
        COALESCE(SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS completed_cache_creation_cost,
        COALESCE(SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS completed_cache_read_cost
    FROM usage
    CROSS JOIN tmp_stats_completed_cache_affinity_context AS context
    WHERE usage.created_at < context.current_hour_utc
      AND usage.status = 'completed'
    GROUP BY hour_utc
)
UPDATE stats_hourly
SET
    completed_total_requests = aggregated.completed_total_requests,
    completed_cache_hit_requests = aggregated.completed_cache_hit_requests,
    completed_input_tokens = aggregated.completed_input_tokens,
    completed_cache_creation_tokens = aggregated.completed_cache_creation_tokens,
    completed_cache_read_tokens = aggregated.completed_cache_read_tokens,
    completed_total_input_context = aggregated.completed_total_input_context,
    completed_cache_creation_cost = aggregated.completed_cache_creation_cost,
    completed_cache_read_cost = aggregated.completed_cache_read_cost,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_completed_cache_affinity_context AS context
WHERE stats_hourly.hour_utc = aggregated.hour_utc;
CREATE TEMP TABLE tmp_stats_settled_cost_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc,
    (date_trunc('hour', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_hour_utc;

UPDATE stats_daily
SET
    settled_total_cost = 0,
    settled_total_requests = 0,
    settled_input_tokens = 0,
    settled_output_tokens = 0,
    settled_cache_creation_tokens = 0,
    settled_cache_read_tokens = 0,
    settled_first_finalized_at_unix_secs = NULL,
    settled_last_finalized_at_unix_secs = NULL;

WITH aggregated AS (
    SELECT
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        COALESCE(SUM(COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS settled_total_cost,
        COUNT(*)::BIGINT AS settled_total_requests,
        COALESCE(SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)), 0)::BIGINT
            AS settled_input_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.output_tokens, 0), 0)), 0)::BIGINT
            AS settled_output_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_creation_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_read_tokens,
        MIN(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_first_finalized_at_unix_secs,
        MAX(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_last_finalized_at_unix_secs
    FROM usage
    CROSS JOIN tmp_stats_settled_cost_context AS context
    WHERE usage.created_at < context.current_day_utc
      AND usage.billing_status = 'settled'
      AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    GROUP BY day_utc
)
UPDATE stats_daily
SET
    settled_total_cost = aggregated.settled_total_cost,
    settled_total_requests = aggregated.settled_total_requests,
    settled_input_tokens = aggregated.settled_input_tokens,
    settled_output_tokens = aggregated.settled_output_tokens,
    settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
    settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_settled_cost_context AS context
WHERE stats_daily.date = aggregated.day_utc;

UPDATE stats_hourly
SET
    settled_total_cost = 0,
    settled_total_requests = 0,
    settled_input_tokens = 0,
    settled_output_tokens = 0,
    settled_cache_creation_tokens = 0,
    settled_cache_read_tokens = 0,
    settled_first_finalized_at_unix_secs = NULL,
    settled_last_finalized_at_unix_secs = NULL;

WITH aggregated AS (
    SELECT
        (date_trunc('hour', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
        COALESCE(SUM(COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS settled_total_cost,
        COUNT(*)::BIGINT AS settled_total_requests,
        COALESCE(SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)), 0)::BIGINT
            AS settled_input_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.output_tokens, 0), 0)), 0)::BIGINT
            AS settled_output_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_creation_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_read_tokens,
        MIN(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_first_finalized_at_unix_secs,
        MAX(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_last_finalized_at_unix_secs
    FROM usage
    CROSS JOIN tmp_stats_settled_cost_context AS context
    WHERE usage.created_at < context.current_hour_utc
      AND usage.billing_status = 'settled'
      AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    GROUP BY hour_utc
)
UPDATE stats_hourly
SET
    settled_total_cost = aggregated.settled_total_cost,
    settled_total_requests = aggregated.settled_total_requests,
    settled_input_tokens = aggregated.settled_input_tokens,
    settled_output_tokens = aggregated.settled_output_tokens,
    settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
    settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_settled_cost_context AS context
WHERE stats_hourly.hour_utc = aggregated.hour_utc;

UPDATE stats_user_daily
SET
    settled_total_cost = 0,
    settled_total_requests = 0,
    settled_input_tokens = 0,
    settled_output_tokens = 0,
    settled_cache_creation_tokens = 0,
    settled_cache_read_tokens = 0,
    settled_first_finalized_at_unix_secs = NULL,
    settled_last_finalized_at_unix_secs = NULL;

WITH aggregated AS (
    SELECT
        usage.user_id,
        (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
        COALESCE(SUM(COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS settled_total_cost,
        COUNT(*)::BIGINT AS settled_total_requests,
        COALESCE(SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)), 0)::BIGINT
            AS settled_input_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.output_tokens, 0), 0)), 0)::BIGINT
            AS settled_output_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_creation_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_read_tokens,
        MIN(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_first_finalized_at_unix_secs,
        MAX(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_last_finalized_at_unix_secs
    FROM usage
    CROSS JOIN tmp_stats_settled_cost_context AS context
    WHERE usage.created_at < context.current_day_utc
      AND usage.user_id IS NOT NULL
      AND usage.billing_status = 'settled'
      AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    GROUP BY usage.user_id, day_utc
)
UPDATE stats_user_daily
SET
    settled_total_cost = aggregated.settled_total_cost,
    settled_total_requests = aggregated.settled_total_requests,
    settled_input_tokens = aggregated.settled_input_tokens,
    settled_output_tokens = aggregated.settled_output_tokens,
    settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
    settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_settled_cost_context AS context
WHERE stats_user_daily.user_id = aggregated.user_id
  AND stats_user_daily.date = aggregated.day_utc;

UPDATE stats_hourly_user
SET
    settled_total_cost = 0,
    settled_total_requests = 0,
    settled_input_tokens = 0,
    settled_output_tokens = 0,
    settled_cache_creation_tokens = 0,
    settled_cache_read_tokens = 0,
    settled_first_finalized_at_unix_secs = NULL,
    settled_last_finalized_at_unix_secs = NULL;

WITH aggregated AS (
    SELECT
        usage.user_id,
        (date_trunc('hour', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS hour_utc,
        COALESCE(SUM(COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0)), 0)
            AS settled_total_cost,
        COUNT(*)::BIGINT AS settled_total_requests,
        COALESCE(SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)), 0)::BIGINT
            AS settled_input_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.output_tokens, 0), 0)), 0)::BIGINT
            AS settled_output_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_creation_tokens,
        COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0)::BIGINT
            AS settled_cache_read_tokens,
        MIN(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_first_finalized_at_unix_secs,
        MAX(CAST(EXTRACT(EPOCH FROM usage.finalized_at) AS BIGINT))
            AS settled_last_finalized_at_unix_secs
    FROM usage
    CROSS JOIN tmp_stats_settled_cost_context AS context
    WHERE usage.created_at < context.current_hour_utc
      AND usage.user_id IS NOT NULL
      AND usage.billing_status = 'settled'
      AND COALESCE(CAST(usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    GROUP BY usage.user_id, hour_utc
)
UPDATE stats_hourly_user
SET
    settled_total_cost = aggregated.settled_total_cost,
    settled_total_requests = aggregated.settled_total_requests,
    settled_input_tokens = aggregated.settled_input_tokens,
    settled_output_tokens = aggregated.settled_output_tokens,
    settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
    settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs,
    updated_at = context.now_utc
FROM aggregated
CROSS JOIN tmp_stats_settled_cost_context AS context
WHERE stats_hourly_user.user_id = aggregated.user_id
  AND stats_hourly_user.hour_utc = aggregated.hour_utc;
CREATE TEMP TABLE tmp_stats_cost_savings_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc;

CREATE TEMP TABLE tmp_stats_cost_savings_source ON COMMIT DROP AS
SELECT
    usage.user_id,
    usage.username,
    COALESCE(usage.provider_name, '') AS provider_name,
    COALESCE(usage.model, '') AS model,
    (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
    GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::BIGINT AS cache_read_tokens,
    COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0) AS cache_read_cost,
    COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0) AS cache_creation_cost,
    COALESCE(
        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
        0
    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION / 1000000.0
        AS estimated_full_cost
FROM usage
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = usage.request_id
CROSS JOIN tmp_stats_cost_savings_context AS context
WHERE usage.created_at < context.current_day_utc;

TRUNCATE TABLE
    stats_daily_cost_savings,
    stats_daily_cost_savings_provider,
    stats_daily_cost_savings_model,
    stats_daily_cost_savings_model_provider,
    stats_user_daily_cost_savings,
    stats_user_daily_cost_savings_provider,
    stats_user_daily_cost_savings_model,
    stats_user_daily_cost_savings_model_provider;

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
    md5(CONCAT('stats-daily-cost-savings:', CAST(source.day_utc AS TEXT))),
    source.day_utc,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
GROUP BY source.day_utc, context.now_utc;

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
            CAST(source.day_utc AS TEXT),
            ':',
            source.provider_name
        )
    ),
    source.day_utc,
    source.provider_name,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
GROUP BY source.day_utc, source.provider_name, context.now_utc;

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
            CAST(source.day_utc AS TEXT),
            ':',
            source.model
        )
    ),
    source.day_utc,
    source.model,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
GROUP BY source.day_utc, source.model, context.now_utc;

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
            CAST(source.day_utc AS TEXT),
            ':',
            source.model,
            ':',
            source.provider_name
        )
    ),
    source.day_utc,
    source.model,
    source.provider_name,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
GROUP BY source.day_utc, source.model, source.provider_name, context.now_utc;

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
            source.user_id,
            ':',
            CAST(source.day_utc AS TEXT)
        )
    ),
    source.user_id,
    MAX(source.username),
    source.day_utc,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
WHERE source.user_id IS NOT NULL
GROUP BY source.user_id, source.day_utc, context.now_utc;

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
            source.user_id,
            ':',
            CAST(source.day_utc AS TEXT),
            ':',
            source.provider_name
        )
    ),
    source.user_id,
    MAX(source.username),
    source.day_utc,
    source.provider_name,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
WHERE source.user_id IS NOT NULL
GROUP BY source.user_id, source.day_utc, source.provider_name, context.now_utc;

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
            source.user_id,
            ':',
            CAST(source.day_utc AS TEXT),
            ':',
            source.model
        )
    ),
    source.user_id,
    MAX(source.username),
    source.day_utc,
    source.model,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
WHERE source.user_id IS NOT NULL
GROUP BY source.user_id, source.day_utc, source.model, context.now_utc;

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
            source.user_id,
            ':',
            CAST(source.day_utc AS TEXT),
            ':',
            source.model,
            ':',
            source.provider_name
        )
    ),
    source.user_id,
    MAX(source.username),
    source.day_utc,
    source.model,
    source.provider_name,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_stats_cost_savings_source AS source
CROSS JOIN tmp_stats_cost_savings_context AS context
WHERE source.user_id IS NOT NULL
GROUP BY
    source.user_id,
    source.day_utc,
    source.model,
    source.provider_name,
    context.now_utc;
