CREATE TABLE IF NOT EXISTS system_configs (
    id TEXT PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    value TEXT NOT NULL,
    description TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS auth_modules (
    id TEXT PRIMARY KEY,
    module_type TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL DEFAULT 1,
    config TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS oauth_providers (
    provider_type TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    client_id TEXT NOT NULL,
    client_secret_encrypted TEXT,
    authorization_url_override TEXT,
    token_url_override TEXT,
    userinfo_url_override TEXT,
    scopes TEXT,
    redirect_uri TEXT NOT NULL,
    frontend_callback_url TEXT NOT NULL,
    attribute_mapping TEXT,
    extra_config TEXT,
    is_enabled INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ldap_configs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_url TEXT NOT NULL,
    bind_dn TEXT NOT NULL,
    bind_password_encrypted TEXT,
    base_dn TEXT NOT NULL,
    user_search_filter TEXT DEFAULT '(uid={username})' NOT NULL,
    username_attr TEXT DEFAULT 'uid' NOT NULL,
    email_attr TEXT DEFAULT 'mail' NOT NULL,
    display_name_attr TEXT DEFAULT 'cn' NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 0,
    is_exclusive INTEGER NOT NULL DEFAULT 0,
    use_starttls INTEGER NOT NULL DEFAULT 0,
    connect_timeout INTEGER NOT NULL DEFAULT 10,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS user_oauth_links (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    provider_username TEXT,
    provider_email TEXT,
    extra_data TEXT,
    linked_at INTEGER NOT NULL,
    last_login_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_user_oauth_links_provider_user
    ON user_oauth_links (provider_type, provider_user_id);
CREATE UNIQUE INDEX IF NOT EXISTS uq_user_oauth_links_user_provider
    ON user_oauth_links (user_id, provider_type);
CREATE INDEX IF NOT EXISTS user_oauth_links_provider_type_idx ON user_oauth_links (provider_type);
CREATE INDEX IF NOT EXISTS user_oauth_links_user_id_idx ON user_oauth_links (user_id);
