CREATE TABLE IF NOT EXISTS wallets (
    id TEXT PRIMARY KEY,
    user_id TEXT UNIQUE,
    api_key_id TEXT UNIQUE,
    balance REAL NOT NULL DEFAULT 0,
    gift_balance REAL NOT NULL DEFAULT 0,
    limit_mode TEXT NOT NULL DEFAULT 'finite',
    currency TEXT NOT NULL DEFAULT 'USD',
    status TEXT NOT NULL DEFAULT 'active',
    total_recharged REAL NOT NULL DEFAULT 0,
    total_consumed REAL NOT NULL DEFAULT 0,
    total_refunded REAL NOT NULL DEFAULT 0,
    total_adjusted REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS wallets_api_key_id_idx ON wallets (api_key_id);
CREATE INDEX IF NOT EXISTS wallets_user_id_idx ON wallets (user_id);

CREATE TABLE IF NOT EXISTS wallet_transactions (
    id TEXT PRIMARY KEY,
    wallet_id TEXT NOT NULL,
    category TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    amount REAL NOT NULL,
    balance_before REAL NOT NULL,
    balance_after REAL NOT NULL,
    recharge_balance_before REAL NOT NULL,
    recharge_balance_after REAL NOT NULL,
    gift_balance_before REAL NOT NULL,
    gift_balance_after REAL NOT NULL,
    link_type TEXT,
    link_id TEXT,
    operator_id TEXT,
    description TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wallet_tx_wallet_created
    ON wallet_transactions (wallet_id, created_at);
CREATE INDEX IF NOT EXISTS idx_wallet_tx_category_created
    ON wallet_transactions (category, created_at);
CREATE INDEX IF NOT EXISTS idx_wallet_tx_reason_created
    ON wallet_transactions (reason_code, created_at);
CREATE INDEX IF NOT EXISTS idx_wallet_tx_link
    ON wallet_transactions (link_type, link_id);
CREATE INDEX IF NOT EXISTS ix_wallet_transactions_operator_id
    ON wallet_transactions (operator_id);

CREATE TABLE IF NOT EXISTS wallet_daily_usage_ledgers (
    id TEXT PRIMARY KEY,
    wallet_id TEXT NOT NULL,
    billing_date TEXT NOT NULL,
    billing_timezone TEXT NOT NULL,
    total_cost_usd REAL NOT NULL DEFAULT 0,
    total_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    first_finalized_at INTEGER,
    last_finalized_at INTEGER,
    aggregated_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wallet_daily_usage_wallet_date
    ON wallet_daily_usage_ledgers (wallet_id, billing_timezone, billing_date);

CREATE TABLE IF NOT EXISTS payment_orders (
    id TEXT PRIMARY KEY,
    order_no TEXT NOT NULL UNIQUE,
    wallet_id TEXT NOT NULL,
    user_id TEXT,
    amount_usd REAL NOT NULL,
    pay_amount REAL,
    pay_currency TEXT,
    exchange_rate REAL,
    refunded_amount_usd REAL NOT NULL DEFAULT 0,
    refundable_amount_usd REAL NOT NULL DEFAULT 0,
    payment_method TEXT NOT NULL,
    gateway_order_id TEXT,
    gateway_response TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    paid_at INTEGER,
    credited_at INTEGER,
    expires_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_payment_orders_wallet_created
    ON payment_orders (wallet_id, created_at);
CREATE INDEX IF NOT EXISTS idx_payment_orders_user_created
    ON payment_orders (user_id, created_at);
CREATE INDEX IF NOT EXISTS idx_payment_orders_status
    ON payment_orders (status);
CREATE INDEX IF NOT EXISTS idx_payment_orders_gateway_order_id
    ON payment_orders (gateway_order_id);

CREATE TABLE IF NOT EXISTS payment_callbacks (
    id TEXT PRIMARY KEY,
    payment_order_id TEXT,
    payment_method TEXT NOT NULL,
    callback_key TEXT NOT NULL UNIQUE,
    order_no TEXT,
    gateway_order_id TEXT,
    payload_hash TEXT,
    signature_valid INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'received',
    payload TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    processed_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_payment_callbacks_order
    ON payment_callbacks (order_no);
CREATE INDEX IF NOT EXISTS idx_payment_callbacks_gateway_order
    ON payment_callbacks (gateway_order_id);
CREATE INDEX IF NOT EXISTS idx_payment_callbacks_created
    ON payment_callbacks (created_at);
CREATE INDEX IF NOT EXISTS ix_payment_callbacks_payment_order_id
    ON payment_callbacks (payment_order_id);

CREATE TABLE IF NOT EXISTS refund_requests (
    id TEXT PRIMARY KEY,
    refund_no TEXT NOT NULL UNIQUE,
    wallet_id TEXT NOT NULL,
    user_id TEXT,
    payment_order_id TEXT,
    source_type TEXT NOT NULL,
    source_id TEXT,
    refund_mode TEXT NOT NULL,
    amount_usd REAL NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending_approval',
    reason TEXT,
    requested_by TEXT,
    approved_by TEXT,
    processed_by TEXT,
    gateway_refund_id TEXT,
    payout_method TEXT,
    payout_reference TEXT,
    payout_proof TEXT,
    failure_reason TEXT,
    idempotency_key TEXT UNIQUE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    processed_at INTEGER,
    completed_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_refund_wallet_created
    ON refund_requests (wallet_id, created_at);
CREATE INDEX IF NOT EXISTS idx_refund_user_created
    ON refund_requests (user_id, created_at);
CREATE INDEX IF NOT EXISTS idx_refund_status
    ON refund_requests (status);
CREATE INDEX IF NOT EXISTS ix_refund_requests_payment_order_id
    ON refund_requests (payment_order_id);
CREATE INDEX IF NOT EXISTS ix_refund_requests_requested_by
    ON refund_requests (requested_by);
CREATE INDEX IF NOT EXISTS ix_refund_requests_approved_by
    ON refund_requests (approved_by);
CREATE INDEX IF NOT EXISTS ix_refund_requests_processed_by
    ON refund_requests (processed_by);

CREATE TABLE IF NOT EXISTS redeem_code_batches (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    amount_usd REAL NOT NULL,
    currency TEXT NOT NULL DEFAULT 'USD',
    balance_bucket TEXT NOT NULL DEFAULT 'gift',
    total_count INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    description TEXT,
    created_by TEXT,
    expires_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_redeem_code_batches_status
    ON redeem_code_batches (status, created_at);

CREATE TABLE IF NOT EXISTS redeem_codes (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL,
    code_hash TEXT NOT NULL UNIQUE,
    code_prefix TEXT NOT NULL,
    code_suffix TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    redeemed_by_user_id TEXT,
    redeemed_wallet_id TEXT,
    redeemed_payment_order_id TEXT,
    redeemed_at INTEGER,
    disabled_by TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_batch_created
    ON redeem_codes (batch_id, created_at);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_status
    ON redeem_codes (status, updated_at);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_redeemed_user
    ON redeem_codes (redeemed_by_user_id, redeemed_at);
CREATE INDEX IF NOT EXISTS idx_redeem_codes_redeemed_order
    ON redeem_codes (redeemed_payment_order_id);
