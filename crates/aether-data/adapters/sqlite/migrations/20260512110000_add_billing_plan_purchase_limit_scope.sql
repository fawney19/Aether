ALTER TABLE billing_plans
  ADD COLUMN purchase_limit_scope TEXT NOT NULL DEFAULT 'active_period';
