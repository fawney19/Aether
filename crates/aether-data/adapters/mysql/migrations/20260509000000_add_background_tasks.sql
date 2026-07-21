CREATE TABLE IF NOT EXISTS background_task_runs (
    id VARCHAR(64) PRIMARY KEY,
    task_key VARCHAR(200) NOT NULL,
    kind VARCHAR(32) NOT NULL,
    `trigger` VARCHAR(64) NOT NULL,
    status VARCHAR(32) NOT NULL,
    attempt INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 0,
    owner_instance VARCHAR(200),
    progress_percent INT NOT NULL DEFAULT 0,
    progress_message TEXT,
    payload_json JSON,
    result_json JSON,
    error_message TEXT,
    cancel_requested TINYINT(1) NOT NULL DEFAULT 0,
    created_by VARCHAR(200),
    created_at_unix_secs BIGINT NOT NULL,
    started_at_unix_secs BIGINT NULL,
    finished_at_unix_secs BIGINT NULL,
    updated_at_unix_secs BIGINT NOT NULL,
    INDEX idx_background_task_runs_task_key (task_key),
    INDEX idx_background_task_runs_status (status),
    INDEX idx_background_task_runs_kind (kind),
    INDEX idx_background_task_runs_created_at (created_at_unix_secs)
);

CREATE TABLE IF NOT EXISTS background_task_events (
    id VARCHAR(64) PRIMARY KEY,
    run_id VARCHAR(64) NOT NULL,
    event_type VARCHAR(64) NOT NULL,
    message TEXT NOT NULL,
    payload_json JSON,
    created_at_unix_secs BIGINT NOT NULL,
    INDEX idx_background_task_events_run_id (run_id, created_at_unix_secs),
    CONSTRAINT fk_background_task_events_run
        FOREIGN KEY (run_id) REFERENCES background_task_runs(id) ON DELETE CASCADE
);
