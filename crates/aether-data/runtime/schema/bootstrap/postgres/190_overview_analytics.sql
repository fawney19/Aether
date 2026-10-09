ALTER TABLE public.usage ADD COLUMN IF NOT EXISTS failure_origin text;
ALTER TABLE public.usage ADD COLUMN IF NOT EXISTS failure_stage text;
ALTER TABLE public.usage ADD COLUMN IF NOT EXISTS failure_reason text;
ALTER TABLE public.usage ADD COLUMN IF NOT EXISTS failure_schema_version integer;
ALTER TABLE public.usage_settlement_snapshots
  ADD COLUMN IF NOT EXISTS quota_covered_amount_usd numeric(20,8),
  ADD COLUMN IF NOT EXISTS wallet_consumed_amount_usd numeric(20,8),
  ADD COLUMN IF NOT EXISTS wallet_debit_amount_usd numeric(20,8),
  ADD COLUMN IF NOT EXISTS wallet_recharge_debit_usd numeric(20,8),
  ADD COLUMN IF NOT EXISTS wallet_gift_debit_usd numeric(20,8),
  ADD COLUMN IF NOT EXISTS wallet_overdraft_usd numeric(20,8),
  ADD COLUMN IF NOT EXISTS allocation_schema_version integer,
  ADD COLUMN IF NOT EXISTS allocation_status text;

CREATE TABLE IF NOT EXISTS public.usage_attribution_snapshots (
  request_id text PRIMARY KEY,
  actor_user_id text,
  credential_owner_id text,
  attribution_kind text NOT NULL DEFAULT 'unknown',
  attribution_source text NOT NULL DEFAULT 'unknown',
  record_kind text NOT NULL DEFAULT 'request',
  parent_request_id text,
  schema_version integer NOT NULL DEFAULT 1,
  attribution_revision bigint NOT NULL DEFAULT 1,
  recorded_at timestamptz NOT NULL DEFAULT NOW()
);
CREATE INDEX IF NOT EXISTS ix_usage_attribution_actor_request
  ON public.usage_attribution_snapshots(actor_user_id, request_id);
-- This attribution table is new and empty; build both lookup indexes here.
CREATE INDEX IF NOT EXISTS ix_usage_attribution_owner_request
  ON public.usage_attribution_snapshots(credential_owner_id, request_id);

CREATE TABLE IF NOT EXISTS public.stats_bucket_state (
  projection_version text NOT NULL,
  granularity text NOT NULL,
  bucket_start timestamptz NOT NULL,
  source_revision bigint NOT NULL DEFAULT 0,
  built_revision bigint NOT NULL DEFAULT -1,
  coverage_status text NOT NULL DEFAULT 'unbuilt',
  built_at timestamptz,
  last_error text,
  last_failed_at timestamptz,
  PRIMARY KEY (projection_version, granularity, bucket_start)
);
CREATE INDEX IF NOT EXISTS ix_stats_bucket_state_dirty
  ON public.stats_bucket_state(bucket_start)
  WHERE source_revision > built_revision;

CREATE TABLE IF NOT EXISTS public.stats_overview_hourly (
  projection_version text NOT NULL,
  bucket_start timestamptz NOT NULL,
  dimensions jsonb NOT NULL,
  metrics jsonb NOT NULL,
  PRIMARY KEY(projection_version, bucket_start, dimensions)
);
CREATE TABLE IF NOT EXISTS public.stats_overview_daily (
  projection_version text NOT NULL,
  bucket_start timestamptz NOT NULL,
  dimensions jsonb NOT NULL,
  metrics jsonb NOT NULL,
  PRIMARY KEY(projection_version, bucket_start, dimensions)
);

-- These triggers cover old writers, delayed settlement, and maintenance in the fact transaction.
-- Only future fact mutations enqueue work; no historical rows are backfilled.
-- Each writer owns its transaction's keys, so unrelated requests never contend
-- on the current hour/day's stats_bucket_state row.
CREATE TABLE IF NOT EXISTS public.stats_overview_dirty_events (
  transaction_id bigint NOT NULL,
  projection_version text NOT NULL,
  granularity text NOT NULL,
  bucket_start timestamptz NOT NULL,
  unrecoverable boolean NOT NULL DEFAULT false,
  PRIMARY KEY (transaction_id, projection_version, granularity, bucket_start)
);
CREATE INDEX IF NOT EXISTS ix_stats_overview_dirty_events_bucket
  ON public.stats_overview_dirty_events(projection_version, granularity, bucket_start);

CREATE OR REPLACE FUNCTION public.overview_mark_usage_bucket() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE old_time timestamptz; new_time timestamptz; bucket record;
BEGIN
  IF TG_TABLE_NAME = 'usage' THEN
    IF TG_OP <> 'INSERT' THEN old_time := OLD.created_at; END IF;
    IF TG_OP <> 'DELETE' THEN new_time := NEW.created_at; END IF;
  ELSE
    IF TG_OP <> 'INSERT' THEN
      SELECT created_at INTO old_time FROM public.usage WHERE request_id = OLD.request_id;
    END IF;
    IF TG_OP <> 'DELETE' THEN
      SELECT created_at INTO new_time FROM public.usage WHERE request_id = NEW.request_id;
    END IF;
  END IF;
  FOR bucket IN
    SELECT DISTINCT g, date_trunc(g, t AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS starts
    FROM unnest(ARRAY[old_time, new_time]) t CROSS JOIN unnest(ARRAY['day','hour']) g
    WHERE t IS NOT NULL ORDER BY g, starts
  LOOP
    INSERT INTO public.stats_overview_dirty_events
      (transaction_id, projection_version, granularity, bucket_start, unrecoverable)
    VALUES (txid_current(), 'overview-v2', bucket.g, bucket.starts,
      TG_TABLE_NAME = 'usage' AND TG_OP = 'DELETE')
    ON CONFLICT (transaction_id, projection_version, granularity, bucket_start)
    DO UPDATE SET unrecoverable = stats_overview_dirty_events.unrecoverable OR EXCLUDED.unrecoverable;
  END LOOP;
  RETURN NULL;
END $$;

DROP TRIGGER IF EXISTS overview_usage_dirty ON public.usage;
CREATE TRIGGER overview_usage_dirty AFTER INSERT OR UPDATE OR DELETE ON public.usage
  FOR EACH ROW EXECUTE FUNCTION public.overview_mark_usage_bucket();
DROP TRIGGER IF EXISTS overview_settlement_dirty ON public.usage_settlement_snapshots;
CREATE TRIGGER overview_settlement_dirty AFTER INSERT OR UPDATE OR DELETE ON public.usage_settlement_snapshots
  FOR EACH ROW EXECUTE FUNCTION public.overview_mark_usage_bucket();
DROP TRIGGER IF EXISTS overview_attribution_dirty ON public.usage_attribution_snapshots;
CREATE TRIGGER overview_attribution_dirty AFTER INSERT OR UPDATE OR DELETE ON public.usage_attribution_snapshots
  FOR EACH ROW EXECUTE FUNCTION public.overview_mark_usage_bucket();

CREATE OR REPLACE FUNCTION public.overview_capture_usage_identity() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE actor text; owner text; kind text; source text; standalone boolean;
BEGIN
  SELECT id INTO owner FROM public.users WHERE id=NEW.user_id AND NOT is_deleted;
  SELECT COALESCE(k.is_standalone,
    CASE WHEN jsonb_typeof(NEW.request_metadata::jsonb #> '{analytics_attribution,is_standalone}')='boolean'
      THEN (NEW.request_metadata #>> '{analytics_attribution,is_standalone}')::boolean END,
    CASE WHEN jsonb_typeof(NEW.request_metadata::jsonb->'api_key_is_standalone')='boolean'
      THEN (NEW.request_metadata->>'api_key_is_standalone')::boolean END,
    CASE WHEN a.attribution_source='user_account' THEN false
      WHEN a.attribution_source='standalone_key' THEN true END,
    CASE WHEN NEW.api_key_id IS NULL THEN false END)
  INTO standalone FROM (SELECT 1) seed
  LEFT JOIN public.api_keys k ON k.id=NEW.api_key_id
  LEFT JOIN public.usage_attribution_snapshots a ON a.request_id=NEW.request_id;
  kind := CASE WHEN owner IS NULL THEN 'unknown' WHEN standalone THEN 'standalone'
    WHEN NOT standalone THEN 'employee' ELSE 'unknown' END;
  actor := CASE WHEN kind='employee' THEN owner END;
  source := CASE kind WHEN 'employee' THEN 'user_account' WHEN 'standalone' THEN 'standalone_key' ELSE 'unknown' END;
  INSERT INTO public.usage_attribution_snapshots(request_id, actor_user_id, credential_owner_id,
    attribution_kind, attribution_source, record_kind, parent_request_id, attribution_revision)
  VALUES (NEW.request_id, actor, owner, kind, source,
    COALESCE(NEW.request_metadata #>> '{analytics_attribution,record_kind}', 'request'),
    NEW.request_metadata #>> '{analytics_attribution,parent_request_id}', 2)
  ON CONFLICT (request_id) DO UPDATE SET actor_user_id = EXCLUDED.actor_user_id,
    credential_owner_id = EXCLUDED.credential_owner_id, attribution_kind = EXCLUDED.attribution_kind,
    attribution_source = EXCLUDED.attribution_source,
    record_kind = COALESCE(NEW.request_metadata #>> '{analytics_attribution,record_kind}', usage_attribution_snapshots.record_kind),
    parent_request_id = COALESCE(EXCLUDED.parent_request_id, usage_attribution_snapshots.parent_request_id),
    attribution_revision = EXCLUDED.attribution_revision, recorded_at = NOW()
  WHERE usage_attribution_snapshots.attribution_revision <= EXCLUDED.attribution_revision;
  RETURN NULL;
END $$;
DROP TRIGGER IF EXISTS overview_usage_identity ON public.usage;
CREATE TRIGGER overview_usage_identity AFTER INSERT OR UPDATE OF user_id, api_key_id, request_metadata ON public.usage
  FOR EACH ROW EXECUTE FUNCTION public.overview_capture_usage_identity();

CREATE OR REPLACE FUNCTION public.overview_capture_failure() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.status = 'completed' THEN
    NEW.failure_origin := NULL; NEW.failure_stage := NULL; NEW.failure_reason := NULL;
    NEW.failure_schema_version := NULL; RETURN NEW;
  END IF;
  IF NEW.request_metadata #>> '{analytics_failure,origin}' IN ('client','gateway','upstream','transport','unknown') THEN
    NEW.failure_origin := NEW.request_metadata #>> '{analytics_failure,origin}';
    NEW.failure_stage := NEW.request_metadata #>> '{analytics_failure,stage}';
    NEW.failure_reason := NEW.request_metadata #>> '{analytics_failure,reason}';
    NEW.failure_schema_version := 1;
  END IF;
  RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS overview_usage_failure ON public.usage;
CREATE TRIGGER overview_usage_failure BEFORE INSERT OR UPDATE OF request_metadata, status ON public.usage
  FOR EACH ROW EXECUTE FUNCTION public.overview_capture_failure();

CREATE OR REPLACE FUNCTION public.overview_anonymize_user() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP = 'DELETE' OR NEW.is_deleted THEN
    UPDATE public.usage_attribution_snapshots SET actor_user_id = NULL, credential_owner_id = NULL,
      attribution_kind = 'unknown', attribution_source = 'unknown', attribution_revision = attribution_revision + 10
    WHERE actor_user_id = OLD.id OR credential_owner_id = OLD.id;
    UPDATE public.usage SET request_metadata = (request_metadata::jsonb #- '{analytics_attribution,actor_user_id}')::json
    WHERE request_metadata #>> '{analytics_attribution,actor_user_id}' = OLD.id;
    DELETE FROM public.stats_overview_hourly WHERE dimensions->>'actor_user_id'=OLD.id OR dimensions->>'credential_owner_id'=OLD.id;
    DELETE FROM public.stats_overview_daily WHERE dimensions->>'actor_user_id'=OLD.id OR dimensions->>'credential_owner_id'=OLD.id;
  END IF;
  RETURN NULL;
END $$;
DROP TRIGGER IF EXISTS overview_user_anonymize ON public.users;
CREATE TRIGGER overview_user_anonymize AFTER DELETE OR UPDATE OF is_deleted ON public.users
  FOR EACH ROW EXECUTE FUNCTION public.overview_anonymize_user();

CREATE OR REPLACE FUNCTION public.overview_delete_attribution() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  DELETE FROM public.usage_attribution_snapshots WHERE request_id = OLD.request_id;
  RETURN OLD;
END $$;
DROP TRIGGER IF EXISTS overview_usage_delete_attribution ON public.usage;
CREATE TRIGGER overview_usage_delete_attribution BEFORE DELETE ON public.usage
  FOR EACH ROW EXECUTE FUNCTION public.overview_delete_attribution();

-- Customer charges use the immutable request-time factor snapshot. Provider
-- procurement cost remains in actual_total_cost_usd for legacy reporting.
CREATE OR REPLACE FUNCTION public.usage_customer_billable_amount(
  metadata jsonb, base_cost numeric, legacy_cost numeric
) RETURNS numeric LANGUAGE plpgsql IMMUTABLE PARALLEL UNSAFE AS $$
DECLARE factor jsonb; multiplier numeric; amount numeric;
  factor_name text; factor_value jsonb; factor_number double precision;
  expected_multiplier double precision := 1.0; factor_count integer := 0;
  has_zero boolean := false;
BEGIN
  IF metadata ? 'billing_multiplier_snapshot' THEN
    IF jsonb_typeof(metadata->'billing_multiplier_snapshot') <> 'object'
      OR metadata #> '{billing_multiplier_snapshot,version}' IS DISTINCT FROM '1'::jsonb
      OR jsonb_typeof(metadata #> '{billing_multiplier_snapshot,factors}') IS DISTINCT FROM 'object'
    THEN RETURN NULL; END IF;
    factor := metadata #> '{billing_multiplier_snapshot,multiplier}';
    FOR factor_name, factor_value IN
      SELECT key, value FROM jsonb_each(metadata #> '{billing_multiplier_snapshot,factors}') ORDER BY key COLLATE "C"
    LOOP
      factor_count := factor_count + 1;
      IF factor_count > 16 OR factor_name = '' OR length(factor_name) > 64
        OR factor_name !~ '^[A-Za-z0-9_]+$'
        OR jsonb_typeof(factor_value) IS DISTINCT FROM 'number'
      THEN RETURN NULL; END IF;
      factor_number := factor_value::text::double precision;
      IF factor_number < 0 OR factor_number > 1.7976931348623157e308::double precision
      THEN RETURN NULL; END IF;
      has_zero := has_zero OR factor_number = 0;
    END LOOP;
    -- Rust short-circuits zero before multiplying any of the other factors.
    IF has_zero THEN expected_multiplier := 0;
    ELSE
      FOR factor_name, factor_value IN
        SELECT key, value FROM jsonb_each(metadata #> '{billing_multiplier_snapshot,factors}') ORDER BY key COLLATE "C"
      LOOP
        factor_number := factor_value::text::double precision;
        BEGIN
          expected_multiplier := expected_multiplier * factor_number;
        EXCEPTION WHEN numeric_value_out_of_range THEN
          -- PostgreSQL raises on float underflow; Rust rounds that product to 0.
          IF expected_multiplier::numeric * factor_number::numeric > 1.7976931348623157e308::numeric
          THEN RETURN NULL; END IF;
          expected_multiplier := 0;
        END;
      END LOOP;
    END IF;
  ELSIF metadata ? 'routing_group_billing_multiplier' THEN
    factor := metadata->'routing_group_billing_multiplier';
    expected_multiplier := NULL;
  ELSE
    RETURN CASE WHEN legacy_cost NOT IN ('NaN'::numeric,'Infinity'::numeric,'-Infinity'::numeric)
      THEN round(legacy_cost,8) END;
  END IF;
  IF jsonb_typeof(factor) IS DISTINCT FROM 'number' THEN RETURN NULL; END IF;
  multiplier := factor::text::numeric;
  factor_number := factor::text::double precision;
  IF factor_number < 0
    OR factor_number > 1.7976931348623157e308::double precision
    OR (expected_multiplier IS NOT NULL AND factor_number <> expected_multiplier)
    OR multiplier < 0 OR multiplier > 1.7976931348623157e308::numeric
    OR base_cost IS NULL OR base_cost < 0
    OR base_cost IN ('NaN'::numeric,'Infinity'::numeric,'-Infinity'::numeric)
  THEN RETURN NULL; END IF;
  amount := base_cost * multiplier;
  IF amount > 1.7976931348623157e308::numeric THEN RETURN NULL; END IF;
  RETURN round(amount,8);
EXCEPTION WHEN numeric_value_out_of_range OR invalid_text_representation THEN
  -- Corrupt captured pricing must not abort an entire analytics query.
  RETURN NULL;
END $$;

CREATE OR REPLACE VIEW public.usage_analytics_facts_v1 AS
SELECT u.request_id, COALESCE(u.id, u.request_id) AS id, u.created_at,
  CASE WHEN identity.owner_id IS NOT NULL AND identity.is_standalone=false THEN identity.owner_id END AS actor_user_id,
  identity.owner_id AS credential_owner_id,
  CASE WHEN identity.owner_id IS NULL THEN 'unknown' WHEN identity.is_standalone THEN 'standalone'
    WHEN NOT identity.is_standalone THEN 'employee' ELSE 'unknown' END AS attribution_kind,
  CASE WHEN identity.owner_id IS NULL THEN 'unknown' WHEN identity.is_standalone THEN 'standalone_key'
    WHEN NOT identity.is_standalone THEN 'user_account' ELSE 'unknown' END AS attribution_source,
  COALESCE(a.record_kind, 'request') AS record_kind, a.parent_request_id,
  u.api_key_id, u.model, u.target_model, u.provider_id, u.provider_name,
  u.api_format, u.endpoint_kind, u.request_type, u.is_stream, u.has_format_conversion,
  u.status, u.status_code, u.error_category, u.failure_origin, u.failure_stage, u.failure_reason,
  u.failure_schema_version, u.response_time_ms, u.first_byte_time_ms,
  COALESCE(s.billing_status, u.billing_status) AS settlement_status,
  COALESCE(metadata.value->'usage_available', 'true'::jsonb) <> 'false'::jsonb AS usage_available,
  COALESCE(metadata.value->'usage_pricing_available', 'true'::jsonb) <> 'false'::jsonb
    AND (s.billing_total_cost_usd IS NOT NULL OR COALESCE(s.billing_status, u.billing_status) = 'settled') AS pricing_available,
  CASE WHEN COALESCE(metadata.value->'usage_available', 'true'::jsonb) <> 'false'::jsonb
    THEN b.input_tokens END AS input_tokens,
  CASE WHEN COALESCE(metadata.value->'usage_available', 'true'::jsonb) <> 'false'::jsonb
    THEN b.output_tokens END AS output_tokens,
  CASE WHEN COALESCE(metadata.value->'usage_available', 'true'::jsonb) <> 'false'::jsonb
    THEN b.total_tokens END AS total_tokens,
  CASE WHEN COALESCE(metadata.value->'usage_available', 'true'::jsonb) <> 'false'::jsonb
    THEN b.cache_read_input_tokens END AS cache_read_input_tokens,
  CASE WHEN COALESCE(metadata.value->'usage_available', 'true'::jsonb) <> 'false'::jsonb
    THEN b.cache_creation_input_tokens END AS cache_creation_input_tokens,
  CASE WHEN COALESCE(metadata.value->'usage_pricing_available', 'true'::jsonb) <> 'false'::jsonb
    AND (s.billing_total_cost_usd IS NOT NULL OR COALESCE(s.billing_status, u.billing_status) = 'settled')
    THEN round(COALESCE(s.billing_total_cost_usd::numeric, u.total_cost_usd::numeric), 8) END AS rated_amount,
  CASE WHEN COALESCE(metadata.value->'usage_pricing_available', 'true'::jsonb) <> 'false'::jsonb
    AND (s.billing_actual_total_cost_usd IS NOT NULL OR COALESCE(s.billing_status, u.billing_status) = 'settled')
    THEN public.usage_customer_billable_amount(metadata.value,
      COALESCE(s.billing_total_cost_usd::numeric, u.total_cost_usd::numeric),
      COALESCE(s.billing_actual_total_cost_usd::numeric, u.actual_total_cost_usd::numeric)) END AS billable_amount,
  s.quota_covered_amount_usd AS quota_covered_amount,
  s.wallet_consumed_amount_usd AS wallet_consumed_amount,
  s.wallet_debit_amount_usd AS wallet_debit_amount,
  s.wallet_recharge_debit_usd AS wallet_recharge_debit_amount,
  s.wallet_gift_debit_usd AS wallet_gift_debit_amount,
  s.wallet_overdraft_usd AS wallet_overdraft_amount,
  s.allocation_status, s.finalized_at AS settled_at,
  CASE WHEN s.billing_total_cost_usd IS NOT NULL THEN 'settlement_snapshot' ELSE 'legacy_float' END AS amount_source,
  b.upstream_is_stream,
  CASE WHEN metadata.value #>> '{analytics_measurement,source}' IN ('reported','estimated','mixed')
    THEN metadata.value #>> '{analytics_measurement,source}' ELSE 'unknown' END AS token_source,
  CASE WHEN COALESCE(metadata.value->'usage_available','true'::jsonb) <> 'false'::jsonb
    AND COALESCE(metadata.value->'usage_pricing_available','true'::jsonb) <> 'false'::jsonb
    AND s.input_price_per_1m IS NOT NULL AND s.billing_cache_read_cost_usd IS NOT NULL
    THEN round(s.input_price_per_1m::numeric * b.cache_read_input_tokens::numeric / 1000000,8) END AS cache_estimated_full_cost_amount,
  CASE WHEN COALESCE(metadata.value->'usage_available','true'::jsonb) <> 'false'::jsonb
    AND COALESCE(metadata.value->'usage_pricing_available','true'::jsonb) <> 'false'::jsonb
    AND s.input_price_per_1m IS NOT NULL AND s.billing_cache_read_cost_usd IS NOT NULL
    THEN round(s.billing_cache_read_cost_usd::numeric,8) END AS cache_read_cost_amount,
  CASE WHEN COALESCE(metadata.value->'usage_available','true'::jsonb) <> 'false'::jsonb
    AND COALESCE(metadata.value->'usage_pricing_available','true'::jsonb) <> 'false'::jsonb
    AND s.input_price_per_1m IS NOT NULL AND s.billing_cache_creation_cost_usd IS NOT NULL
    THEN round(s.billing_cache_creation_cost_usd::numeric,8) END AS cache_creation_cost_amount
FROM public.usage u
-- OFFSET 0 keeps this projection from being flattened: large metadata is
-- detoasted and parsed once per request, rather than once per metric expression.
CROSS JOIN LATERAL (SELECT u.request_metadata::jsonb AS value OFFSET 0) metadata
LEFT JOIN public.usage_settlement_snapshots s USING (request_id)
LEFT JOIN public.usage_attribution_snapshots a USING (request_id)
JOIN public.usage_billing_facts b USING (request_id)
LEFT JOIN public.api_keys k ON k.id=u.api_key_id
CROSS JOIN LATERAL (
  SELECT CASE WHEN a.request_id IS NOT NULL THEN a.credential_owner_id
    WHEN EXISTS (SELECT 1 FROM public.users WHERE id=u.user_id AND NOT is_deleted) THEN u.user_id END AS owner_id,
    COALESCE(k.is_standalone,
      CASE WHEN jsonb_typeof(metadata.value #> '{analytics_attribution,is_standalone}')='boolean'
        THEN (metadata.value #>> '{analytics_attribution,is_standalone}')::boolean END,
      CASE WHEN jsonb_typeof(metadata.value->'api_key_is_standalone')='boolean'
        THEN (metadata.value->>'api_key_is_standalone')::boolean END,
      CASE WHEN a.attribution_source='user_account' THEN false
        WHEN a.attribution_source='standalone_key' THEN true END,
      CASE WHEN u.api_key_id IS NULL THEN false END) AS is_standalone
) identity;
