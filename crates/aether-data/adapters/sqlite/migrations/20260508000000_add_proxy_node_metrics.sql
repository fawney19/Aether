ALTER TABLE proxy_node_events
    ADD COLUMN event_metadata TEXT;

CREATE TABLE IF NOT EXISTS proxy_node_metrics_1m (
    node_id TEXT NOT NULL,
    bucket_start_unix_secs INTEGER NOT NULL,
    samples INTEGER NOT NULL DEFAULT 0,
    uptime_samples INTEGER NOT NULL DEFAULT 0,
    active_connections_sum INTEGER NOT NULL DEFAULT 0,
    active_connections_max INTEGER NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_sum INTEGER NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_max INTEGER NOT NULL DEFAULT 0,
    connect_errors_delta INTEGER NOT NULL DEFAULT 0,
    disconnects_delta INTEGER NOT NULL DEFAULT 0,
    error_events_delta INTEGER NOT NULL DEFAULT 0,
    ws_in_bytes_delta INTEGER NOT NULL DEFAULT 0,
    ws_out_bytes_delta INTEGER NOT NULL DEFAULT 0,
    ws_in_frames_delta INTEGER NOT NULL DEFAULT 0,
    ws_out_frames_delta INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (node_id, bucket_start_unix_secs)
);

CREATE TABLE IF NOT EXISTS proxy_node_metrics_1h (
    node_id TEXT NOT NULL,
    bucket_start_unix_secs INTEGER NOT NULL,
    samples INTEGER NOT NULL DEFAULT 0,
    uptime_samples INTEGER NOT NULL DEFAULT 0,
    active_connections_sum INTEGER NOT NULL DEFAULT 0,
    active_connections_max INTEGER NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_sum INTEGER NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_max INTEGER NOT NULL DEFAULT 0,
    connect_errors_delta INTEGER NOT NULL DEFAULT 0,
    disconnects_delta INTEGER NOT NULL DEFAULT 0,
    error_events_delta INTEGER NOT NULL DEFAULT 0,
    ws_in_bytes_delta INTEGER NOT NULL DEFAULT 0,
    ws_out_bytes_delta INTEGER NOT NULL DEFAULT 0,
    ws_in_frames_delta INTEGER NOT NULL DEFAULT 0,
    ws_out_frames_delta INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (node_id, bucket_start_unix_secs)
);

CREATE INDEX IF NOT EXISTS idx_proxy_node_metrics_1m_bucket_start
    ON proxy_node_metrics_1m (bucket_start_unix_secs);

CREATE INDEX IF NOT EXISTS idx_proxy_node_metrics_1h_bucket_start
    ON proxy_node_metrics_1h (bucket_start_unix_secs);
