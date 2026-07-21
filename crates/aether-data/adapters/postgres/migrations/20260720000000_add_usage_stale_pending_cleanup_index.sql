-- no-transaction
-- Stale pending cleanup only visits the small active subset of the append-heavy
-- usage table. Matching its ordering also avoids sorting before SKIP LOCKED.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_usage_stale_pending_created_request
ON public.usage USING btree (created_at, request_id)
WHERE status IN ('pending', 'streaming');
