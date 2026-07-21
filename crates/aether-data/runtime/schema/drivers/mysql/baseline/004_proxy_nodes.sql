CREATE TABLE IF NOT EXISTS proxy_nodes (
    id VARCHAR(64) PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    ip VARCHAR(512) NOT NULL,
    port INT NOT NULL,
    region VARCHAR(100),
    status VARCHAR(32) NOT NULL DEFAULT 'online',
    registered_by VARCHAR(64),
    last_heartbeat_at BIGINT,
    heartbeat_interval INT NOT NULL DEFAULT 30,
    active_connections INT NOT NULL DEFAULT 0,
    total_requests BIGINT NOT NULL DEFAULT 0,
    avg_latency_ms DOUBLE,
    is_manual TINYINT(1) NOT NULL DEFAULT 0,
    proxy_url VARCHAR(500),
    proxy_username VARCHAR(255),
    proxy_password VARCHAR(500),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    remote_config TEXT,
    config_version INT NOT NULL DEFAULT 0,
    hardware_info TEXT,
    estimated_max_concurrency INT,
    tunnel_mode TINYINT(1) NOT NULL DEFAULT 0,
    tunnel_connected TINYINT(1) NOT NULL DEFAULT 0,
    tunnel_connected_at BIGINT,
    failed_requests BIGINT NOT NULL DEFAULT 0,
    dns_failures BIGINT NOT NULL DEFAULT 0,
    stream_errors BIGINT NOT NULL DEFAULT 0,
    proxy_metadata TEXT
);

CREATE TABLE IF NOT EXISTS proxy_node_events (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    node_id VARCHAR(64) NOT NULL,
    event_type VARCHAR(64) NOT NULL,
    detail VARCHAR(500),
    created_at BIGINT NOT NULL
);
