ALTER TABLE public.risk_control_logs
    ADD COLUMN IF NOT EXISTS notification_attempts bigint DEFAULT 0 NOT NULL;

ALTER TABLE public.risk_control_logs
    ADD COLUMN IF NOT EXISTS notification_last_error text;

ALTER TABLE public.risk_control_logs
    ADD COLUMN IF NOT EXISTS notification_last_attempt_at bigint;
