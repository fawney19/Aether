CREATE TABLE IF NOT EXISTS proxy_nodes (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    ip TEXT NOT NULL,
    port INTEGER NOT NULL,
    region TEXT,
    status TEXT NOT NULL DEFAULT 'online',
    registered_by TEXT,
    last_heartbeat_at INTEGER,
    heartbeat_interval INTEGER NOT NULL DEFAULT 30,
    active_connections INTEGER NOT NULL DEFAULT 0,
    total_requests INTEGER NOT NULL DEFAULT 0,
    avg_latency_ms REAL,
    is_manual INTEGER NOT NULL DEFAULT 0,
    proxy_url TEXT,
    proxy_username TEXT,
    proxy_password TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    remote_config TEXT,
    config_version INTEGER NOT NULL DEFAULT 0,
    hardware_info TEXT,
    estimated_max_concurrency INTEGER,
    tunnel_mode INTEGER NOT NULL DEFAULT 0,
    tunnel_connected INTEGER NOT NULL DEFAULT 0,
    tunnel_connected_at INTEGER,
    failed_requests INTEGER NOT NULL DEFAULT 0,
    dns_failures INTEGER NOT NULL DEFAULT 0,
    stream_errors INTEGER NOT NULL DEFAULT 0,
    proxy_metadata TEXT
);

CREATE TABLE IF NOT EXISTS proxy_node_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    detail TEXT,
    created_at INTEGER NOT NULL
);
