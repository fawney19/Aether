SELECT
  api_key_id,
  COALESCE(
    SUM(
      GREATEST(COALESCE(total_tokens, 0), 0)
    ),
    0
  )::BIGINT AS total_tokens
FROM usage_billing_facts AS "usage"
WHERE api_key_id = ANY($1::TEXT[])
GROUP BY api_key_id
ORDER BY api_key_id ASC
