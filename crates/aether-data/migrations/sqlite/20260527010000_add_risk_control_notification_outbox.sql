CREATE TABLE IF NOT EXISTS risk_control_notification_outbox (
    id TEXT PRIMARY KEY NOT NULL,
    log_id TEXT NOT NULL,
    item_key TEXT NOT NULL,
    title TEXT NOT NULL,
    markdown_body TEXT NOT NULL,
    text_body TEXT NOT NULL,
    variables_json TEXT NOT NULL,
    status TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 10,
    next_attempt_at INTEGER,
    lease_until INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    sent_at INTEGER,
    UNIQUE (log_id, item_key)
);

CREATE INDEX IF NOT EXISTS idx_risk_control_notification_outbox_due
    ON risk_control_notification_outbox (status, next_attempt_at);

CREATE INDEX IF NOT EXISTS idx_risk_control_notification_outbox_lease
    ON risk_control_notification_outbox (status, lease_until);

CREATE INDEX IF NOT EXISTS idx_risk_control_notification_outbox_updated
    ON risk_control_notification_outbox (updated_at);
