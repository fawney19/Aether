-- Empty databases do not need a concurrent build: the snapshot runs before
-- traffic starts and executes inside a transaction.
CREATE INDEX IF NOT EXISTS idx_usage_legacy_body_ref_cleanup_created_at
ON public.usage (created_at, id)
WHERE request_metadata IS NOT NULL
  AND (
    request_metadata::jsonb ? 'request_body_ref'
    OR request_metadata::jsonb ? 'provider_request_body_ref'
    OR request_metadata::jsonb ? 'response_body_ref'
    OR request_metadata::jsonb ? 'client_response_body_ref'
  );
