-- no-transaction
-- Dashboard aggregates read canonical billing fields for every usage row in
-- the selected window. Keeping those narrow fields in the request-id index
-- lets PostgreSQL satisfy the view join with an index-only scan instead of
-- performing one heap lookup in the wide settlement snapshot table per row.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_usage_settlement_dashboard_cover
ON usage_settlement_snapshots (request_id)
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
