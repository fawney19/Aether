INSERT INTO usage_routing_snapshots (
  request_id,
  candidate_id,
  candidate_index,
  key_name,
  planner_kind,
  route_family,
  route_kind,
  execution_path,
  local_execution_runtime_miss_reason,
  selected_provider_id,
  selected_endpoint_id,
  selected_provider_api_key_id,
  has_format_conversion
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
  $13
)
ON CONFLICT (request_id)
DO UPDATE SET
  candidate_id = COALESCE(EXCLUDED.candidate_id, usage_routing_snapshots.candidate_id),
  candidate_index = COALESCE(
    EXCLUDED.candidate_index,
    usage_routing_snapshots.candidate_index
  ),
  key_name = COALESCE(EXCLUDED.key_name, usage_routing_snapshots.key_name),
  planner_kind = COALESCE(EXCLUDED.planner_kind, usage_routing_snapshots.planner_kind),
  route_family = COALESCE(EXCLUDED.route_family, usage_routing_snapshots.route_family),
  route_kind = COALESCE(EXCLUDED.route_kind, usage_routing_snapshots.route_kind),
  execution_path = COALESCE(EXCLUDED.execution_path, usage_routing_snapshots.execution_path),
  local_execution_runtime_miss_reason = COALESCE(
    EXCLUDED.local_execution_runtime_miss_reason,
    usage_routing_snapshots.local_execution_runtime_miss_reason
  ),
  selected_provider_id = COALESCE(
    EXCLUDED.selected_provider_id,
    usage_routing_snapshots.selected_provider_id
  ),
  selected_endpoint_id = COALESCE(
    EXCLUDED.selected_endpoint_id,
    usage_routing_snapshots.selected_endpoint_id
  ),
  selected_provider_api_key_id = COALESCE(
    EXCLUDED.selected_provider_api_key_id,
    usage_routing_snapshots.selected_provider_api_key_id
  ),
  has_format_conversion = COALESCE(
    EXCLUDED.has_format_conversion,
    usage_routing_snapshots.has_format_conversion
  ),
  updated_at = NOW()
