CREATE TABLE IF NOT EXISTS "usage" (
    request_id TEXT PRIMARY KEY,
    id TEXT,
    user_id TEXT,
    api_key_id TEXT,
    provider_name TEXT NOT NULL DEFAULT 'unknown',
    model TEXT NOT NULL DEFAULT 'unknown',
    target_model TEXT,
    provider_id TEXT,
    provider_endpoint_id TEXT,
    provider_api_key_id TEXT,
    request_type TEXT,
    api_format TEXT,
    api_family TEXT,
    endpoint_kind TEXT,
    endpoint_api_format TEXT,
    provider_api_family TEXT,
    provider_endpoint_kind TEXT,
    has_format_conversion INTEGER NOT NULL DEFAULT 0,
    is_stream INTEGER NOT NULL DEFAULT 0,
    upstream_is_stream INTEGER,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_ephemeral_5m_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_ephemeral_1h_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_cost_usd REAL NOT NULL DEFAULT 0,
    cache_read_cost_usd REAL NOT NULL DEFAULT 0,
    output_price_per_1m REAL,
    status_code INTEGER,
    error_message TEXT,
    error_category TEXT,
    response_time_ms INTEGER,
    first_byte_time_ms INTEGER,
    wallet_id TEXT,
    status TEXT NOT NULL DEFAULT 'completed',
    billing_status TEXT NOT NULL DEFAULT 'pending',
    total_cost_usd REAL NOT NULL DEFAULT 0,
    actual_total_cost_usd REAL NOT NULL DEFAULT 0,
    request_metadata TEXT,
    candidate_id TEXT,
    candidate_index INTEGER,
    key_name TEXT,
    planner_kind TEXT,
    route_family TEXT,
    route_kind TEXT,
    execution_path TEXT,
    local_execution_runtime_miss_reason TEXT,
    wallet_balance_before REAL,
    wallet_balance_after REAL,
    wallet_recharge_balance_before REAL,
    wallet_recharge_balance_after REAL,
    wallet_gift_balance_before REAL,
    wallet_gift_balance_after REAL,
    finalized_at INTEGER,
    created_at_unix_ms INTEGER NOT NULL DEFAULT 0,
    updated_at_unix_secs INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS usage_api_key_id_idx ON "usage" (api_key_id);
CREATE INDEX IF NOT EXISTS usage_billing_status_idx ON "usage" (billing_status);
CREATE INDEX IF NOT EXISTS usage_created_at_idx ON "usage" (created_at_unix_ms);
CREATE INDEX IF NOT EXISTS usage_provider_api_key_id_idx ON "usage" (provider_api_key_id);
CREATE INDEX IF NOT EXISTS usage_provider_id_idx ON "usage" (provider_id);
CREATE INDEX IF NOT EXISTS usage_request_id_idx ON "usage" (request_id);
CREATE INDEX IF NOT EXISTS usage_user_id_idx ON "usage" (user_id);
CREATE INDEX IF NOT EXISTS usage_wallet_id_idx ON "usage" (wallet_id);

CREATE TABLE IF NOT EXISTS usage_settlement_snapshots (
    request_id TEXT PRIMARY KEY,
    billing_status TEXT NOT NULL,
    wallet_id TEXT,
    wallet_balance_before REAL,
    wallet_balance_after REAL,
    wallet_recharge_balance_before REAL,
    wallet_recharge_balance_after REAL,
    wallet_gift_balance_before REAL,
    wallet_gift_balance_after REAL,
    provider_monthly_used_usd REAL,
    finalized_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS usage_settlement_snapshots_billing_status_idx
    ON usage_settlement_snapshots (billing_status);
CREATE INDEX IF NOT EXISTS usage_settlement_snapshots_wallet_id_idx
    ON usage_settlement_snapshots (wallet_id);


CREATE TABLE IF NOT EXISTS stats_hourly (
    id TEXT PRIMARY KEY,
    hour_utc INTEGER NOT NULL UNIQUE,
    total_requests INTEGER NOT NULL DEFAULT 0,
    success_requests INTEGER NOT NULL DEFAULT 0,
    error_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    actual_total_cost REAL NOT NULL DEFAULT 0,
    avg_response_time_ms REAL NOT NULL DEFAULT 0,
    is_complete INTEGER NOT NULL DEFAULT 0,
    aggregated_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS stats_hourly_user (
    id TEXT PRIMARY KEY,
    hour_utc INTEGER NOT NULL,
    user_id TEXT NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    success_requests INTEGER NOT NULL DEFAULT 0,
    error_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (hour_utc, user_id)
);

CREATE TABLE IF NOT EXISTS stats_hourly_user_model (
    id TEXT PRIMARY KEY,
    hour_utc INTEGER NOT NULL,
    user_id TEXT NOT NULL,
    model TEXT NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (hour_utc, user_id, model)
);

CREATE TABLE IF NOT EXISTS stats_hourly_model (
    id TEXT PRIMARY KEY,
    hour_utc INTEGER NOT NULL,
    model TEXT NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    avg_response_time_ms REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (hour_utc, model)
);

CREATE TABLE IF NOT EXISTS stats_hourly_provider (
    id TEXT PRIMARY KEY,
    hour_utc INTEGER NOT NULL,
    provider_name TEXT NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (hour_utc, provider_name)
);

CREATE TABLE IF NOT EXISTS stats_daily (
    id TEXT PRIMARY KEY,
    "date" INTEGER NOT NULL UNIQUE,
    total_requests INTEGER NOT NULL DEFAULT 0,
    success_requests INTEGER NOT NULL DEFAULT 0,
    error_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    actual_total_cost REAL NOT NULL DEFAULT 0,
    avg_response_time_ms REAL NOT NULL DEFAULT 0,
    fallback_count INTEGER NOT NULL DEFAULT 0,
    unique_models INTEGER NOT NULL DEFAULT 0,
    unique_providers INTEGER NOT NULL DEFAULT 0,
    is_complete INTEGER NOT NULL DEFAULT 0,
    aggregated_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS stats_daily_model (
    id TEXT PRIMARY KEY,
    "date" INTEGER NOT NULL,
    model TEXT NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    avg_response_time_ms REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE ("date", model)
);

CREATE TABLE IF NOT EXISTS stats_daily_provider (
    id TEXT PRIMARY KEY,
    "date" INTEGER NOT NULL,
    provider_name TEXT NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE ("date", provider_name)
);

CREATE TABLE IF NOT EXISTS stats_daily_api_key (
    id TEXT PRIMARY KEY,
    api_key_id TEXT NOT NULL,
    "date" INTEGER NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    success_requests INTEGER NOT NULL DEFAULT 0,
    error_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    api_key_name TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE ("date", api_key_id)
);

CREATE TABLE IF NOT EXISTS stats_daily_error (
    id TEXT PRIMARY KEY,
    "date" INTEGER NOT NULL,
    error_category TEXT NOT NULL,
    provider_name TEXT,
    model TEXT,
    count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE ("date", error_category, provider_name, model)
);

CREATE TABLE IF NOT EXISTS stats_user_daily (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    "date" INTEGER NOT NULL,
    total_requests INTEGER NOT NULL DEFAULT 0,
    success_requests INTEGER NOT NULL DEFAULT 0,
    error_requests INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost REAL NOT NULL DEFAULT 0,
    username TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE ("date", user_id)
);
