INSERT INTO "usage" (
  id,
  request_id,
  user_id,
  api_key_id,
  provider_name,
  model,
  target_model,
  provider_id,
  provider_endpoint_id,
  provider_api_key_id,
  request_type,
  api_format,
  api_family,
  endpoint_kind,
  endpoint_api_format,
  provider_api_family,
  provider_endpoint_kind,
  has_format_conversion,
  is_stream,
  upstream_is_stream,
  status_code,
  response_time_ms,
  first_byte_time_ms,
  status,
  billing_status,
  request_metadata,
  created_at,
  updated_at_unix_secs
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  $14,
  $15,
  $16,
  $17,
  COALESCE($18, FALSE),
  TRUE,
  COALESCE(
    CASE
      WHEN ($22::json->>'upstream_is_stream') IN ('true', 'false')
      THEN ($22::json->>'upstream_is_stream')::boolean
      ELSE NULL
    END,
    TRUE
  ),
  $19,
  $20,
  $21,
  'streaming',
  'pending',
  $22::json,
  COALESCE(TO_TIMESTAMP($23::double precision), NOW()),
  COALESCE(
    NULLIF($24::bigint, 0),
    CAST(EXTRACT(EPOCH FROM COALESCE(TO_TIMESTAMP($23::double precision), NOW())) AS BIGINT)
  )
)
ON CONFLICT (request_id)
DO UPDATE SET
  user_id = COALESCE(EXCLUDED.user_id, "usage".user_id),
  api_key_id = COALESCE(EXCLUDED.api_key_id, "usage".api_key_id),
  provider_name = EXCLUDED.provider_name,
  model = EXCLUDED.model,
  target_model = COALESCE(EXCLUDED.target_model, "usage".target_model),
  provider_id = COALESCE(EXCLUDED.provider_id, "usage".provider_id),
  provider_endpoint_id = COALESCE(EXCLUDED.provider_endpoint_id, "usage".provider_endpoint_id),
  provider_api_key_id = COALESCE(EXCLUDED.provider_api_key_id, "usage".provider_api_key_id),
  request_type = COALESCE(EXCLUDED.request_type, "usage".request_type),
  api_format = COALESCE(EXCLUDED.api_format, "usage".api_format),
  api_family = COALESCE(EXCLUDED.api_family, "usage".api_family),
  endpoint_kind = COALESCE(EXCLUDED.endpoint_kind, "usage".endpoint_kind),
  endpoint_api_format = COALESCE(EXCLUDED.endpoint_api_format, "usage".endpoint_api_format),
  provider_api_family = COALESCE(EXCLUDED.provider_api_family, "usage".provider_api_family),
  provider_endpoint_kind = COALESCE(EXCLUDED.provider_endpoint_kind, "usage".provider_endpoint_kind),
  has_format_conversion = COALESCE($18, "usage".has_format_conversion, FALSE),
  is_stream = TRUE,
  upstream_is_stream = COALESCE(
    CASE
      WHEN ($22::json->>'upstream_is_stream') IN ('true', 'false')
      THEN ($22::json->>'upstream_is_stream')::boolean
      ELSE NULL
    END,
    "usage".upstream_is_stream,
    "usage".is_stream,
    TRUE
  ),
  status_code = COALESCE(EXCLUDED.status_code, "usage".status_code),
  response_time_ms = CASE
    WHEN EXCLUDED.response_time_ms IS NULL OR EXCLUDED.response_time_ms = 0
      THEN "usage".response_time_ms
    ELSE EXCLUDED.response_time_ms
  END,
  first_byte_time_ms = CASE
    WHEN "usage".first_byte_time_ms IS NOT NULL AND "usage".first_byte_time_ms <> 0
      THEN "usage".first_byte_time_ms
    WHEN EXCLUDED.first_byte_time_ms IS NULL OR EXCLUDED.first_byte_time_ms = 0
      THEN "usage".first_byte_time_ms
    ELSE EXCLUDED.first_byte_time_ms
  END,
  status = 'streaming',
  request_metadata = COALESCE("usage".request_metadata, EXCLUDED.request_metadata),
  updated_at_unix_secs = GREATEST(
    COALESCE(NULLIF("usage".updated_at_unix_secs, 0), 0),
    COALESCE(NULLIF(EXCLUDED.updated_at_unix_secs, 0), 0),
    CAST(EXTRACT(EPOCH FROM "usage".created_at) AS BIGINT)
  )
WHERE "usage".billing_status = 'pending'
  AND "usage".status IN ('pending', 'streaming')
  AND "usage".finalized_at IS NULL
RETURNING
  request_id,
  provider_api_key_id,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs
