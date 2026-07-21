-- Squashed unreleased usage schema split:
--   20260412000000_add_usage_body_blobs.sql
--   20260412010000_add_usage_http_audits.sql
--   20260412020000_add_usage_routing_snapshots.sql
--   20260412030000_add_usage_settlement_snapshots.sql
--   20260413000000_expand_usage_settlement_snapshots_for_pricing.sql
--   20260413010000_mark_usage_legacy_columns_deprecated.sql
--   20260413020000_add_candidate_index_to_usage_routing_snapshots.sql

CREATE TABLE IF NOT EXISTS public.usage_body_blobs (
    body_ref character varying(160) NOT NULL,
    request_id character varying(100) NOT NULL,
    body_field character varying(50) NOT NULL,
    payload_gzip bytea NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_body_blobs_pkey PRIMARY KEY (body_ref),
    CONSTRAINT usage_body_blobs_request_id_field_key UNIQUE (request_id, body_field),
    CONSTRAINT usage_body_blobs_request_id_fkey
        FOREIGN KEY (request_id)
        REFERENCES public.usage(request_id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS ix_usage_body_blobs_request_id
    ON public.usage_body_blobs USING btree (request_id);

CREATE TABLE IF NOT EXISTS public.usage_http_audits (
    request_id character varying(100) NOT NULL,
    request_headers json,
    provider_request_headers json,
    response_headers json,
    client_response_headers json,
    request_body_ref character varying(160),
    provider_request_body_ref character varying(160),
    response_body_ref character varying(160),
    client_response_body_ref character varying(160),
    request_body_state character varying(32),
    provider_request_body_state character varying(32),
    response_body_state character varying(32),
    client_response_body_state character varying(32),
    body_capture_mode character varying(32) DEFAULT 'none' NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_http_audits_pkey PRIMARY KEY (request_id),
    CONSTRAINT usage_http_audits_request_id_fkey
        FOREIGN KEY (request_id)
        REFERENCES public.usage(request_id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS ix_usage_http_audits_updated_at
    ON public.usage_http_audits USING btree (updated_at);

CREATE TABLE IF NOT EXISTS public.usage_routing_snapshots (
    request_id character varying(100) NOT NULL,
    candidate_id character varying(160),
    candidate_index integer,
    key_name character varying(255),
    planner_kind character varying(120),
    route_family character varying(80),
    route_kind character varying(80),
    execution_path character varying(80),
    local_execution_runtime_miss_reason character varying(120),
    selected_provider_id character varying(100),
    selected_endpoint_id character varying(100),
    selected_provider_api_key_id character varying(100),
    has_format_conversion boolean,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_routing_snapshots_pkey PRIMARY KEY (request_id),
    CONSTRAINT usage_routing_snapshots_request_id_fkey
        FOREIGN KEY (request_id)
        REFERENCES public.usage(request_id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS ix_usage_routing_snapshots_route_family_kind
    ON public.usage_routing_snapshots USING btree (route_family, route_kind);

CREATE INDEX IF NOT EXISTS ix_usage_routing_snapshots_candidate_id
    ON public.usage_routing_snapshots USING btree (candidate_id);

CREATE TABLE IF NOT EXISTS public.usage_settlement_snapshots (
    request_id character varying(100) NOT NULL,
    billing_status character varying(20) NOT NULL,
    wallet_id character varying(36),
    wallet_balance_before numeric(20,8),
    wallet_balance_after numeric(20,8),
    wallet_recharge_balance_before numeric(20,8),
    wallet_recharge_balance_after numeric(20,8),
    wallet_gift_balance_before numeric(20,8),
    wallet_gift_balance_after numeric(20,8),
    provider_monthly_used_usd numeric(20,8),
    finalized_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_settlement_snapshots_pkey PRIMARY KEY (request_id),
    CONSTRAINT usage_settlement_snapshots_request_id_fkey
        FOREIGN KEY (request_id)
        REFERENCES public.usage(request_id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS ix_usage_settlement_snapshots_wallet_id
    ON public.usage_settlement_snapshots USING btree (wallet_id);

CREATE INDEX IF NOT EXISTS ix_usage_settlement_snapshots_billing_status
    ON public.usage_settlement_snapshots USING btree (billing_status);

ALTER TABLE IF EXISTS public.usage_settlement_snapshots
    ADD COLUMN IF NOT EXISTS billing_snapshot_schema_version character varying(20),
    ADD COLUMN IF NOT EXISTS billing_snapshot_status character varying(20),
    ADD COLUMN IF NOT EXISTS rate_multiplier numeric(10,6),
    ADD COLUMN IF NOT EXISTS is_free_tier boolean,
    ADD COLUMN IF NOT EXISTS input_price_per_1m numeric(20,8),
    ADD COLUMN IF NOT EXISTS output_price_per_1m numeric(20,8),
    ADD COLUMN IF NOT EXISTS cache_creation_price_per_1m numeric(20,8),
    ADD COLUMN IF NOT EXISTS cache_read_price_per_1m numeric(20,8),
    ADD COLUMN IF NOT EXISTS price_per_request numeric(20,8);

COMMENT ON COLUMN public.usage.wallet_id IS
  'DEPRECATED: settlement owner moved to public.usage_settlement_snapshots.wallet_id. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.wallet_balance_before IS
  'DEPRECATED: settlement owner moved to public.usage_settlement_snapshots.wallet_balance_before. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.wallet_balance_after IS
  'DEPRECATED: settlement owner moved to public.usage_settlement_snapshots.wallet_balance_after. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.wallet_recharge_balance_before IS
  'DEPRECATED: settlement owner moved to public.usage_settlement_snapshots.wallet_recharge_balance_before. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.wallet_recharge_balance_after IS
  'DEPRECATED: settlement owner moved to public.usage_settlement_snapshots.wallet_recharge_balance_after. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.wallet_gift_balance_before IS
  'DEPRECATED: settlement owner moved to public.usage_settlement_snapshots.wallet_gift_balance_before. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.wallet_gift_balance_after IS
  'DEPRECATED: settlement owner moved to public.usage_settlement_snapshots.wallet_gift_balance_after. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.rate_multiplier IS
  'DEPRECATED: settlement pricing owner moved to public.usage_settlement_snapshots.rate_multiplier. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.input_price_per_1m IS
  'DEPRECATED: settlement pricing owner moved to public.usage_settlement_snapshots.input_price_per_1m. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.output_price_per_1m IS
  'DEPRECATED: settlement pricing owner moved to public.usage_settlement_snapshots.output_price_per_1m. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.cache_creation_price_per_1m IS
  'DEPRECATED: settlement pricing owner moved to public.usage_settlement_snapshots.cache_creation_price_per_1m. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.cache_read_price_per_1m IS
  'DEPRECATED: settlement pricing owner moved to public.usage_settlement_snapshots.cache_read_price_per_1m. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.price_per_request IS
  'DEPRECATED: settlement pricing owner moved to public.usage_settlement_snapshots.price_per_request. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.username IS
  'DEPRECATED: display cache only. Prefer join-time lookup from user/auth records. Legacy compatibility only.';
COMMENT ON COLUMN public.usage.api_key_name IS
  'DEPRECATED: display cache only. Prefer join-time lookup from API key records. Legacy compatibility only.';
