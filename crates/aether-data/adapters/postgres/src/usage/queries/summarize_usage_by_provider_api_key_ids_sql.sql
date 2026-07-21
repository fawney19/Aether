SELECT
  id AS provider_api_key_id,
  COALESCE(request_count, 0)::BIGINT AS request_count,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  CAST(COALESCE(total_cost_usd, 0) AS DOUBLE PRECISION) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs
FROM provider_api_keys
WHERE id = ANY($1::TEXT[])
  AND COALESCE(request_count, 0) > 0
ORDER BY id ASC
