UPDATE provider_api_keys
SET
  request_count = 0,
  success_count = 0,
  error_count = 0,
  total_tokens = 0,
  total_cost_usd = 0,
  total_response_time_ms = 0,
  last_used_at = NULL
