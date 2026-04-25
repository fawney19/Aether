SELECT
  api_key_id,
  COALESCE(
    SUM(
      COALESCE(
        total_tokens,
        COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
      )
    ),
    0
  ) AS total_tokens
FROM usage_billing_facts AS "usage"
WHERE api_key_id = ANY($1::TEXT[])
GROUP BY api_key_id
ORDER BY api_key_id ASC
