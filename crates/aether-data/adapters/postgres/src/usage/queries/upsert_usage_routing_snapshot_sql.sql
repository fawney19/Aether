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
  candidate_id = CASE WHEN $14 THEN EXCLUDED.candidate_id ELSE COALESCE(EXCLUDED.candidate_id, usage_routing_snapshots.candidate_id) END,
  candidate_index = CASE WHEN $14 THEN EXCLUDED.candidate_index ELSE COALESCE(EXCLUDED.candidate_index, usage_routing_snapshots.candidate_index) END,
  key_name = CASE WHEN $14 THEN EXCLUDED.key_name ELSE COALESCE(EXCLUDED.key_name, usage_routing_snapshots.key_name) END,
  planner_kind = CASE WHEN $14 THEN EXCLUDED.planner_kind ELSE COALESCE(EXCLUDED.planner_kind, usage_routing_snapshots.planner_kind) END,
  route_family = CASE WHEN $14 THEN EXCLUDED.route_family ELSE COALESCE(EXCLUDED.route_family, usage_routing_snapshots.route_family) END,
  route_kind = CASE WHEN $14 THEN EXCLUDED.route_kind ELSE COALESCE(EXCLUDED.route_kind, usage_routing_snapshots.route_kind) END,
  execution_path = CASE WHEN $14 THEN EXCLUDED.execution_path ELSE COALESCE(EXCLUDED.execution_path, usage_routing_snapshots.execution_path) END,
  local_execution_runtime_miss_reason = CASE WHEN $14 THEN EXCLUDED.local_execution_runtime_miss_reason ELSE COALESCE(EXCLUDED.local_execution_runtime_miss_reason, usage_routing_snapshots.local_execution_runtime_miss_reason) END,
  selected_provider_id = CASE WHEN $14 THEN EXCLUDED.selected_provider_id ELSE COALESCE(EXCLUDED.selected_provider_id, usage_routing_snapshots.selected_provider_id) END,
  selected_endpoint_id = CASE WHEN $14 THEN EXCLUDED.selected_endpoint_id ELSE COALESCE(EXCLUDED.selected_endpoint_id, usage_routing_snapshots.selected_endpoint_id) END,
  selected_provider_api_key_id = CASE WHEN $14 THEN EXCLUDED.selected_provider_api_key_id ELSE COALESCE(EXCLUDED.selected_provider_api_key_id, usage_routing_snapshots.selected_provider_api_key_id) END,
  has_format_conversion = CASE WHEN $14 THEN EXCLUDED.has_format_conversion ELSE COALESCE(EXCLUDED.has_format_conversion, usage_routing_snapshots.has_format_conversion) END,
  updated_at = NOW()
