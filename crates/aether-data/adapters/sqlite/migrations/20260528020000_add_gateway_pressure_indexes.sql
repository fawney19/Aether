-- High-concurrency gateway read/cleanup paths.
-- SQLite remains single-node/lightweight but benefits from the same bounded scans.

CREATE INDEX IF NOT EXISTS idx_usage_created_id_desc
    ON "usage" (created_at_unix_ms DESC, request_id ASC);

CREATE INDEX IF NOT EXISTS idx_usage_user_created_id_desc
    ON "usage" (user_id, created_at_unix_ms DESC, request_id ASC);

CREATE INDEX IF NOT EXISTS idx_usage_api_format_created_id_desc
    ON "usage" (api_format, created_at_unix_ms DESC, request_id ASC);

CREATE INDEX IF NOT EXISTS idx_usage_status_created_id_desc
    ON "usage" (status, created_at_unix_ms DESC, request_id ASC);

CREATE INDEX IF NOT EXISTS idx_request_candidates_provider_created
    ON request_candidates (provider_id, created_at DESC, id ASC);

CREATE INDEX IF NOT EXISTS idx_request_candidates_api_key_created
    ON request_candidates (api_key_id, created_at ASC, id ASC);

CREATE INDEX IF NOT EXISTS idx_background_task_runs_status_created
    ON background_task_runs (status, created_at_unix_secs DESC, updated_at_unix_secs DESC);

CREATE INDEX IF NOT EXISTS idx_background_task_runs_kind_created
    ON background_task_runs (kind, created_at_unix_secs DESC, updated_at_unix_secs DESC);
