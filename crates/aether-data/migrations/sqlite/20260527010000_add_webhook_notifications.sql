CREATE TABLE IF NOT EXISTS webhook_endpoints (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    secret_encrypted TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    event_filter_json TEXT,
    timeout_ms INTEGER NOT NULL DEFAULT 5000,
    max_attempts INTEGER NOT NULL DEFAULT 5,
    initial_retry_delay_seconds INTEGER NOT NULL DEFAULT 60,
    max_retry_delay_seconds INTEGER NOT NULL DEFAULT 3600,
    last_test_delivery_id TEXT,
    last_test_status TEXT,
    last_test_error TEXT,
    last_test_at_unix_secs INTEGER,
    created_by TEXT,
    created_at_unix_secs INTEGER NOT NULL,
    updated_at_unix_secs INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_enabled
    ON webhook_endpoints (enabled);
CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_updated
    ON webhook_endpoints (updated_at_unix_secs);

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id TEXT PRIMARY KEY,
    endpoint_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    is_test INTEGER NOT NULL DEFAULT 0,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 5,
    next_attempt_at_unix_secs INTEGER NOT NULL,
    last_attempt_at_unix_secs INTEGER,
    delivered_at_unix_secs INTEGER,
    failed_at_unix_secs INTEGER,
    last_http_status INTEGER,
    last_error TEXT,
    response_excerpt TEXT,
    lease_owner TEXT,
    lease_expires_at_unix_secs INTEGER,
    created_at_unix_secs INTEGER NOT NULL,
    updated_at_unix_secs INTEGER NOT NULL,
    UNIQUE (endpoint_id, event_id),
    CONSTRAINT fk_webhook_deliveries_endpoint
        FOREIGN KEY (endpoint_id) REFERENCES webhook_endpoints(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_endpoint_created
    ON webhook_deliveries (endpoint_id, created_at_unix_secs);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_status_next
    ON webhook_deliveries (status, next_attempt_at_unix_secs);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_event
    ON webhook_deliveries (event_type, event_id);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_lease
    ON webhook_deliveries (lease_owner, lease_expires_at_unix_secs);

CREATE TABLE IF NOT EXISTS webhook_delivery_attempts (
    id TEXT PRIMARY KEY,
    delivery_id TEXT NOT NULL,
    attempt_index INTEGER NOT NULL,
    status TEXT NOT NULL,
    http_status INTEGER,
    error_message TEXT,
    response_excerpt TEXT,
    started_at_unix_secs INTEGER NOT NULL,
    completed_at_unix_secs INTEGER,
    duration_ms INTEGER,
    UNIQUE (delivery_id, attempt_index),
    CONSTRAINT fk_webhook_attempts_delivery
        FOREIGN KEY (delivery_id) REFERENCES webhook_deliveries(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_webhook_attempts_delivery_created
    ON webhook_delivery_attempts (delivery_id, started_at_unix_secs);
CREATE INDEX IF NOT EXISTS idx_webhook_attempts_status
    ON webhook_delivery_attempts (status, started_at_unix_secs);
