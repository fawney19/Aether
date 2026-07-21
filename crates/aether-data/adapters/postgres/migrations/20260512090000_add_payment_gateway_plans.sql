ALTER TABLE public.payment_orders
  ADD COLUMN payment_provider character varying(64),
  ADD COLUMN payment_channel character varying(64),
  ADD COLUMN order_kind character varying(64) NOT NULL DEFAULT 'wallet_recharge',
  ADD COLUMN product_id character varying(64),
  ADD COLUMN product_snapshot jsonb,
  ADD COLUMN fulfillment_status character varying(64) NOT NULL DEFAULT 'pending',
  ADD COLUMN fulfillment_error text;

CREATE INDEX IF NOT EXISTS idx_payment_orders_kind_status
  ON public.payment_orders USING btree (order_kind, status);
CREATE INDEX IF NOT EXISTS idx_payment_orders_product
  ON public.payment_orders USING btree (product_id);

CREATE TABLE IF NOT EXISTS public.payment_gateway_configs (
    provider character varying(64) PRIMARY KEY,
    enabled boolean NOT NULL DEFAULT false,
    endpoint_url character varying(512) NOT NULL,
    callback_base_url character varying(512),
    merchant_id character varying(128) NOT NULL,
    merchant_key_encrypted text,
    pay_currency character varying(16) NOT NULL DEFAULT 'CNY',
    usd_exchange_rate numeric(18,8) NOT NULL DEFAULT 7.2,
    min_recharge_usd numeric(20,8) NOT NULL DEFAULT 1,
    channels_json jsonb,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);

CREATE TABLE IF NOT EXISTS public.billing_plans (
    id character varying(64) PRIMARY KEY,
    title character varying(128) NOT NULL,
    description text,
    price_amount numeric(20,8) NOT NULL,
    price_currency character varying(16) NOT NULL DEFAULT 'CNY',
    duration_unit character varying(32) NOT NULL,
    duration_value bigint NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    sort_order bigint NOT NULL DEFAULT 0,
    max_active_per_user bigint NOT NULL DEFAULT 1,
    entitlements_json jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_billing_plans_enabled_sort
  ON public.billing_plans USING btree (enabled, sort_order);

CREATE TABLE IF NOT EXISTS public.user_plan_entitlements (
    id character varying(64) PRIMARY KEY,
    user_id character varying(64) NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    plan_id character varying(64) NOT NULL REFERENCES public.billing_plans(id) ON DELETE RESTRICT,
    payment_order_id character varying(64) NOT NULL REFERENCES public.payment_orders(id) ON DELETE RESTRICT,
    status character varying(64) NOT NULL DEFAULT 'active',
    starts_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    entitlements_snapshot jsonb NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_user_plan_entitlements_user_active
  ON public.user_plan_entitlements USING btree (user_id, status, expires_at);
CREATE INDEX IF NOT EXISTS idx_user_plan_entitlements_order
  ON public.user_plan_entitlements USING btree (payment_order_id);

CREATE TABLE IF NOT EXISTS public.entitlement_usage_ledgers (
    id character varying(64) PRIMARY KEY,
    user_entitlement_id character varying(64) NOT NULL REFERENCES public.user_plan_entitlements(id) ON DELETE CASCADE,
    user_id character varying(64) NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    request_id character varying(128) NOT NULL,
    amount_usd numeric(20,8) NOT NULL,
    balance_before numeric(20,8) NOT NULL,
    balance_after numeric(20,8) NOT NULL,
    usage_date character varying(16) NOT NULL,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT uq_entitlement_usage_request UNIQUE (user_entitlement_id, request_id)
);

CREATE INDEX IF NOT EXISTS idx_entitlement_usage_user_date
  ON public.entitlement_usage_ledgers USING btree (user_id, usage_date);
