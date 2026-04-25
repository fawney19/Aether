INSERT INTO usage_http_audits (
  request_id,
  request_headers,
  provider_request_headers,
  response_headers,
  client_response_headers,
  request_body_ref,
  provider_request_body_ref,
  response_body_ref,
  client_response_body_ref,
  request_body_state,
  provider_request_body_state,
  response_body_state,
  client_response_body_state,
  body_capture_mode
) VALUES (
  $1,
  $2::json,
  $3::json,
  $4::json,
  $5::json,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  $14
)
ON CONFLICT (request_id)
DO UPDATE SET
  request_headers = COALESCE(EXCLUDED.request_headers, usage_http_audits.request_headers),
  provider_request_headers = COALESCE(
    EXCLUDED.provider_request_headers,
    usage_http_audits.provider_request_headers
  ),
  response_headers = COALESCE(EXCLUDED.response_headers, usage_http_audits.response_headers),
  client_response_headers = COALESCE(
    EXCLUDED.client_response_headers,
    usage_http_audits.client_response_headers
  ),
  request_body_ref = COALESCE(EXCLUDED.request_body_ref, usage_http_audits.request_body_ref),
  provider_request_body_ref = COALESCE(
    EXCLUDED.provider_request_body_ref,
    usage_http_audits.provider_request_body_ref
  ),
  response_body_ref = COALESCE(EXCLUDED.response_body_ref, usage_http_audits.response_body_ref),
  client_response_body_ref = COALESCE(
    EXCLUDED.client_response_body_ref,
    usage_http_audits.client_response_body_ref
  ),
  request_body_state = COALESCE(
    EXCLUDED.request_body_state,
    usage_http_audits.request_body_state
  ),
  provider_request_body_state = COALESCE(
    EXCLUDED.provider_request_body_state,
    usage_http_audits.provider_request_body_state
  ),
  response_body_state = COALESCE(
    EXCLUDED.response_body_state,
    usage_http_audits.response_body_state
  ),
  client_response_body_state = COALESCE(
    EXCLUDED.client_response_body_state,
    usage_http_audits.client_response_body_state
  ),
  body_capture_mode = COALESCE(
    NULLIF(EXCLUDED.body_capture_mode, 'none'),
    usage_http_audits.body_capture_mode,
    'none'
  ),
  updated_at = NOW()
