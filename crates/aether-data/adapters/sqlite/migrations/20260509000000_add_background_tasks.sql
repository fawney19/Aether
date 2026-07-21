CREATE TABLE IF NOT EXISTS background_task_runs (
    id TEXT PRIMARY KEY,
    task_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    "trigger" TEXT NOT NULL,
    status TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 0,
    owner_instance TEXT,
    progress_percent INTEGER NOT NULL DEFAULT 0,
    progress_message TEXT,
    payload_json TEXT,
    result_json TEXT,
    error_message TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    created_by TEXT,
    created_at_unix_secs INTEGER NOT NULL,
    started_at_unix_secs INTEGER,
    finished_at_unix_secs INTEGER,
    updated_at_unix_secs INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_background_task_runs_task_key
    ON background_task_runs (task_key);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_status
    ON background_task_runs (status);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_kind
    ON background_task_runs (kind);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_created_at
    ON background_task_runs (created_at_unix_secs DESC);

CREATE TABLE IF NOT EXISTS background_task_events (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    message TEXT NOT NULL,
    payload_json TEXT,
    created_at_unix_secs INTEGER NOT NULL,
    FOREIGN KEY (run_id) REFERENCES background_task_runs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_background_task_events_run_id
    ON background_task_events (run_id, created_at_unix_secs ASC);
