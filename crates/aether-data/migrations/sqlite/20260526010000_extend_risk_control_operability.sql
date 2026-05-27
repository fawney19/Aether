ALTER TABLE risk_control_logs
    ADD COLUMN notification_attempts INTEGER NOT NULL DEFAULT 0;

ALTER TABLE risk_control_logs
    ADD COLUMN notification_last_error TEXT;

ALTER TABLE risk_control_logs
    ADD COLUMN notification_last_attempt_at INTEGER;
