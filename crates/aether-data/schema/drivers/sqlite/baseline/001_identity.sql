CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    external_id TEXT,
    email TEXT UNIQUE,
    username TEXT UNIQUE,
    password_hash TEXT,
    role TEXT,
    auth_source TEXT NOT NULL DEFAULT 'local',
    email_verified INTEGER NOT NULL DEFAULT 0,
    is_active INTEGER NOT NULL DEFAULT 1,
    is_deleted INTEGER NOT NULL DEFAULT 0,
    allowed_models TEXT,
    allowed_providers TEXT,
    allowed_api_formats TEXT,
    model_capability_settings TEXT,
    rate_limit INTEGER,
    metadata TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_login_at INTEGER,
    ldap_dn TEXT,
    ldap_username TEXT
);

CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    key_encrypted TEXT,
    name TEXT,
    key_prefix TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    allowed_models TEXT,
    allowed_providers TEXT,
    allowed_api_formats TEXT,
    rate_limit INTEGER DEFAULT 100,
    concurrent_limit INTEGER,
    force_capabilities TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    is_locked INTEGER NOT NULL DEFAULT 0,
    is_standalone INTEGER NOT NULL DEFAULT 0,
    auto_delete_on_expiry INTEGER NOT NULL DEFAULT 0,
    total_requests INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_usd REAL NOT NULL DEFAULT 0,
    metadata TEXT,
    expires_at INTEGER,
    last_used_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS api_keys_user_id_idx ON api_keys (user_id);

CREATE TABLE IF NOT EXISTS audit_logs (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    user_id TEXT,
    api_key_id TEXT,
    description TEXT NOT NULL,
    ip_address TEXT,
    user_agent TEXT,
    request_id TEXT,
    event_metadata TEXT,
    status_code INTEGER,
    error_message TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_logs_created_at_idx ON audit_logs (created_at);
CREATE INDEX IF NOT EXISTS audit_logs_event_type_idx ON audit_logs (event_type);
CREATE INDEX IF NOT EXISTS audit_logs_request_id_idx ON audit_logs (request_id);
CREATE INDEX IF NOT EXISTS audit_logs_user_id_idx ON audit_logs (user_id);

CREATE TABLE IF NOT EXISTS announcements (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    type TEXT NOT NULL DEFAULT 'info',
    priority INTEGER NOT NULL DEFAULT 0,
    author_id TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    is_pinned INTEGER NOT NULL DEFAULT 0,
    start_time INTEGER,
    end_time INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS announcements_author_id_idx ON announcements (author_id);
CREATE INDEX IF NOT EXISTS announcements_created_at_idx ON announcements (created_at);
CREATE INDEX IF NOT EXISTS announcements_is_active_idx ON announcements (is_active);

CREATE TABLE IF NOT EXISTS announcement_reads (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    announcement_id TEXT NOT NULL,
    read_at INTEGER NOT NULL,
    UNIQUE (user_id, announcement_id)
);
CREATE INDEX IF NOT EXISTS announcement_reads_announcement_id_idx ON announcement_reads (announcement_id);
CREATE INDEX IF NOT EXISTS announcement_reads_user_id_idx ON announcement_reads (user_id);

CREATE TABLE IF NOT EXISTS management_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    token_hash TEXT NOT NULL UNIQUE,
    token_prefix TEXT,
    allowed_ips TEXT,
    expires_at INTEGER,
    last_used_at INTEGER,
    last_used_ip TEXT,
    usage_count INTEGER NOT NULL DEFAULT 0,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (user_id, name)
);
CREATE INDEX IF NOT EXISTS management_tokens_user_id_idx ON management_tokens (user_id);

CREATE TABLE IF NOT EXISTS user_preferences (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL UNIQUE,
    avatar_url TEXT,
    bio TEXT,
    default_provider_id TEXT,
    theme TEXT NOT NULL DEFAULT 'light',
    language TEXT NOT NULL DEFAULT 'zh-CN',
    timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
    email_notifications INTEGER NOT NULL DEFAULT 1,
    usage_alerts INTEGER NOT NULL DEFAULT 1,
    announcement_notifications INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS user_preferences_default_provider_id_idx
    ON user_preferences (default_provider_id);
CREATE INDEX IF NOT EXISTS user_preferences_user_id_idx
    ON user_preferences (user_id);

CREATE TABLE IF NOT EXISTS user_sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    client_device_id TEXT NOT NULL,
    device_label TEXT,
    device_type TEXT NOT NULL DEFAULT 'unknown',
    browser_name TEXT,
    browser_version TEXT,
    os_name TEXT,
    os_version TEXT,
    device_model TEXT,
    ip_address TEXT,
    user_agent TEXT,
    client_hints TEXT,
    refresh_token_hash TEXT NOT NULL,
    prev_refresh_token_hash TEXT,
    rotated_at INTEGER,
    last_seen_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    revoke_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS user_sessions_user_active_idx
    ON user_sessions (user_id, revoked_at, expires_at);
CREATE INDEX IF NOT EXISTS user_sessions_user_device_idx
    ON user_sessions (user_id, client_device_id);
