-- Align Rust-managed migrations with legacy Alembic revision
-- c3d4e5f6a7b8 (usage_token_semantics_v2).

DO $$
BEGIN
  IF EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public'
      AND table_name = 'usage'
      AND column_name = 'total_tokens'
  ) AND NOT EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public'
      AND table_name = 'usage'
      AND column_name = 'input_output_total_tokens'
  ) THEN
    ALTER TABLE "usage" RENAME COLUMN total_tokens TO input_output_total_tokens;
  END IF;
END $$;

ALTER TABLE "usage"
  ADD COLUMN IF NOT EXISTS input_output_total_tokens INTEGER DEFAULT 0,
  ADD COLUMN IF NOT EXISTS cache_creation_input_tokens_5m INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS cache_creation_input_tokens_1h INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS input_context_tokens INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS total_tokens INTEGER NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS cache_creation_cost_usd_5m NUMERIC(20, 8) NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS cache_creation_cost_usd_1h NUMERIC(20, 8) NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS actual_cache_creation_cost_usd_5m NUMERIC(20, 8) NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS actual_cache_creation_cost_usd_1h NUMERIC(20, 8) NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS actual_cache_cost_usd NUMERIC(20, 8) NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS cache_creation_price_per_1m_5m NUMERIC(20, 8),
  ADD COLUMN IF NOT EXISTS cache_creation_price_per_1m_1h NUMERIC(20, 8);

UPDATE "usage"
SET
  input_output_total_tokens = src.new_iot,
  input_context_tokens = src.new_ict,
  total_tokens = src.new_total,
  cache_creation_cost_usd_5m = src.new_cc5m,
  cache_creation_cost_usd_1h = src.new_cc1h,
  actual_cache_creation_cost_usd_5m = src.new_acc5m,
  actual_cache_creation_cost_usd_1h = src.new_acc1h,
  actual_cache_cost_usd = src.new_accu,
  cache_creation_price_per_1m_5m = src.new_cp5m,
  cache_creation_price_per_1m_1h = src.new_cp1h,
  cache_cost_usd = src.new_ccu
FROM (
  SELECT
    id,
    COALESCE(
      input_output_total_tokens,
      COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ) AS new_iot,
    COALESCE(input_tokens, 0) + COALESCE(cache_read_input_tokens, 0) AS new_ict,
    COALESCE(
      input_output_total_tokens,
      COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
    ) + COALESCE(cache_creation_input_tokens, 0) + COALESCE(cache_read_input_tokens, 0) AS new_total,
    CASE
      WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
        AND COALESCE(cache_creation_input_tokens_1h, 0) = 0
      THEN COALESCE(cache_creation_cost_usd, 0)
      WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
        AND COALESCE(cache_creation_input_tokens, 0) > 0
      THEN COALESCE(cache_creation_cost_usd, 0)
        * (COALESCE(cache_creation_input_tokens_5m, 0) * 1.0
          / GREATEST(COALESCE(cache_creation_input_tokens, 0), 1))
      ELSE 0
    END AS new_cc5m,
    CASE
      WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
        AND COALESCE(cache_creation_input_tokens_5m, 0) = 0
      THEN COALESCE(cache_creation_cost_usd, 0)
      WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
        AND COALESCE(cache_creation_input_tokens, 0) > 0
      THEN COALESCE(cache_creation_cost_usd, 0)
        * (COALESCE(cache_creation_input_tokens_1h, 0) * 1.0
          / GREATEST(COALESCE(cache_creation_input_tokens, 0), 1))
      ELSE 0
    END AS new_cc1h,
    CASE
      WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
        AND COALESCE(cache_creation_input_tokens_1h, 0) = 0
      THEN COALESCE(actual_cache_creation_cost_usd, 0)
      WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
        AND COALESCE(cache_creation_input_tokens, 0) > 0
      THEN COALESCE(actual_cache_creation_cost_usd, 0)
        * (COALESCE(cache_creation_input_tokens_5m, 0) * 1.0
          / GREATEST(COALESCE(cache_creation_input_tokens, 0), 1))
      ELSE 0
    END AS new_acc5m,
    CASE
      WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
        AND COALESCE(cache_creation_input_tokens_5m, 0) = 0
      THEN COALESCE(actual_cache_creation_cost_usd, 0)
      WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
        AND COALESCE(cache_creation_input_tokens, 0) > 0
      THEN COALESCE(actual_cache_creation_cost_usd, 0)
        * (COALESCE(cache_creation_input_tokens_1h, 0) * 1.0
          / GREATEST(COALESCE(cache_creation_input_tokens, 0), 1))
      ELSE 0
    END AS new_acc1h,
    COALESCE(actual_cache_creation_cost_usd, 0)
      + COALESCE(actual_cache_read_cost_usd, 0) AS new_accu,
    CASE
      WHEN COALESCE(cache_creation_input_tokens_5m, 0) > 0
        AND COALESCE(cache_creation_input_tokens_1h, 0) = 0
      THEN cache_creation_price_per_1m
      ELSE NULL
    END AS new_cp5m,
    CASE
      WHEN COALESCE(cache_creation_input_tokens_1h, 0) > 0
        AND COALESCE(cache_creation_input_tokens_5m, 0) = 0
      THEN cache_creation_price_per_1m
      ELSE NULL
    END AS new_cp1h,
    COALESCE(cache_creation_cost_usd, 0)
      + COALESCE(cache_read_cost_usd, 0) AS new_ccu
  FROM "usage"
) AS src
WHERE "usage".id = src.id;
