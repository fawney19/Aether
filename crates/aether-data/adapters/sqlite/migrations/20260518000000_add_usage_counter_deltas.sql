CREATE TABLE IF NOT EXISTS usage_counter_deltas (
    id TEXT PRIMARY KEY NOT NULL,
    request_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    request_count_delta INTEGER NOT NULL DEFAULT 0,
    total_requests_delta INTEGER NOT NULL DEFAULT 0,
    success_count_delta INTEGER NOT NULL DEFAULT 0,
    error_count_delta INTEGER NOT NULL DEFAULT 0,
    dns_failures_delta INTEGER NOT NULL DEFAULT 0,
    stream_errors_delta INTEGER NOT NULL DEFAULT 0,
    total_tokens_delta INTEGER NOT NULL DEFAULT 0,
    total_cost_usd_delta REAL NOT NULL DEFAULT 0,
    total_response_time_ms_delta INTEGER NOT NULL DEFAULT 0,
    last_used_at_unix_secs INTEGER,
    last_used_ip TEXT,
    candidate_last_used_at_unix_secs INTEGER,
    removed_last_used_at_unix_secs INTEGER,
    usage_created_at_unix_secs INTEGER,
    created_at INTEGER NOT NULL,
    processed_at INTEGER
);
CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_unprocessed
    ON usage_counter_deltas (created_at, id);
CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_processed
    ON usage_counter_deltas (processed_at, created_at, id);
CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_request_kind
    ON usage_counter_deltas (request_id, kind, target_id);

CREATE INDEX IF NOT EXISTS video_tasks_due_poll_idx
    ON video_tasks (status, next_poll_at, updated_at);

CREATE INDEX IF NOT EXISTS idx_entitlement_usage_entitlement_date
    ON entitlement_usage_ledgers (user_entitlement_id, usage_date);
