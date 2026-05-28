CREATE TABLE IF NOT EXISTS public.webhook_endpoints (
    id character varying(64) PRIMARY KEY,
    name character varying(128) NOT NULL,
    url character varying(1000) NOT NULL,
    secret_encrypted text,
    enabled boolean NOT NULL DEFAULT true,
    event_filter_json jsonb,
    timeout_ms integer NOT NULL DEFAULT 5000,
    max_attempts integer NOT NULL DEFAULT 5,
    initial_retry_delay_seconds integer NOT NULL DEFAULT 60,
    max_retry_delay_seconds integer NOT NULL DEFAULT 3600,
    last_test_delivery_id character varying(64),
    last_test_status character varying(32),
    last_test_error text,
    last_test_at_unix_secs bigint,
    created_by character varying(64),
    created_at_unix_secs bigint NOT NULL,
    updated_at_unix_secs bigint NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_enabled
    ON public.webhook_endpoints (enabled);
CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_updated
    ON public.webhook_endpoints (updated_at_unix_secs);

CREATE TABLE IF NOT EXISTS public.webhook_deliveries (
    id character varying(64) PRIMARY KEY,
    endpoint_id character varying(64) NOT NULL REFERENCES public.webhook_endpoints(id) ON DELETE CASCADE,
    event_id character varying(128) NOT NULL,
    event_type character varying(128) NOT NULL,
    is_test boolean NOT NULL DEFAULT false,
    payload_json jsonb NOT NULL,
    status character varying(32) NOT NULL DEFAULT 'pending',
    attempt_count integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 5,
    next_attempt_at_unix_secs bigint NOT NULL,
    last_attempt_at_unix_secs bigint,
    delivered_at_unix_secs bigint,
    failed_at_unix_secs bigint,
    last_http_status integer,
    last_error text,
    response_excerpt text,
    lease_owner character varying(200),
    lease_expires_at_unix_secs bigint,
    created_at_unix_secs bigint NOT NULL,
    updated_at_unix_secs bigint NOT NULL,
    CONSTRAINT uq_webhook_deliveries_endpoint_event UNIQUE (endpoint_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_endpoint_created
    ON public.webhook_deliveries (endpoint_id, created_at_unix_secs);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_status_next
    ON public.webhook_deliveries (status, next_attempt_at_unix_secs);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_event
    ON public.webhook_deliveries (event_type, event_id);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_lease
    ON public.webhook_deliveries (lease_owner, lease_expires_at_unix_secs);

CREATE TABLE IF NOT EXISTS public.webhook_delivery_attempts (
    id character varying(64) PRIMARY KEY,
    delivery_id character varying(64) NOT NULL REFERENCES public.webhook_deliveries(id) ON DELETE CASCADE,
    attempt_index integer NOT NULL,
    status character varying(32) NOT NULL,
    http_status integer,
    error_message text,
    response_excerpt text,
    started_at_unix_secs bigint NOT NULL,
    completed_at_unix_secs bigint,
    duration_ms integer,
    CONSTRAINT uq_webhook_attempts_delivery_index UNIQUE (delivery_id, attempt_index)
);

CREATE INDEX IF NOT EXISTS idx_webhook_attempts_delivery_created
    ON public.webhook_delivery_attempts (delivery_id, started_at_unix_secs);
CREATE INDEX IF NOT EXISTS idx_webhook_attempts_status
    ON public.webhook_delivery_attempts (status, started_at_unix_secs);
