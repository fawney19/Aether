ALTER TABLE billing_plans
  ADD COLUMN purchase_limit_scope VARCHAR(32) NOT NULL DEFAULT 'active_period';
