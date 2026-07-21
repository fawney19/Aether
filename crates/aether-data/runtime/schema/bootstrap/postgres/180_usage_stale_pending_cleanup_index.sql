-- Empty databases do not need a concurrent build: the snapshot runs before
-- traffic starts and executes inside a transaction.
CREATE INDEX IF NOT EXISTS idx_usage_stale_pending_created_request
ON public.usage USING btree (created_at, request_id)
WHERE status IN ('pending', 'streaming');
