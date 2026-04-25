ALTER TABLE IF EXISTS public.usage_settlement_snapshots
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
        WHEN GREATEST(COALESCE(usage_rows.cache_read_input_tokens, 0), 0) <= 0
        THEN GREATEST(COALESCE(usage_rows.input_tokens, 0), 0)
        WHEN split_part(lower(COALESCE(COALESCE(usage_rows.endpoint_api_format, usage_rows.api_format), '')), ':', 1)
             IN ('openai', 'gemini', 'google')
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
        WHEN settlement.billing_input_tokens IS NOT NULL
          OR settlement.billing_output_tokens IS NOT NULL
          OR settlement.billing_cache_creation_tokens IS NOT NULL
          OR settlement.billing_cache_creation_5m_tokens IS NOT NULL
          OR settlement.billing_cache_creation_1h_tokens IS NOT NULL
          OR settlement.billing_cache_read_tokens IS NOT NULL
        THEN COALESCE(settlement.billing_input_tokens, 0)
          + COALESCE(settlement.billing_output_tokens, 0)
          + COALESCE(
              settlement.billing_cache_creation_tokens,
              COALESCE(settlement.billing_cache_creation_5m_tokens, 0)
                + COALESCE(settlement.billing_cache_creation_1h_tokens, 0),
              0
            )
          + COALESCE(settlement.billing_cache_read_tokens, 0)
      END,
      usage_rows.total_tokens,
      0
    ),
    0
  )::bigint AS total_tokens,
  GREATEST(
    COALESCE(
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
             IN ('openai', 'gemini', 'google')
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
  settlement.billing_rule_version
FROM public."usage" AS usage_rows
LEFT JOIN public.usage_settlement_snapshots AS settlement
  ON settlement.request_id = usage_rows.request_id;

COMMENT ON VIEW public.usage_billing_facts IS
  'Canonical billing read model. Token/cost fields prefer usage_settlement_snapshots.billing_* and fall back to deprecated usage mirrors for legacy rows.';

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
