UPDATE public.api_keys
SET
    total_requests = 0,
    total_tokens = 0,
    total_cost_usd = 0,
    last_used_at = NULL;

WITH aggregated AS (
    SELECT
        usage.api_key_id,
        COUNT(*)::INTEGER AS total_requests,
        COALESCE(
            SUM(
                GREATEST(
                    COALESCE(
                        usage.total_tokens,
                        COALESCE(usage.input_tokens, 0) + COALESCE(usage.output_tokens, 0)
                    ),
                    0
                )::BIGINT
            ),
            0
        )::BIGINT AS total_tokens,
        COALESCE(SUM(COALESCE(usage.total_cost_usd, 0)), 0)::NUMERIC(20,8) AS total_cost_usd,
        MAX(usage.created_at) AS last_used_at
    FROM public.usage
    WHERE usage.api_key_id IS NOT NULL
      AND BTRIM(usage.api_key_id) <> ''
    GROUP BY usage.api_key_id
)
UPDATE public.api_keys
SET
    total_requests = aggregated.total_requests,
    total_tokens = aggregated.total_tokens,
    total_cost_usd = aggregated.total_cost_usd,
    last_used_at = aggregated.last_used_at
FROM aggregated
WHERE public.api_keys.id = aggregated.api_key_id;
