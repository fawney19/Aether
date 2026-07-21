CREATE TABLE IF NOT EXISTS system_configs (
    id VARCHAR(64) PRIMARY KEY,
    `key` VARCHAR(255) NOT NULL,
    value TEXT NOT NULL,
    description TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE KEY system_configs_key_key (`key`)
);

CREATE TABLE IF NOT EXISTS auth_modules (
    id VARCHAR(64) PRIMARY KEY,
    module_type VARCHAR(128) NOT NULL,
    enabled TINYINT(1) NOT NULL DEFAULT 1,
    config TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE KEY auth_modules_module_type_key (module_type)
);

CREATE TABLE IF NOT EXISTS oauth_providers (
    provider_type VARCHAR(64) PRIMARY KEY,
    display_name VARCHAR(255) NOT NULL,
    client_id TEXT NOT NULL,
    client_secret_encrypted TEXT,
    authorization_url_override VARCHAR(500),
    token_url_override VARCHAR(500),
    userinfo_url_override VARCHAR(500),
    scopes TEXT,
    redirect_uri VARCHAR(500) NOT NULL,
    frontend_callback_url VARCHAR(500) NOT NULL,
    attribute_mapping TEXT,
    extra_config TEXT,
    is_enabled TINYINT(1) NOT NULL DEFAULT 0,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS ldap_configs (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    server_url VARCHAR(255) NOT NULL,
    bind_dn TEXT NOT NULL,
    bind_password_encrypted TEXT,
    base_dn TEXT NOT NULL,
    user_search_filter VARCHAR(512) NOT NULL DEFAULT '(uid={username})',
    username_attr VARCHAR(50) NOT NULL DEFAULT 'uid',
    email_attr VARCHAR(50) NOT NULL DEFAULT 'mail',
    display_name_attr VARCHAR(50) NOT NULL DEFAULT 'cn',
    is_enabled TINYINT(1) NOT NULL DEFAULT 0,
    is_exclusive TINYINT(1) NOT NULL DEFAULT 0,
    use_starttls TINYINT(1) NOT NULL DEFAULT 0,
    connect_timeout INT NOT NULL DEFAULT 10,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS user_oauth_links (
    id VARCHAR(64) PRIMARY KEY,
    user_id VARCHAR(64) NOT NULL,
    provider_type VARCHAR(64) NOT NULL,
    provider_user_id VARCHAR(255) NOT NULL,
    provider_username VARCHAR(255),
    provider_email VARCHAR(255),
    extra_data TEXT,
    linked_at BIGINT NOT NULL,
    last_login_at BIGINT,
    UNIQUE KEY uq_user_oauth_links_provider_user (provider_type, provider_user_id),
    UNIQUE KEY uq_user_oauth_links_user_provider (user_id, provider_type),
    KEY user_oauth_links_provider_type_idx (provider_type),
    KEY user_oauth_links_user_id_idx (user_id)
);
