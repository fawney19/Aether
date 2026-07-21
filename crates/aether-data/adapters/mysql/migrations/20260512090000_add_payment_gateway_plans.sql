ALTER TABLE payment_orders
  ADD COLUMN payment_provider VARCHAR(64),
  ADD COLUMN payment_channel VARCHAR(64),
  ADD COLUMN order_kind VARCHAR(64) NOT NULL DEFAULT 'wallet_recharge',
  ADD COLUMN product_id VARCHAR(64),
  ADD COLUMN product_snapshot TEXT,
  ADD COLUMN fulfillment_status VARCHAR(64) NOT NULL DEFAULT 'pending',
  ADD COLUMN fulfillment_error TEXT;

CREATE INDEX idx_payment_orders_kind_status
  ON payment_orders (order_kind, status);
CREATE INDEX idx_payment_orders_product
  ON payment_orders (product_id);

CREATE TABLE IF NOT EXISTS payment_gateway_configs (
    provider VARCHAR(64) PRIMARY KEY,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    endpoint_url VARCHAR(512) NOT NULL,
    callback_base_url VARCHAR(512),
    merchant_id VARCHAR(128) NOT NULL,
    merchant_key_encrypted TEXT,
    pay_currency VARCHAR(16) NOT NULL DEFAULT 'CNY',
    usd_exchange_rate DOUBLE NOT NULL DEFAULT 7.2,
    min_recharge_usd DOUBLE NOT NULL DEFAULT 1,
    channels_json TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS billing_plans (
    id VARCHAR(64) PRIMARY KEY,
    title VARCHAR(128) NOT NULL,
    description TEXT,
    price_amount DOUBLE NOT NULL,
    price_currency VARCHAR(16) NOT NULL DEFAULT 'CNY',
    duration_unit VARCHAR(32) NOT NULL,
    duration_value BIGINT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    sort_order BIGINT NOT NULL DEFAULT 0,
    max_active_per_user BIGINT NOT NULL DEFAULT 1,
    entitlements_json TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    KEY idx_billing_plans_enabled_sort (enabled, sort_order)
);

CREATE TABLE IF NOT EXISTS user_plan_entitlements (
    id VARCHAR(64) PRIMARY KEY,
    user_id VARCHAR(64) NOT NULL,
    plan_id VARCHAR(64) NOT NULL,
    payment_order_id VARCHAR(64) NOT NULL,
    status VARCHAR(64) NOT NULL DEFAULT 'active',
    starts_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    entitlements_snapshot TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    KEY idx_user_plan_entitlements_user_active (user_id, status, expires_at),
    KEY idx_user_plan_entitlements_order (payment_order_id),
    CONSTRAINT user_plan_entitlements_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT user_plan_entitlements_plan_id_fkey FOREIGN KEY (plan_id) REFERENCES billing_plans(id) ON DELETE RESTRICT,
    CONSTRAINT user_plan_entitlements_payment_order_id_fkey FOREIGN KEY (payment_order_id) REFERENCES payment_orders(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS entitlement_usage_ledgers (
    id VARCHAR(64) PRIMARY KEY,
    user_entitlement_id VARCHAR(64) NOT NULL,
    user_id VARCHAR(64) NOT NULL,
    request_id VARCHAR(128) NOT NULL,
    amount_usd DOUBLE NOT NULL,
    balance_before DOUBLE NOT NULL,
    balance_after DOUBLE NOT NULL,
    usage_date VARCHAR(16) NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE KEY uq_entitlement_usage_request (user_entitlement_id, request_id),
    KEY idx_entitlement_usage_user_date (user_id, usage_date),
    CONSTRAINT entitlement_usage_ledgers_entitlement_fkey FOREIGN KEY (user_entitlement_id) REFERENCES user_plan_entitlements(id) ON DELETE CASCADE,
    CONSTRAINT entitlement_usage_ledgers_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
