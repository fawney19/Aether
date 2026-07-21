UPDATE usage_settlement_snapshots
SET
  billing_status = 'pending',
  finalized_at = NULL,
  updated_at = NOW()
WHERE request_id = $1
  AND billing_status = 'void'
