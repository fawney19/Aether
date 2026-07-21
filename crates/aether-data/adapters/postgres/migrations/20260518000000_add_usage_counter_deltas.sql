CREATE TABLE IF NOT EXISTS public.usage_counter_deltas (
    id character varying(36) NOT NULL,
    request_id character varying(128) NOT NULL,
    kind character varying(64) NOT NULL,
    target_id text NOT NULL,
    request_count_delta bigint DEFAULT 0 NOT NULL,
    total_requests_delta bigint DEFAULT 0 NOT NULL,
    success_count_delta bigint DEFAULT 0 NOT NULL,
    error_count_delta bigint DEFAULT 0 NOT NULL,
    dns_failures_delta bigint DEFAULT 0 NOT NULL,
    stream_errors_delta bigint DEFAULT 0 NOT NULL,
    total_tokens_delta bigint DEFAULT 0 NOT NULL,
    total_cost_usd_delta double precision DEFAULT 0 NOT NULL,
    total_response_time_ms_delta bigint DEFAULT 0 NOT NULL,
    last_used_at_unix_secs bigint,
    last_used_ip text,
    candidate_last_used_at_unix_secs bigint,
    removed_last_used_at_unix_secs bigint,
    usage_created_at_unix_secs bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    processed_at timestamp with time zone,
    CONSTRAINT usage_counter_deltas_pkey PRIMARY KEY (id),
    CONSTRAINT usage_counter_deltas_kind_check CHECK (
        kind IN (
            'api_key',
            'provider_api_key',
            'model',
            'provider_monthly',
            'proxy_node',
            'management_token',
            'api_key_last_used'
        )
    )
);

CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_unprocessed
    ON public.usage_counter_deltas USING btree (created_at, id)
    WHERE processed_at IS NULL;

CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_processed
    ON public.usage_counter_deltas USING btree (processed_at, created_at, id)
    WHERE processed_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_request_kind
    ON public.usage_counter_deltas USING btree (request_id, kind, target_id);

CREATE INDEX IF NOT EXISTS idx_entitlement_usage_entitlement_date
    ON public.entitlement_usage_ledgers USING btree (user_entitlement_id, usage_date);

CREATE INDEX IF NOT EXISTS idx_video_tasks_due_poll
    ON public.video_tasks USING btree (status, next_poll_at, updated_at)
    WHERE next_poll_at IS NOT NULL;

ALTER TABLE IF EXISTS public.api_keys
  ALTER COLUMN total_requests TYPE bigint USING COALESCE(total_requests, 0)::bigint;

ALTER TABLE IF EXISTS public.global_models
  ALTER COLUMN usage_count TYPE bigint USING COALESCE(usage_count, 0)::bigint;

ALTER TABLE IF EXISTS public.management_tokens
  ALTER COLUMN usage_count TYPE bigint USING COALESCE(usage_count, 0)::bigint;

ALTER TABLE IF EXISTS public.provider_api_keys
  ALTER COLUMN request_count TYPE bigint USING COALESCE(request_count, 0)::bigint,
  ALTER COLUMN success_count TYPE bigint USING COALESCE(success_count, 0)::bigint,
  ALTER COLUMN error_count TYPE bigint USING COALESCE(error_count, 0)::bigint,
  ALTER COLUMN total_response_time_ms TYPE bigint USING COALESCE(total_response_time_ms, 0)::bigint;
