CREATE TABLE IF NOT EXISTS risk_control_logs (
    id VARCHAR(36) NOT NULL,
    trace_id VARCHAR(100) NOT NULL,
    request_id VARCHAR(100),
    user_id VARCHAR(36),
    username VARCHAR(100),
    user_email VARCHAR(255),
    api_key_id VARCHAR(36),
    api_key_name VARCHAR(255),
    route_family VARCHAR(80),
    route_kind VARCHAR(80),
    api_format VARCHAR(80),
    endpoint VARCHAR(255) NOT NULL,
    model VARCHAR(255),
    mode VARCHAR(32) NOT NULL,
    action VARCHAR(32) NOT NULL,
    decision_source VARCHAR(32) NOT NULL,
    flagged TINYINT(1) NOT NULL DEFAULT 0,
    highest_category VARCHAR(100),
    highest_score DOUBLE NOT NULL DEFAULT 0,
    category_scores TEXT,
    thresholds TEXT,
    matched_keywords TEXT,
    input_hash VARCHAR(64),
    excerpt TEXT,
    latency_ms BIGINT,
    queue_delay_ms BIGINT,
    violation_count BIGINT NOT NULL DEFAULT 0,
    auto_action VARCHAR(64),
    notification_sent TINYINT(1) NOT NULL DEFAULT 0,
    error_message TEXT,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (id),
    KEY idx_risk_control_logs_created_at (created_at),
    KEY idx_risk_control_logs_flagged_created_at (flagged, created_at),
    KEY idx_risk_control_logs_user_created_at (user_id, created_at),
    KEY idx_risk_control_logs_api_key_created_at (api_key_id, created_at),
    KEY idx_risk_control_logs_endpoint_created_at (endpoint, created_at),
    KEY idx_risk_control_logs_input_hash (input_hash)
);

CREATE TABLE IF NOT EXISTS risk_control_flagged_hashes (
    input_hash VARCHAR(64) NOT NULL,
    source_log_id VARCHAR(36),
    reason VARCHAR(255),
    highest_category VARCHAR(100),
    highest_score DOUBLE NOT NULL DEFAULT 0,
    excerpt TEXT,
    first_seen_at BIGINT NOT NULL,
    last_seen_at BIGINT NOT NULL,
    hit_count BIGINT NOT NULL DEFAULT 1,
    PRIMARY KEY (input_hash),
    KEY idx_risk_control_hashes_last_seen_at (last_seen_at)
);

INSERT IGNORE INTO system_configs (
    id,
    `key`,
    value,
    description,
    created_at,
    updated_at
)
VALUES
    ('risk-control-enabled', 'module.risk_control.enabled', 'false', 'Risk control center enabled', UNIX_TIMESTAMP(), UNIX_TIMESTAMP()),
    ('risk-control-config', 'module.risk_control.config', '{}', 'Risk control center config', UNIX_TIMESTAMP(), UNIX_TIMESTAMP());
