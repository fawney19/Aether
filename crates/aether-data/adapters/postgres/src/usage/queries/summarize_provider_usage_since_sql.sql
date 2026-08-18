SELECT
  COUNT(*)::BIGINT AS total_requests,
  COUNT(*) FILTER (WHERE sla_eligible)::BIGINT AS sla_eligible_requests,
  COUNT(*) FILTER (WHERE outcome_class = 'success')::BIGINT AS successful_requests,
  COUNT(*) FILTER (WHERE outcome_class = 'service_error')::BIGINT AS failed_requests,
  COUNT(*) FILTER (WHERE outcome_class = 'user_error')::BIGINT AS user_error_requests,
  COALESCE(AVG(GREATEST(response_time_ms, 0)) FILTER (WHERE response_time_ms IS NOT NULL), 0) AS avg_response_time_ms,
  COALESCE(SUM(total_cost_usd), 0) AS total_cost_usd
FROM usage_billing_facts
WHERE provider_id = $1
  AND created_at >= TO_TIMESTAMP($2::double precision)
