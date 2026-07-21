ALTER TABLE public.billing_plans
  ADD COLUMN IF NOT EXISTS purchase_limit_scope character varying(32) NOT NULL DEFAULT 'active_period';
