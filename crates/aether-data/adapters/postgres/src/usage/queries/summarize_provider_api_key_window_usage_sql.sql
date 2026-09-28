WITH requested AS (
  SELECT
    request_row.provider_api_key_id,
    request_row.window_code,
    request_row.start_unix_secs,
    request_row.end_unix_secs,
    request_row.include_model_prefix,
    request_row.exclude_model_prefix,
    request_row.ordinality
  FROM UNNEST(
    $1::TEXT[],
    $2::TEXT[],
    $3::BIGINT[],
    $4::BIGINT[],
    $5::TEXT[],
    $6::TEXT[]
  ) WITH ORDINALITY AS request_row(
    provider_api_key_id,
    window_code,
    start_unix_secs,
    end_unix_secs,
    include_model_prefix,
    exclude_model_prefix,
    ordinality
  )
)
SELECT
  requested.provider_api_key_id,
  requested.window_code,
  COUNT("usage".id)::BIGINT AS request_count,
  COALESCE(SUM("usage".total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM("usage".total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM requested
LEFT JOIN usage_billing_facts AS "usage"
  ON "usage".provider_api_key_id = requested.provider_api_key_id
 AND "usage".created_at >= to_timestamp(requested.start_unix_secs::DOUBLE PRECISION)
 AND "usage".created_at < to_timestamp(requested.end_unix_secs::DOUBLE PRECISION)
 AND (
   requested.include_model_prefix IS NULL
   OR starts_with(
     LOWER(COALESCE(NULLIF(BTRIM("usage".target_model), ''), BTRIM("usage".model), '')),
     requested.include_model_prefix
   )
 )
 AND (
   requested.exclude_model_prefix IS NULL
   OR NOT starts_with(
     LOWER(COALESCE(NULLIF(BTRIM("usage".target_model), ''), BTRIM("usage".model), '')),
     requested.exclude_model_prefix
   )
 )
GROUP BY
  requested.provider_api_key_id,
  requested.window_code,
  requested.ordinality
ORDER BY requested.ordinality ASC
