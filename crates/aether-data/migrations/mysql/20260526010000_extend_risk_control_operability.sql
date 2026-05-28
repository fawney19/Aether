ALTER TABLE risk_control_logs
    ADD COLUMN notification_attempts BIGINT NOT NULL DEFAULT 0 AFTER notification_sent,
    ADD COLUMN notification_last_error LONGTEXT AFTER notification_attempts,
    ADD COLUMN notification_last_attempt_at BIGINT AFTER notification_last_error;
