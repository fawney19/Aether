UPDATE api_keys
SET
  total_requests = GREATEST(COALESCE(total_requests, 0) + $2, 0),
  total_tokens = GREATEST(COALESCE(total_tokens, 0) + $3, 0),
  total_cost_usd = CAST(
    GREATEST(CAST(COALESCE(total_cost_usd, 0) AS DOUBLE PRECISION) + $4, 0) AS NUMERIC(20,8)
  ),
  last_used_at = CASE
    WHEN $5::double precision IS NOT NULL THEN CASE
      WHEN last_used_at IS NULL THEN TO_TIMESTAMP($5::double precision)
      ELSE GREATEST(last_used_at, TO_TIMESTAMP($5::double precision))
    END
    WHEN $6::double precision IS NOT NULL
      AND last_used_at IS NOT NULL
      AND EXTRACT(EPOCH FROM last_used_at)::BIGINT = $6::BIGINT
    THEN (
      SELECT MAX(created_at)
      FROM "usage"
      WHERE api_key_id = $1
    )
    ELSE last_used_at
  END
WHERE id = $1
