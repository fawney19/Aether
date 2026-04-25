WITH aggregated AS (
  SELECT
    api_key_id,
    COUNT(*)::INTEGER AS total_requests,
    COALESCE(SUM(
      GREATEST(
        COALESCE(
          total_tokens,
          COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
        ),
        0
      )::BIGINT
    ), 0)::BIGINT AS total_tokens,
    COALESCE(SUM(COALESCE(total_cost_usd, 0)), 0)::NUMERIC(20,8) AS total_cost_usd,
    MAX(created_at) AS last_used_at
  FROM usage_billing_facts AS "usage"
  WHERE api_key_id IS NOT NULL
    AND BTRIM(api_key_id) <> ''
  GROUP BY api_key_id
)
UPDATE api_keys
SET
  total_requests = aggregated.total_requests,
  total_tokens = aggregated.total_tokens,
  total_cost_usd = aggregated.total_cost_usd,
  last_used_at = aggregated.last_used_at
FROM aggregated
WHERE api_keys.id = aggregated.api_key_id
