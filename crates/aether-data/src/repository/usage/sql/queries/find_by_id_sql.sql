SELECT
  "usage".id,
  "usage".request_id,
  "usage".user_id,
  "usage".api_key_id,
  "usage".username,
  "usage".api_key_name,
  "usage".provider_name,
  "usage".model,
  "usage".target_model,
  "usage".provider_id,
  "usage".provider_endpoint_id,
  "usage".provider_api_key_id,
  "usage".request_type,
  "usage".api_format,
  "usage".api_family,
  "usage".endpoint_kind,
  "usage".endpoint_api_format,
  "usage".provider_api_family,
  "usage".provider_endpoint_kind,
  COALESCE("usage".has_format_conversion, FALSE) AS has_format_conversion,
  COALESCE("usage".is_stream, FALSE) AS is_stream,
  CAST("usage".input_tokens AS INTEGER) AS input_tokens,
  CAST(COALESCE(usage_settlement_snapshots.billing_output_tokens, "usage".output_tokens) AS INTEGER) AS output_tokens,
  CAST(COALESCE(
    CASE
      WHEN usage_settlement_snapshots.billing_input_tokens IS NOT NULL
        OR usage_settlement_snapshots.billing_output_tokens IS NOT NULL
        OR usage_settlement_snapshots.billing_cache_creation_tokens IS NOT NULL
        OR usage_settlement_snapshots.billing_cache_creation_5m_tokens IS NOT NULL
        OR usage_settlement_snapshots.billing_cache_creation_1h_tokens IS NOT NULL
        OR usage_settlement_snapshots.billing_cache_read_tokens IS NOT NULL
      THEN COALESCE(usage_settlement_snapshots.billing_input_tokens, 0)
        + COALESCE(usage_settlement_snapshots.billing_output_tokens, 0)
        + COALESCE(
            usage_settlement_snapshots.billing_cache_creation_tokens,
            COALESCE(usage_settlement_snapshots.billing_cache_creation_5m_tokens, 0)
              + COALESCE(usage_settlement_snapshots.billing_cache_creation_1h_tokens, 0),
            0
          )
        + COALESCE(usage_settlement_snapshots.billing_cache_read_tokens, 0)
    END,
    "usage".total_tokens
  ) AS INTEGER) AS total_tokens,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_cache_creation_tokens,
    "usage".cache_creation_input_tokens,
    0
  ) AS INTEGER) AS cache_creation_input_tokens,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_cache_creation_5m_tokens,
    "usage".cache_creation_input_tokens_5m,
    0
  ) AS INTEGER) AS cache_creation_ephemeral_5m_input_tokens,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_cache_creation_1h_tokens,
    "usage".cache_creation_input_tokens_1h,
    0
  ) AS INTEGER) AS cache_creation_ephemeral_1h_input_tokens,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_cache_read_tokens,
    "usage".cache_read_input_tokens,
    0
  ) AS INTEGER) AS cache_read_input_tokens,
  COALESCE(
    CAST(usage_settlement_snapshots.billing_cache_creation_cost_usd AS DOUBLE PRECISION),
    CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION),
    0
  ) AS cache_creation_cost_usd,
  COALESCE(
    CAST(usage_settlement_snapshots.billing_cache_read_cost_usd AS DOUBLE PRECISION),
    CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION),
    0
  ) AS cache_read_cost_usd,
  COALESCE(
    CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
    CAST("usage".output_price_per_1m AS DOUBLE PRECISION)
  ) AS output_price_per_1m,
  COALESCE(
    CAST(usage_settlement_snapshots.billing_total_cost_usd AS DOUBLE PRECISION),
    CAST("usage".total_cost_usd AS DOUBLE PRECISION),
    0
  ) AS total_cost_usd,
  COALESCE(
    CAST(usage_settlement_snapshots.billing_actual_total_cost_usd AS DOUBLE PRECISION),
    CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION),
    0
  ) AS actual_total_cost_usd,
  "usage".status_code,
  "usage".error_message,
  "usage".error_category,
  "usage".response_time_ms,
  "usage".first_byte_time_ms,
  "usage".status,
  COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) AS billing_status,
  COALESCE(usage_http_audits.request_headers, "usage".request_headers) AS request_headers,
  "usage".request_body,
  "usage".request_body_compressed,
  COALESCE(
    usage_http_audits.provider_request_headers,
    "usage".provider_request_headers
  ) AS provider_request_headers,
  "usage".provider_request_body,
  "usage".provider_request_body_compressed,
  COALESCE(usage_http_audits.response_headers, "usage".response_headers) AS response_headers,
  "usage".response_body,
  "usage".response_body_compressed,
  COALESCE(
    usage_http_audits.client_response_headers,
    "usage".client_response_headers
  ) AS client_response_headers,
  "usage".client_response_body,
  "usage".client_response_body_compressed,
  "usage".request_metadata,
  usage_http_audits.request_body_ref AS http_request_body_ref,
  usage_http_audits.provider_request_body_ref AS http_provider_request_body_ref,
  usage_http_audits.response_body_ref AS http_response_body_ref,
  usage_http_audits.client_response_body_ref AS http_client_response_body_ref,
  usage_routing_snapshots.candidate_id AS routing_candidate_id,
  usage_routing_snapshots.candidate_index AS routing_candidate_index,
  usage_routing_snapshots.key_name AS routing_key_name,
  usage_routing_snapshots.planner_kind AS routing_planner_kind,
  usage_routing_snapshots.route_family AS routing_route_family,
  usage_routing_snapshots.route_kind AS routing_route_kind,
  usage_routing_snapshots.execution_path AS routing_execution_path,
  usage_routing_snapshots.local_execution_runtime_miss_reason AS routing_local_execution_runtime_miss_reason,
  usage_settlement_snapshots.billing_snapshot_schema_version AS settlement_billing_snapshot_schema_version,
  usage_settlement_snapshots.billing_snapshot_status AS settlement_billing_snapshot_status,
  CAST(usage_settlement_snapshots.rate_multiplier AS DOUBLE PRECISION) AS settlement_rate_multiplier,
  usage_settlement_snapshots.is_free_tier AS settlement_is_free_tier,
  CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION) AS settlement_input_price_per_1m,
  CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION) AS settlement_output_price_per_1m,
  CAST(usage_settlement_snapshots.cache_creation_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_creation_price_per_1m,
  CAST(usage_settlement_snapshots.cache_read_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_read_price_per_1m,
  CAST(usage_settlement_snapshots.price_per_request AS DOUBLE PRECISION) AS settlement_price_per_request,
  usage_settlement_snapshots.settlement_snapshot_schema_version AS settlement_snapshot_schema_version,
  usage_settlement_snapshots.settlement_snapshot AS settlement_snapshot,
  usage_settlement_snapshots.billing_dimensions AS settlement_billing_dimensions,
  usage_settlement_snapshots.billing_input_tokens AS settlement_billing_input_tokens,
  usage_settlement_snapshots.billing_effective_input_tokens AS settlement_billing_effective_input_tokens,
  usage_settlement_snapshots.billing_output_tokens AS settlement_billing_output_tokens,
  usage_settlement_snapshots.billing_cache_creation_tokens AS settlement_billing_cache_creation_tokens,
  usage_settlement_snapshots.billing_cache_creation_5m_tokens AS settlement_billing_cache_creation_5m_tokens,
  usage_settlement_snapshots.billing_cache_creation_1h_tokens AS settlement_billing_cache_creation_1h_tokens,
  usage_settlement_snapshots.billing_cache_read_tokens AS settlement_billing_cache_read_tokens,
  usage_settlement_snapshots.billing_total_input_context AS settlement_billing_total_input_context,
  CAST(usage_settlement_snapshots.billing_cache_creation_cost_usd AS DOUBLE PRECISION) AS settlement_billing_cache_creation_cost_usd,
  CAST(usage_settlement_snapshots.billing_cache_read_cost_usd AS DOUBLE PRECISION) AS settlement_billing_cache_read_cost_usd,
  CAST(usage_settlement_snapshots.billing_total_cost_usd AS DOUBLE PRECISION) AS settlement_billing_total_cost_usd,
  CAST(usage_settlement_snapshots.billing_actual_total_cost_usd AS DOUBLE PRECISION) AS settlement_billing_actual_total_cost_usd,
  usage_settlement_snapshots.billing_pricing_source AS settlement_billing_pricing_source,
  usage_settlement_snapshots.billing_rule_id AS settlement_billing_rule_id,
  usage_settlement_snapshots.billing_rule_version AS settlement_billing_rule_version,
  CAST(EXTRACT(EPOCH FROM "usage".created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(
        usage_settlement_snapshots.finalized_at,
        "usage".finalized_at,
        "usage".created_at
      )
    ) AS BIGINT
  ) AS updated_at_unix_secs,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at)
    ) AS BIGINT
  ) AS finalized_at_unix_secs
FROM "usage"
LEFT JOIN usage_http_audits
  ON usage_http_audits.request_id = "usage".request_id
LEFT JOIN usage_routing_snapshots
  ON usage_routing_snapshots.request_id = "usage".request_id
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = "usage".request_id
WHERE "usage".id = $1
LIMIT 1
