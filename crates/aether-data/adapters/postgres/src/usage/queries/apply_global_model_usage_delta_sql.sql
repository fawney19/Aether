UPDATE global_models
SET
  usage_count = GREATEST(COALESCE(usage_count, 0) + $2, 0),
  updated_at = NOW()
WHERE name = $1
