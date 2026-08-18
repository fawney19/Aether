WITH aggregated AS (
  SELECT
    provider_api_key_id,
    COUNT(*)::BIGINT AS request_count,
    COUNT(*) FILTER (WHERE sla_eligible)::BIGINT AS sla_eligible_count,
    COALESCE(SUM(
      CASE
        WHEN outcome_class = 'success'
        THEN 1
        ELSE 0
      END
    ), 0)::BIGINT AS success_count,
    COALESCE(SUM(
      CASE
        WHEN outcome_class = 'service_error'
        THEN 1
        ELSE 0
      END
    ), 0)::BIGINT AS error_count,
    COUNT(*) FILTER (WHERE outcome_class = 'user_error')::BIGINT AS user_error_count,
    COALESCE(SUM(
      CASE
        WHEN status IN ('pending', 'streaming') THEN 0
        ELSE GREATEST(
          COALESCE(total_tokens, 0),
          0
        )::BIGINT
      END
    ), 0)::BIGINT AS total_tokens,
    COALESCE(SUM(
      CASE
        WHEN status IN ('pending', 'streaming') THEN 0
        ELSE COALESCE(total_cost_usd, 0)
      END
    ), 0)::NUMERIC(20,8) AS total_cost_usd,
    COALESCE(SUM(
      CASE
        WHEN outcome_class = 'success'
             AND response_time_ms IS NOT NULL
        THEN GREATEST(response_time_ms, 0)
        ELSE 0
      END
    ), 0)::BIGINT AS total_response_time_ms,
    MAX(created_at) AS last_used_at
  FROM usage_billing_facts AS "usage"
  WHERE provider_api_key_id IS NOT NULL
    AND BTRIM(provider_api_key_id) <> ''
  GROUP BY provider_api_key_id
)
UPDATE provider_api_keys
SET
  request_count = aggregated.request_count,
  sla_eligible_count = aggregated.sla_eligible_count,
  success_count = aggregated.success_count,
  error_count = aggregated.error_count,
  user_error_count = aggregated.user_error_count,
  total_tokens = aggregated.total_tokens,
  total_cost_usd = aggregated.total_cost_usd,
  total_response_time_ms = aggregated.total_response_time_ms,
  last_used_at = aggregated.last_used_at
FROM aggregated
WHERE provider_api_keys.id = aggregated.provider_api_key_id
