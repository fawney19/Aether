CREATE TABLE IF NOT EXISTS risk_control_logs (
    id TEXT NOT NULL PRIMARY KEY,
    trace_id TEXT NOT NULL,
    request_id TEXT,
    user_id TEXT,
    username TEXT,
    user_email TEXT,
    api_key_id TEXT,
    api_key_name TEXT,
    route_family TEXT,
    route_kind TEXT,
    api_format TEXT,
    endpoint TEXT NOT NULL,
    model TEXT,
    mode TEXT NOT NULL,
    action TEXT NOT NULL,
    decision_source TEXT NOT NULL,
    flagged INTEGER NOT NULL DEFAULT 0,
    highest_category TEXT,
    highest_score REAL NOT NULL DEFAULT 0,
    category_scores TEXT,
    thresholds TEXT,
    matched_keywords TEXT,
    input_hash TEXT,
    excerpt TEXT,
    latency_ms INTEGER,
    queue_delay_ms INTEGER,
    violation_count INTEGER NOT NULL DEFAULT 0,
    auto_action TEXT,
    notification_sent INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS risk_control_flagged_hashes (
    input_hash TEXT NOT NULL PRIMARY KEY,
    source_log_id TEXT,
    reason TEXT,
    highest_category TEXT,
    highest_score REAL NOT NULL DEFAULT 0,
    excerpt TEXT,
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_risk_control_logs_created_at
    ON risk_control_logs (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_flagged_created_at
    ON risk_control_logs (flagged, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_user_created_at
    ON risk_control_logs (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_api_key_created_at
    ON risk_control_logs (api_key_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_endpoint_created_at
    ON risk_control_logs (endpoint, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_input_hash
    ON risk_control_logs (input_hash);
CREATE INDEX IF NOT EXISTS idx_risk_control_hashes_last_seen_at
    ON risk_control_flagged_hashes (last_seen_at DESC);

INSERT OR IGNORE INTO system_configs (
    id,
    key,
    value,
    description,
    created_at,
    updated_at
)
VALUES
    ('risk-control-enabled', 'module.risk_control.enabled', 'false', 'Risk control center enabled', CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER)),
    ('risk-control-config', 'module.risk_control.config', '{}', 'Risk control center config', CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER));
