ALTER TABLE payment_orders ADD COLUMN payment_provider TEXT;
ALTER TABLE payment_orders ADD COLUMN payment_channel TEXT;
ALTER TABLE payment_orders ADD COLUMN order_kind TEXT NOT NULL DEFAULT 'wallet_recharge';
ALTER TABLE payment_orders ADD COLUMN product_id TEXT;
ALTER TABLE payment_orders ADD COLUMN product_snapshot TEXT;
ALTER TABLE payment_orders ADD COLUMN fulfillment_status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE payment_orders ADD COLUMN fulfillment_error TEXT;

CREATE INDEX IF NOT EXISTS idx_payment_orders_kind_status
  ON payment_orders (order_kind, status);
CREATE INDEX IF NOT EXISTS idx_payment_orders_product
  ON payment_orders (product_id);

CREATE TABLE IF NOT EXISTS payment_gateway_configs (
    provider TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 0,
    endpoint_url TEXT NOT NULL,
    callback_base_url TEXT,
    merchant_id TEXT NOT NULL,
    merchant_key_encrypted TEXT,
    pay_currency TEXT NOT NULL DEFAULT 'CNY',
    usd_exchange_rate REAL NOT NULL DEFAULT 7.2,
    min_recharge_usd REAL NOT NULL DEFAULT 1,
    channels_json TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS billing_plans (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    price_amount REAL NOT NULL,
    price_currency TEXT NOT NULL DEFAULT 'CNY',
    duration_unit TEXT NOT NULL,
    duration_value INTEGER NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    sort_order INTEGER NOT NULL DEFAULT 0,
    max_active_per_user INTEGER NOT NULL DEFAULT 1,
    entitlements_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_billing_plans_enabled_sort
  ON billing_plans (enabled, sort_order);

CREATE TABLE IF NOT EXISTS user_plan_entitlements (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    payment_order_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    starts_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    entitlements_snapshot TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(plan_id) REFERENCES billing_plans(id) ON DELETE RESTRICT,
    FOREIGN KEY(payment_order_id) REFERENCES payment_orders(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_user_plan_entitlements_user_active
  ON user_plan_entitlements (user_id, status, expires_at);
CREATE INDEX IF NOT EXISTS idx_user_plan_entitlements_order
  ON user_plan_entitlements (payment_order_id);

CREATE TABLE IF NOT EXISTS entitlement_usage_ledgers (
    id TEXT PRIMARY KEY,
    user_entitlement_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    amount_usd REAL NOT NULL,
    balance_before REAL NOT NULL,
    balance_after REAL NOT NULL,
    usage_date TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (user_entitlement_id, request_id),
    FOREIGN KEY(user_entitlement_id) REFERENCES user_plan_entitlements(id) ON DELETE CASCADE,
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_entitlement_usage_user_date
  ON entitlement_usage_ledgers (user_id, usage_date);
