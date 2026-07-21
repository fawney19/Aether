INSERT INTO usage_body_blobs (
  body_ref,
  request_id,
  body_field,
  payload_gzip
) VALUES (
  $1,
  $2,
  $3,
  $4
)
ON CONFLICT (body_ref)
DO UPDATE SET
  payload_gzip = EXCLUDED.payload_gzip,
  updated_at = NOW()
