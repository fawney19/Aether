UPDATE "usage"
SET
  billing_status = 'pending',
  finalized_at = NULL
WHERE request_id = $1
  AND billing_status = 'void'
  AND status IN ('failed', 'cancelled')
