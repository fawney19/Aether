SELECT
  "usage".user_id,
  COUNT(*)::BIGINT AS request_count,
  COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)), 0)::BIGINT AS total_tokens
FROM usage_billing_facts AS "usage"
WHERE "usage".user_id = ANY($1::TEXT[])
  AND "usage".status NOT IN ('pending', 'streaming')
  AND "usage".provider_name NOT IN ('unknown', 'pending')
GROUP BY "usage".user_id
ORDER BY "usage".user_id ASC
