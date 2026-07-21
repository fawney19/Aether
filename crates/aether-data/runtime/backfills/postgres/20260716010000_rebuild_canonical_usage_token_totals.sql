CREATE TEMP TABLE tmp_canonical_usage_token_totals ON COMMIT DROP AS
SELECT
    usage.user_id,
    usage.api_key_id,
    usage.provider_api_key_id,
    usage.provider_name,
    usage.model,
    usage.api_format,
    (date_trunc('day', usage.created_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') AS day_utc,
    GREATEST(COALESCE(usage.total_tokens, 0), 0)::BIGINT AS total_tokens
FROM public.usage_billing_facts AS usage
WHERE usage.status NOT IN ('pending', 'streaming');

CREATE INDEX tmp_canonical_usage_token_totals_day_idx
    ON tmp_canonical_usage_token_totals (day_utc);
CREATE INDEX tmp_canonical_usage_token_totals_api_key_idx
    ON tmp_canonical_usage_token_totals (api_key_id);
CREATE INDEX tmp_canonical_usage_token_totals_provider_key_idx
    ON tmp_canonical_usage_token_totals (provider_api_key_id);

ANALYZE tmp_canonical_usage_token_totals;

WITH aggregated AS (
    SELECT
        day_utc,
        model,
        provider_name,
        COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens
    FROM tmp_canonical_usage_token_totals
    WHERE model IS NOT NULL
      AND model <> ''
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY day_utc, model, provider_name
)
UPDATE public.stats_daily_model_provider AS target
SET
    total_tokens = aggregated.total_tokens,
    updated_at = NOW()
FROM aggregated
WHERE target.date = aggregated.day_utc
  AND target.model = aggregated.model
  AND target.provider_name = aggregated.provider_name;

WITH aggregated AS (
    SELECT
        user_id,
        day_utc,
        model,
        COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens
    FROM tmp_canonical_usage_token_totals
    WHERE user_id IS NOT NULL
      AND model IS NOT NULL
      AND model <> ''
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, day_utc, model
)
UPDATE public.stats_user_daily_model AS target
SET
    total_tokens = aggregated.total_tokens,
    updated_at = NOW()
FROM aggregated
WHERE target.user_id = aggregated.user_id
  AND target.date = aggregated.day_utc
  AND target.model = aggregated.model;

WITH aggregated AS (
    SELECT
        user_id,
        day_utc,
        provider_name,
        COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens
    FROM tmp_canonical_usage_token_totals
    WHERE user_id IS NOT NULL
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, day_utc, provider_name
)
UPDATE public.stats_user_daily_provider AS target
SET
    total_tokens = aggregated.total_tokens,
    updated_at = NOW()
FROM aggregated
WHERE target.user_id = aggregated.user_id
  AND target.date = aggregated.day_utc
  AND target.provider_name = aggregated.provider_name;

WITH aggregated AS (
    SELECT
        user_id,
        day_utc,
        api_format,
        COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens
    FROM tmp_canonical_usage_token_totals
    WHERE user_id IS NOT NULL
      AND api_format IS NOT NULL
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, day_utc, api_format
)
UPDATE public.stats_user_daily_api_format AS target
SET
    total_tokens = aggregated.total_tokens,
    updated_at = NOW()
FROM aggregated
WHERE target.user_id = aggregated.user_id
  AND target.date = aggregated.day_utc
  AND target.api_format = aggregated.api_format;

WITH aggregated AS (
    SELECT
        user_id,
        day_utc,
        model,
        provider_name,
        COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens
    FROM tmp_canonical_usage_token_totals
    WHERE user_id IS NOT NULL
      AND model IS NOT NULL
      AND model <> ''
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, day_utc, model, provider_name
)
UPDATE public.stats_user_daily_model_provider AS target
SET
    total_tokens = aggregated.total_tokens,
    updated_at = NOW()
FROM aggregated
WHERE target.user_id = aggregated.user_id
  AND target.date = aggregated.day_utc
  AND target.model = aggregated.model
  AND target.provider_name = aggregated.provider_name;

UPDATE public.api_keys
SET total_tokens = 0;

WITH aggregated AS (
    SELECT
        api_key_id,
        COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens
    FROM tmp_canonical_usage_token_totals
    WHERE api_key_id IS NOT NULL
      AND BTRIM(api_key_id) <> ''
    GROUP BY api_key_id
)
UPDATE public.api_keys AS target
SET total_tokens = aggregated.total_tokens
FROM aggregated
WHERE target.id = aggregated.api_key_id;

UPDATE public.provider_api_keys
SET total_tokens = 0;

WITH aggregated AS (
    SELECT
        provider_api_key_id,
        COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens
    FROM tmp_canonical_usage_token_totals
    WHERE provider_api_key_id IS NOT NULL
      AND BTRIM(provider_api_key_id) <> ''
    GROUP BY provider_api_key_id
)
UPDATE public.provider_api_keys AS target
SET total_tokens = aggregated.total_tokens
FROM aggregated
WHERE target.id = aggregated.provider_api_key_id;
