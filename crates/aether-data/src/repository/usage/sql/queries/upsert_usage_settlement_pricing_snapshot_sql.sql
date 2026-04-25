INSERT INTO usage_settlement_snapshots (
  request_id,
  billing_status,
  billing_snapshot_schema_version,
  billing_snapshot_status,
  settlement_snapshot_schema_version,
  settlement_snapshot,
  billing_dimensions,
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
  billing_pricing_source,
  billing_rule_id,
  billing_rule_version,
  rate_multiplier,
  is_free_tier,
  input_price_per_1m,
  output_price_per_1m,
  cache_creation_price_per_1m,
  cache_read_price_per_1m,
  price_per_request
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  $14,
  $15,
  $16,
  $17,
  $18,
  $19,
  $20,
  $21,
  $22,
  $23,
  $24,
  $25,
  $26,
  $27,
  $28,
  $29
)
ON CONFLICT (request_id)
DO UPDATE SET
  billing_snapshot_schema_version = COALESCE(
    EXCLUDED.billing_snapshot_schema_version,
    usage_settlement_snapshots.billing_snapshot_schema_version
  ),
  billing_snapshot_status = COALESCE(
    EXCLUDED.billing_snapshot_status,
    usage_settlement_snapshots.billing_snapshot_status
  ),
  settlement_snapshot_schema_version = COALESCE(
    EXCLUDED.settlement_snapshot_schema_version,
    usage_settlement_snapshots.settlement_snapshot_schema_version
  ),
  settlement_snapshot = COALESCE(
    EXCLUDED.settlement_snapshot,
    usage_settlement_snapshots.settlement_snapshot
  ),
  billing_dimensions = COALESCE(
    EXCLUDED.billing_dimensions,
    usage_settlement_snapshots.billing_dimensions
  ),
  billing_input_tokens = COALESCE(
    EXCLUDED.billing_input_tokens,
    usage_settlement_snapshots.billing_input_tokens
  ),
  billing_effective_input_tokens = COALESCE(
    EXCLUDED.billing_effective_input_tokens,
    usage_settlement_snapshots.billing_effective_input_tokens
  ),
  billing_output_tokens = COALESCE(
    EXCLUDED.billing_output_tokens,
    usage_settlement_snapshots.billing_output_tokens
  ),
  billing_cache_creation_tokens = COALESCE(
    EXCLUDED.billing_cache_creation_tokens,
    usage_settlement_snapshots.billing_cache_creation_tokens
  ),
  billing_cache_creation_5m_tokens = COALESCE(
    EXCLUDED.billing_cache_creation_5m_tokens,
    usage_settlement_snapshots.billing_cache_creation_5m_tokens
  ),
  billing_cache_creation_1h_tokens = COALESCE(
    EXCLUDED.billing_cache_creation_1h_tokens,
    usage_settlement_snapshots.billing_cache_creation_1h_tokens
  ),
  billing_cache_read_tokens = COALESCE(
    EXCLUDED.billing_cache_read_tokens,
    usage_settlement_snapshots.billing_cache_read_tokens
  ),
  billing_total_input_context = COALESCE(
    EXCLUDED.billing_total_input_context,
    usage_settlement_snapshots.billing_total_input_context
  ),
  billing_cache_creation_cost_usd = COALESCE(
    EXCLUDED.billing_cache_creation_cost_usd,
    usage_settlement_snapshots.billing_cache_creation_cost_usd
  ),
  billing_cache_read_cost_usd = COALESCE(
    EXCLUDED.billing_cache_read_cost_usd,
    usage_settlement_snapshots.billing_cache_read_cost_usd
  ),
  billing_total_cost_usd = COALESCE(
    EXCLUDED.billing_total_cost_usd,
    usage_settlement_snapshots.billing_total_cost_usd
  ),
  billing_actual_total_cost_usd = COALESCE(
    EXCLUDED.billing_actual_total_cost_usd,
    usage_settlement_snapshots.billing_actual_total_cost_usd
  ),
  billing_pricing_source = COALESCE(
    EXCLUDED.billing_pricing_source,
    usage_settlement_snapshots.billing_pricing_source
  ),
  billing_rule_id = COALESCE(
    EXCLUDED.billing_rule_id,
    usage_settlement_snapshots.billing_rule_id
  ),
  billing_rule_version = COALESCE(
    EXCLUDED.billing_rule_version,
    usage_settlement_snapshots.billing_rule_version
  ),
  rate_multiplier = COALESCE(
    EXCLUDED.rate_multiplier,
    usage_settlement_snapshots.rate_multiplier
  ),
  is_free_tier = COALESCE(
    EXCLUDED.is_free_tier,
    usage_settlement_snapshots.is_free_tier
  ),
  input_price_per_1m = COALESCE(
    EXCLUDED.input_price_per_1m,
    usage_settlement_snapshots.input_price_per_1m
  ),
  output_price_per_1m = COALESCE(
    EXCLUDED.output_price_per_1m,
    usage_settlement_snapshots.output_price_per_1m
  ),
  cache_creation_price_per_1m = COALESCE(
    EXCLUDED.cache_creation_price_per_1m,
    usage_settlement_snapshots.cache_creation_price_per_1m
  ),
  cache_read_price_per_1m = COALESCE(
    EXCLUDED.cache_read_price_per_1m,
    usage_settlement_snapshots.cache_read_price_per_1m
  ),
  price_per_request = COALESCE(
    EXCLUDED.price_per_request,
    usage_settlement_snapshots.price_per_request
  ),
  updated_at = NOW()
