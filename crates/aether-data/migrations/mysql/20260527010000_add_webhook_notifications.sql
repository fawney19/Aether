CREATE TABLE IF NOT EXISTS webhook_endpoints (
    id VARCHAR(64) PRIMARY KEY,
    name VARCHAR(128) NOT NULL,
    url VARCHAR(1000) NOT NULL,
    secret_encrypted LONGTEXT,
    enabled TINYINT(1) NOT NULL DEFAULT 1,
    event_filter_json JSON,
    timeout_ms INT NOT NULL DEFAULT 5000,
    max_attempts INT NOT NULL DEFAULT 5,
    initial_retry_delay_seconds INT NOT NULL DEFAULT 60,
    max_retry_delay_seconds INT NOT NULL DEFAULT 3600,
    last_test_delivery_id VARCHAR(64),
    last_test_status VARCHAR(32),
    last_test_error LONGTEXT,
    last_test_at_unix_secs BIGINT,
    created_by VARCHAR(64),
    created_at_unix_secs BIGINT NOT NULL,
    updated_at_unix_secs BIGINT NOT NULL,
    INDEX idx_webhook_endpoints_enabled (enabled),
    INDEX idx_webhook_endpoints_updated (updated_at_unix_secs)
);

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id VARCHAR(64) PRIMARY KEY,
    endpoint_id VARCHAR(64) NOT NULL,
    event_id VARCHAR(128) NOT NULL,
    event_type VARCHAR(128) NOT NULL,
    is_test TINYINT(1) NOT NULL DEFAULT 0,
    payload_json JSON NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'pending',
    attempt_count INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 5,
    next_attempt_at_unix_secs BIGINT NOT NULL,
    last_attempt_at_unix_secs BIGINT,
    delivered_at_unix_secs BIGINT,
    failed_at_unix_secs BIGINT,
    last_http_status INT,
    last_error LONGTEXT,
    response_excerpt LONGTEXT,
    lease_owner VARCHAR(200),
    lease_expires_at_unix_secs BIGINT,
    created_at_unix_secs BIGINT NOT NULL,
    updated_at_unix_secs BIGINT NOT NULL,
    UNIQUE KEY uq_webhook_deliveries_endpoint_event (endpoint_id, event_id),
    INDEX idx_webhook_deliveries_endpoint_created (endpoint_id, created_at_unix_secs),
    INDEX idx_webhook_deliveries_status_next (status, next_attempt_at_unix_secs),
    INDEX idx_webhook_deliveries_event (event_type, event_id),
    INDEX idx_webhook_deliveries_lease (lease_owner, lease_expires_at_unix_secs),
    CONSTRAINT fk_webhook_deliveries_endpoint
        FOREIGN KEY (endpoint_id) REFERENCES webhook_endpoints(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS webhook_delivery_attempts (
    id VARCHAR(64) PRIMARY KEY,
    delivery_id VARCHAR(64) NOT NULL,
    attempt_index INT NOT NULL,
    status VARCHAR(32) NOT NULL,
    http_status INT,
    error_message LONGTEXT,
    response_excerpt LONGTEXT,
    started_at_unix_secs BIGINT NOT NULL,
    completed_at_unix_secs BIGINT,
    duration_ms INT,
    UNIQUE KEY uq_webhook_attempts_delivery_index (delivery_id, attempt_index),
    INDEX idx_webhook_attempts_delivery_created (delivery_id, started_at_unix_secs),
    INDEX idx_webhook_attempts_status (status, started_at_unix_secs),
    CONSTRAINT fk_webhook_attempts_delivery
        FOREIGN KEY (delivery_id) REFERENCES webhook_deliveries(id) ON DELETE CASCADE
);
