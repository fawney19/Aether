
-- Baseline v2 extension: usage body blobs

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


-- Baseline v2 extension: usage http audits

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


-- Baseline v2 extension: usage routing snapshots

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
    ADD COLUMN IF NOT EXISTS price_per_request numeric(20,8),
    ADD COLUMN IF NOT EXISTS settlement_snapshot_schema_version character varying(20),
    ADD COLUMN IF NOT EXISTS settlement_snapshot jsonb,
    ADD COLUMN IF NOT EXISTS billing_dimensions jsonb,
    ADD COLUMN IF NOT EXISTS billing_input_tokens bigint,
    ADD COLUMN IF NOT EXISTS billing_effective_input_tokens bigint,
    ADD COLUMN IF NOT EXISTS billing_output_tokens bigint,
    ADD COLUMN IF NOT EXISTS billing_cache_creation_tokens bigint,
    ADD COLUMN IF NOT EXISTS billing_cache_creation_5m_tokens bigint,
    ADD COLUMN IF NOT EXISTS billing_cache_creation_1h_tokens bigint,
    ADD COLUMN IF NOT EXISTS billing_cache_read_tokens bigint,
    ADD COLUMN IF NOT EXISTS billing_total_input_context bigint,
    ADD COLUMN IF NOT EXISTS billing_cache_creation_cost_usd numeric(20,8),
    ADD COLUMN IF NOT EXISTS billing_cache_read_cost_usd numeric(20,8),
    ADD COLUMN IF NOT EXISTS billing_total_cost_usd numeric(20,8),
    ADD COLUMN IF NOT EXISTS billing_actual_total_cost_usd numeric(20,8),
    ADD COLUMN IF NOT EXISTS billing_pricing_source character varying(50),
    ADD COLUMN IF NOT EXISTS billing_rule_id character varying(100),
    ADD COLUMN IF NOT EXISTS billing_rule_version character varying(50);

CREATE INDEX IF NOT EXISTS ix_usage_settlement_snapshots_schema_version
    ON public.usage_settlement_snapshots USING btree (settlement_snapshot_schema_version);

CREATE INDEX IF NOT EXISTS ix_usage_settlement_snapshots_pricing_source
    ON public.usage_settlement_snapshots USING btree (billing_pricing_source);

CREATE INDEX IF NOT EXISTS idx_usage_settlement_dashboard_cover
    ON public.usage_settlement_snapshots USING btree (request_id)
    INCLUDE (
      billing_input_tokens,
      billing_effective_input_tokens,
      billing_output_tokens,
      billing_cache_creation_tokens,
      billing_cache_creation_5m_tokens,
      billing_cache_creation_1h_tokens,
      billing_cache_read_tokens,
      billing_total_input_context,
      billing_cache_creation_cost_usd,
      billing_cache_read_cost_usd,
      billing_total_cost_usd,
      billing_actual_total_cost_usd,
      input_price_per_1m
    );

ALTER TABLE public.usage SET (
  autovacuum_analyze_scale_factor = 0.02,
  autovacuum_analyze_threshold = 10000
);

ALTER TABLE public.usage_settlement_snapshots SET (
  autovacuum_analyze_scale_factor = 0.02,
  autovacuum_analyze_threshold = 10000
);

CREATE OR REPLACE VIEW public.usage_billing_facts AS
SELECT
  usage_rows.id,
  usage_rows.request_id,
  usage_rows.user_id,
  usage_rows.api_key_id,
  usage_rows.username,
  usage_rows.api_key_name,
  usage_rows.provider_name,
  usage_rows.model,
  usage_rows.target_model,
  usage_rows.provider_id,
  usage_rows.provider_endpoint_id,
  usage_rows.provider_api_key_id,
  usage_rows.request_type,
  usage_rows.api_format,
  usage_rows.api_family,
  usage_rows.endpoint_kind,
  usage_rows.endpoint_api_format,
  usage_rows.provider_api_family,
  usage_rows.provider_endpoint_kind,
  COALESCE(usage_rows.has_format_conversion, FALSE) AS has_format_conversion,
  COALESCE(usage_rows.is_stream, FALSE) AS is_stream,
  usage_rows.status_code,
  usage_rows.error_message,
  usage_rows.error_category,
  usage_rows.response_time_ms,
  usage_rows.first_byte_time_ms,
  usage_rows.status,
  COALESCE(settlement.billing_status, usage_rows.billing_status) AS billing_status,
  usage_rows.created_at,
  COALESCE(settlement.finalized_at, usage_rows.finalized_at) AS finalized_at,
  GREATEST(COALESCE(settlement.billing_input_tokens, usage_rows.input_tokens, 0), 0)::bigint
    AS input_tokens,
  GREATEST(
    COALESCE(
      settlement.billing_effective_input_tokens,
      CASE
        WHEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0) <= 0 THEN 0
        WHEN split_part(lower(COALESCE(COALESCE(usage_rows.endpoint_api_format, usage_rows.api_format), '')), ':', 1)
             = 'openai'
             AND (
               GREATEST(COALESCE(usage_rows.cache_creation_input_tokens, 0), 0) > 0
               OR GREATEST(COALESCE(usage_rows.cache_creation_input_tokens_5m, 0), 0) > 0
               OR GREATEST(COALESCE(usage_rows.cache_creation_input_tokens_1h, 0), 0) > 0
               OR GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0) > 0
             )
        THEN GREATEST(
          GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
            - GREATEST(
                CASE
                  WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                       AND (
                         COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                         + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                       ) > 0
                  THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                     + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                  ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
                END,
                0
              )
            - GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0),
          0
        )
        WHEN split_part(lower(COALESCE(COALESCE(usage_rows.endpoint_api_format, usage_rows.api_format), '')), ':', 1)
             IN ('gemini', 'google')
             AND GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0) > 0
        THEN GREATEST(
          GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
            - GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0),
          0
        )
        ELSE GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
      END
    ),
    0
  )::bigint AS effective_input_tokens,
  GREATEST(COALESCE(settlement.billing_output_tokens, usage_rows.output_tokens, 0), 0)::bigint
    AS output_tokens,
  GREATEST(
    COALESCE(
      settlement.billing_cache_creation_tokens,
      CASE
        WHEN settlement.billing_cache_creation_5m_tokens IS NOT NULL
          OR settlement.billing_cache_creation_1h_tokens IS NOT NULL
        THEN COALESCE(settlement.billing_cache_creation_5m_tokens, 0)
          + COALESCE(settlement.billing_cache_creation_1h_tokens, 0)
      END,
      CASE
        WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
             AND (
               COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
               + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
             ) > 0
        THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
           + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
        ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
      END,
      0
    ),
    0
  )::bigint AS cache_creation_input_tokens,
  GREATEST(
    COALESCE(
      settlement.billing_cache_creation_5m_tokens,
      usage_rows.cache_creation_input_tokens_5m,
      0
    ),
    0
  )::bigint AS cache_creation_input_tokens_5m,
  GREATEST(
    COALESCE(
      settlement.billing_cache_creation_1h_tokens,
      usage_rows.cache_creation_input_tokens_1h,
      0
    ),
    0
  )::bigint AS cache_creation_input_tokens_1h,
  GREATEST(COALESCE(settlement.billing_cache_read_tokens, usage_rows.cache_read_input_tokens, 0), 0)::bigint
    AS cache_read_input_tokens,
  GREATEST(
    COALESCE(
      CASE
        WHEN settlement.billing_effective_input_tokens IS NOT NULL
        THEN GREATEST(settlement.billing_effective_input_tokens, 0)
          + GREATEST(COALESCE(settlement.billing_output_tokens, usage_rows.output_tokens, 0), 0)
          + GREATEST(
              COALESCE(
                settlement.billing_cache_creation_tokens,
                CASE
                  WHEN settlement.billing_cache_creation_5m_tokens IS NOT NULL
                    OR settlement.billing_cache_creation_1h_tokens IS NOT NULL
                  THEN COALESCE(settlement.billing_cache_creation_5m_tokens, 0)
                    + COALESCE(settlement.billing_cache_creation_1h_tokens, 0)
                END,
                CASE
                  WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                       AND (
                         COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                         + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                       ) > 0
                  THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                    + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                  ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
                END,
                0
              ),
              0
            )
          + GREATEST(
              COALESCE(
                settlement.billing_cache_read_tokens,
                usage_rows.cache_read_input_tokens,
                0
              ),
              0
            )
        WHEN settlement.billing_total_input_context IS NOT NULL
        THEN GREATEST(settlement.billing_total_input_context, 0)
          + GREATEST(COALESCE(settlement.billing_output_tokens, usage_rows.output_tokens, 0), 0)
      END,
      NULLIF(GREATEST(COALESCE(usage_rows.total_tokens, 0), 0), 0),
      CASE
        WHEN split_part(lower(COALESCE(COALESCE(usage_rows.endpoint_api_format, usage_rows.api_format), '')), ':', 1)
             IN ('openai', 'gemini', 'google')
        THEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
          + GREATEST(COALESCE(usage_rows.output_tokens, 0), 0)
        ELSE GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
          + GREATEST(COALESCE(usage_rows.output_tokens, 0), 0)
          + GREATEST(
              CASE
                WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                     AND (
                       COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                       + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                     ) > 0
                THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                  + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
              END,
              0
            )
          + GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0)
      END,
      0
    ),
    0
  )::bigint AS total_tokens,
  GREATEST(
    COALESCE(
      CASE
        WHEN settlement.billing_effective_input_tokens IS NOT NULL
        THEN GREATEST(settlement.billing_effective_input_tokens, 0)
          + GREATEST(
              COALESCE(
                settlement.billing_cache_creation_tokens,
                CASE
                  WHEN settlement.billing_cache_creation_5m_tokens IS NOT NULL
                    OR settlement.billing_cache_creation_1h_tokens IS NOT NULL
                  THEN COALESCE(settlement.billing_cache_creation_5m_tokens, 0)
                    + COALESCE(settlement.billing_cache_creation_1h_tokens, 0)
                END,
                CASE
                  WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                       AND (
                         COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                         + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                       ) > 0
                  THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                    + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                  ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
                END,
                0
              ),
              0
            )
          + GREATEST(
              COALESCE(
                settlement.billing_cache_read_tokens,
                usage_rows.cache_read_input_tokens,
                0
              ),
              0
            )
      END,
      settlement.billing_total_input_context,
      CASE
        WHEN split_part(lower(COALESCE(COALESCE(usage_rows.endpoint_api_format, usage_rows.api_format), '')), ':', 1)
             IN ('claude', 'anthropic')
        THEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
           + CASE
               WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                    AND (
                      COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                      + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                    ) > 0
               THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                  + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
               ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
             END
           + GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0)
        WHEN split_part(lower(COALESCE(COALESCE(usage_rows.endpoint_api_format, usage_rows.api_format), '')), ':', 1)
             = 'openai'
        THEN CASE
               WHEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0) <= 0 THEN 0
               WHEN GREATEST(COALESCE(usage_rows.cache_creation_input_tokens, 0), 0) <= 0
                    AND GREATEST(COALESCE(usage_rows.cache_creation_input_tokens_5m, 0), 0) <= 0
                    AND GREATEST(COALESCE(usage_rows.cache_creation_input_tokens_1h, 0), 0) <= 0
                    AND GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0) <= 0
               THEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
               ELSE GREATEST(
                 GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
                   - GREATEST(
                       CASE
                         WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                              AND (
                                COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                                + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                              ) > 0
                         THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                           + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                         ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
                       END,
                       0
                     )
                   - GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0),
                 0
               )
             END
           + GREATEST(
               CASE
                 WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                      AND (
                        COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                        + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                      ) > 0
                 THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                   + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                 ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
               END,
               0
             )
           + GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0)
        WHEN split_part(lower(COALESCE(COALESCE(usage_rows.endpoint_api_format, usage_rows.api_format), '')), ':', 1)
             IN ('gemini', 'google')
        THEN CASE
               WHEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0) <= 0 THEN 0
               WHEN GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0) <= 0
               THEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
               ELSE GREATEST(
                 GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
                   - GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0),
                 0
               )
             END
           + GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0)
        ELSE GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
           + CASE
               WHEN COALESCE(usage_rows.cache_creation_input_tokens, 0) = 0
                    AND (
                      COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                      + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
                    ) > 0
               THEN COALESCE(usage_rows.cache_creation_input_tokens_5m, 0)
                  + COALESCE(usage_rows.cache_creation_input_tokens_1h, 0)
               ELSE COALESCE(usage_rows.cache_creation_input_tokens, 0)
             END
           + GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0)
      END,
      0
    ),
    0
  )::bigint AS total_input_context,
  COALESCE(CAST(usage_rows.input_cost_usd AS DOUBLE PRECISION), 0) AS input_cost_usd,
  COALESCE(CAST(usage_rows.output_cost_usd AS DOUBLE PRECISION), 0) AS output_cost_usd,
  COALESCE(
    CAST(settlement.billing_cache_creation_cost_usd AS DOUBLE PRECISION),
    CAST(usage_rows.cache_creation_cost_usd AS DOUBLE PRECISION),
    0
  ) AS cache_creation_cost_usd,
  COALESCE(
    CAST(settlement.billing_cache_read_cost_usd AS DOUBLE PRECISION),
    CAST(usage_rows.cache_read_cost_usd AS DOUBLE PRECISION),
    0
  ) AS cache_read_cost_usd,
  COALESCE(
    CAST(settlement.billing_total_cost_usd AS DOUBLE PRECISION),
    CAST(usage_rows.total_cost_usd AS DOUBLE PRECISION),
    0
  ) AS total_cost_usd,
  COALESCE(
    CAST(settlement.billing_actual_total_cost_usd AS DOUBLE PRECISION),
    CAST(usage_rows.actual_total_cost_usd AS DOUBLE PRECISION),
    0
  ) AS actual_total_cost_usd,
  COALESCE(
    CAST(settlement.output_price_per_1m AS DOUBLE PRECISION),
    CAST(usage_rows.output_price_per_1m AS DOUBLE PRECISION)
  ) AS output_price_per_1m,
  COALESCE(
    CAST(settlement.input_price_per_1m AS DOUBLE PRECISION),
    CAST(usage_rows.input_price_per_1m AS DOUBLE PRECISION)
  ) AS input_price_per_1m,
  COALESCE(
    CAST(settlement.cache_creation_price_per_1m AS DOUBLE PRECISION),
    CAST(usage_rows.cache_creation_price_per_1m AS DOUBLE PRECISION)
  ) AS cache_creation_price_per_1m,
  COALESCE(
    CAST(settlement.cache_read_price_per_1m AS DOUBLE PRECISION),
    CAST(usage_rows.cache_read_price_per_1m AS DOUBLE PRECISION)
  ) AS cache_read_price_per_1m,
  COALESCE(
    CAST(settlement.price_per_request AS DOUBLE PRECISION),
    CAST(usage_rows.price_per_request AS DOUBLE PRECISION)
  ) AS price_per_request,
  settlement.billing_pricing_source,
  settlement.billing_rule_id,
  settlement.billing_rule_version,
  COALESCE(usage_rows.upstream_is_stream, COALESCE(usage_rows.is_stream, FALSE)) AS upstream_is_stream
FROM public."usage" AS usage_rows
LEFT JOIN public.usage_settlement_snapshots AS settlement
  ON settlement.request_id = usage_rows.request_id;

COMMENT ON VIEW public.usage_billing_facts IS
  'Canonical billing read model. Token/cost fields prefer usage_settlement_snapshots.billing_* and fall back to deprecated usage mirrors for legacy rows.';

COMMENT ON COLUMN public.usage_billing_facts.upstream_is_stream IS
  'Resolved upstream stream mode from public.usage.upstream_is_stream, falling back to usage.is_stream for legacy rows.';

COMMENT ON COLUMN public.usage.input_tokens IS
  'DEPRECATED: billing dimension mirror. Use public.usage_settlement_snapshots.billing_input_tokens or public.usage_billing_facts.input_tokens.';
COMMENT ON COLUMN public.usage.output_tokens IS
  'DEPRECATED: billing dimension mirror. Use public.usage_settlement_snapshots.billing_output_tokens or public.usage_billing_facts.output_tokens.';
COMMENT ON COLUMN public.usage.total_tokens IS
  'DEPRECATED: billing dimension mirror. Use public.usage_billing_facts.total_tokens.';
COMMENT ON COLUMN public.usage.cache_creation_input_tokens IS
  'DEPRECATED: billing dimension mirror. Use public.usage_settlement_snapshots.billing_cache_creation_tokens or public.usage_billing_facts.cache_creation_input_tokens.';
COMMENT ON COLUMN public.usage.cache_creation_input_tokens_5m IS
  'DEPRECATED: billing dimension mirror. Use public.usage_settlement_snapshots.billing_cache_creation_5m_tokens or public.usage_billing_facts.cache_creation_input_tokens_5m.';
COMMENT ON COLUMN public.usage.cache_creation_input_tokens_1h IS
  'DEPRECATED: billing dimension mirror. Use public.usage_settlement_snapshots.billing_cache_creation_1h_tokens or public.usage_billing_facts.cache_creation_input_tokens_1h.';
COMMENT ON COLUMN public.usage.cache_read_input_tokens IS
  'DEPRECATED: billing dimension mirror. Use public.usage_settlement_snapshots.billing_cache_read_tokens or public.usage_billing_facts.cache_read_input_tokens.';
COMMENT ON COLUMN public.usage.cache_creation_cost_usd IS
  'DEPRECATED: billing cost mirror. Use public.usage_settlement_snapshots.billing_cache_creation_cost_usd or public.usage_billing_facts.cache_creation_cost_usd.';
COMMENT ON COLUMN public.usage.cache_read_cost_usd IS
  'DEPRECATED: billing cost mirror. Use public.usage_settlement_snapshots.billing_cache_read_cost_usd or public.usage_billing_facts.cache_read_cost_usd.';
COMMENT ON COLUMN public.usage.total_cost_usd IS
  'DEPRECATED: billing cost mirror. Use public.usage_settlement_snapshots.billing_total_cost_usd or public.usage_billing_facts.total_cost_usd.';
COMMENT ON COLUMN public.usage.actual_total_cost_usd IS
  'DEPRECATED: billing cost mirror. Use public.usage_settlement_snapshots.billing_actual_total_cost_usd or public.usage_billing_facts.actual_total_cost_usd.';

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
COMMENT ON COLUMN public.usage.billing_status IS
  'DEPRECATED: authoritative owner moved to public.usage_settlement_snapshots.billing_status. Compatibility/index mirror only; do not write new values.';
COMMENT ON COLUMN public.usage.finalized_at IS
  'DEPRECATED: authoritative owner moved to public.usage_settlement_snapshots.finalized_at. Compatibility/index mirror only; do not write new values.';
COMMENT ON COLUMN public.usage.request_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.request_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.provider_request_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.provider_request_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.response_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.response_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.client_response_headers IS
  'DEPRECATED: HTTP audit owner moved to public.usage_http_audits.client_response_headers. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.request_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.request_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.provider_request_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.provider_request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.provider_request_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.provider_request_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.response_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.response_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.response_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.response_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.client_response_body IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.client_response_body_ref. Legacy compatibility only; do not write new values.';
COMMENT ON COLUMN public.usage.client_response_body_compressed IS
  'DEPRECATED: HTTP body owner moved to public.usage_body_blobs plus public.usage_http_audits.client_response_body_ref. Legacy compatibility only; do not write new values.';
