CREATE TABLE IF NOT EXISTS public.risk_control_notification_outbox (
    id character varying(36) NOT NULL,
    log_id character varying(36) NOT NULL,
    item_key character varying(120) NOT NULL,
    title character varying(255) NOT NULL,
    markdown_body text NOT NULL,
    text_body text NOT NULL,
    variables_json jsonb NOT NULL,
    status character varying(32) NOT NULL,
    attempt_count bigint DEFAULT 0 NOT NULL,
    max_attempts bigint DEFAULT 10 NOT NULL,
    next_attempt_at bigint,
    lease_until bigint,
    last_error text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    sent_at bigint
);

ALTER TABLE ONLY public.risk_control_notification_outbox
    ADD CONSTRAINT risk_control_notification_outbox_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.risk_control_notification_outbox
    ADD CONSTRAINT uq_risk_control_notification_outbox_log_item UNIQUE (log_id, item_key);

CREATE INDEX IF NOT EXISTS idx_risk_control_notification_outbox_due
    ON public.risk_control_notification_outbox USING btree (status, next_attempt_at);

CREATE INDEX IF NOT EXISTS idx_risk_control_notification_outbox_lease
    ON public.risk_control_notification_outbox USING btree (status, lease_until);

CREATE INDEX IF NOT EXISTS idx_risk_control_notification_outbox_updated
    ON public.risk_control_notification_outbox USING btree (updated_at);
