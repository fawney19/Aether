SELECT
  COALESCE(SUM(total_requests), 0) AS total_requests,
  COALESCE(SUM(successful_requests), 0) AS successful_requests,
  COALESCE(SUM(failed_requests), 0) AS failed_requests,
  COALESCE(AVG(avg_response_time_ms), 0) AS avg_response_time_ms,
  COALESCE(SUM(total_cost_usd), 0) AS total_cost_usd
FROM provider_usage_tracking
WHERE provider_id = $1
  AND window_start >= TO_TIMESTAMP($2::double precision)
