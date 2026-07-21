CREATE TEMP TABLE tmp_rebuild_cost_savings_context ON COMMIT DROP AS
SELECT
    NOW() AS now_utc,
    (date_trunc('day', NOW() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS current_day_utc;

CREATE TEMP TABLE tmp_rebuild_cost_savings_source ON COMMIT DROP AS
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
        CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION),
        CAST(usage.input_price_per_1m AS DOUBLE PRECISION),
        0
    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION / 1000000.0
        AS estimated_full_cost
FROM usage_billing_facts AS usage
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = usage.request_id
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
    md5(CONCAT('stats-daily-cost-savings-provider:', CAST(source.day_utc AS TEXT), ':', source.provider_name)),
    source.day_utc,
    source.provider_name,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
    md5(CONCAT('stats-daily-cost-savings-model:', CAST(source.day_utc AS TEXT), ':', source.model)),
    source.day_utc,
    source.model,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
    md5(CONCAT('stats-daily-cost-savings-model-provider:', CAST(source.day_utc AS TEXT), ':', source.model, ':', source.provider_name)),
    source.day_utc,
    source.model,
    source.provider_name,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
    md5(CONCAT('stats-user-daily-cost-savings:', source.user_id, ':', CAST(source.day_utc AS TEXT))),
    source.user_id,
    MAX(source.username),
    source.day_utc,
    COALESCE(SUM(source.cache_read_tokens), 0)::BIGINT,
    CAST(COALESCE(SUM(source.cache_read_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.cache_creation_cost), 0) AS DOUBLE PRECISION),
    CAST(COALESCE(SUM(source.estimated_full_cost), 0) AS DOUBLE PRECISION),
    context.now_utc,
    context.now_utc
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
    md5(CONCAT('stats-user-daily-cost-savings-provider:', source.user_id, ':', CAST(source.day_utc AS TEXT), ':', source.provider_name)),
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
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
    md5(CONCAT('stats-user-daily-cost-savings-model:', source.user_id, ':', CAST(source.day_utc AS TEXT), ':', source.model)),
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
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
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
    md5(CONCAT('stats-user-daily-cost-savings-model-provider:', source.user_id, ':', CAST(source.day_utc AS TEXT), ':', source.model, ':', source.provider_name)),
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
FROM tmp_rebuild_cost_savings_source AS source
CROSS JOIN tmp_rebuild_cost_savings_context AS context
WHERE source.user_id IS NOT NULL
GROUP BY
    source.user_id,
    source.day_utc,
    source.model,
    source.provider_name,
    context.now_utc;
