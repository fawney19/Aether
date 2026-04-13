COMMENT ON COLUMN public.usage.billing_status IS
  'DEPRECATED: authoritative owner moved to public.usage_settlement_snapshots.billing_status. Compatibility/index mirror only; do not write new values.';
COMMENT ON COLUMN public.usage.finalized_at IS
  'DEPRECATED: authoritative owner moved to public.usage_settlement_snapshots.finalized_at. Compatibility/index mirror only; do not write new values.';
COMMENT ON COLUMN public.usage.request_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.request_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.provider_request_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.provider_request_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.response_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.response_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.client_response_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.client_response_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.request_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.request_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.provider_request_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.provider_request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.provider_request_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.provider_request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.response_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.response_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.response_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.response_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.client_response_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.client_response_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.client_response_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.client_response_body_ref. Legacy compatibility only; do not write new values.';
