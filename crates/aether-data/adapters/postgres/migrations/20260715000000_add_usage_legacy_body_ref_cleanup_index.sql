-- no-transaction
-- Bound the legacy body-reference cleanup scan to rows that can actually
-- contain legacy metadata. Without this partial index, an empty cleanup batch
-- scans the full historical usage table on every maintenance cycle.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_usage_legacy_body_ref_cleanup_created_at
ON usage (created_at, id)
WHERE request_metadata IS NOT NULL
  AND (
    request_metadata::jsonb ? 'request_body_ref'
    OR request_metadata::jsonb ? 'provider_request_body_ref'
    OR request_metadata::jsonb ? 'response_body_ref'
    OR request_metadata::jsonb ? 'client_response_body_ref'
  );
