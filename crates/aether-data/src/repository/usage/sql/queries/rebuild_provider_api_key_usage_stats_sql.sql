WITH aggregated AS (
  SELECT
    provider_api_key_id,
    COUNT(*)::INTEGER AS request_count,
    COALESCE(SUM(
      CASE
        WHEN status IN ('completed', 'success', 'ok', 'billed', 'settled')
             AND (status_code IS NULL OR status_code < 400)
             AND NULLIF(BTRIM(error_message), '') IS NULL
        THEN 1
        ELSE 0
      END
    ), 0)::INTEGER AS success_count,
    COALESCE(SUM(
      CASE
        WHEN status NOT IN ('pending', 'streaming')
             AND NOT (
               status IN ('completed', 'success', 'ok', 'billed', 'settled')
               AND (status_code IS NULL OR status_code < 400)
               AND NULLIF(BTRIM(error_message), '') IS NULL
             )
        THEN 1
        ELSE 0
      END
    ), 0)::INTEGER AS error_count,
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
    COALESCE(SUM(
      CASE
        WHEN status IN ('completed', 'success', 'ok', 'billed', 'settled')
             AND (status_code IS NULL OR status_code < 400)
             AND NULLIF(BTRIM(error_message), '') IS NULL
             AND response_time_ms IS NOT NULL
        THEN GREATEST(response_time_ms, 0)
        ELSE 0
      END
    ), 0)::INTEGER AS total_response_time_ms,
    MAX(created_at) AS last_used_at
  FROM usage_billing_facts AS "usage"
  WHERE provider_api_key_id IS NOT NULL
    AND BTRIM(provider_api_key_id) <> ''
  GROUP BY provider_api_key_id
)
UPDATE provider_api_keys
SET
  request_count = aggregated.request_count,
  success_count = aggregated.success_count,
  error_count = aggregated.error_count,
  total_tokens = aggregated.total_tokens,
  total_cost_usd = aggregated.total_cost_usd,
  total_response_time_ms = aggregated.total_response_time_ms,
  last_used_at = aggregated.last_used_at
FROM aggregated
WHERE provider_api_keys.id = aggregated.provider_api_key_id
