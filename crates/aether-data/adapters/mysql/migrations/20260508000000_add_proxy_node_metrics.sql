ALTER TABLE proxy_node_events
    ADD COLUMN event_metadata TEXT NULL AFTER detail;

CREATE TABLE IF NOT EXISTS proxy_node_metrics_1m (
    node_id VARCHAR(64) NOT NULL,
    bucket_start_unix_secs BIGINT NOT NULL,
    samples BIGINT NOT NULL DEFAULT 0,
    uptime_samples BIGINT NOT NULL DEFAULT 0,
    active_connections_sum BIGINT NOT NULL DEFAULT 0,
    active_connections_max BIGINT NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_sum BIGINT NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_max BIGINT NOT NULL DEFAULT 0,
    connect_errors_delta BIGINT NOT NULL DEFAULT 0,
    disconnects_delta BIGINT NOT NULL DEFAULT 0,
    error_events_delta BIGINT NOT NULL DEFAULT 0,
    ws_in_bytes_delta BIGINT NOT NULL DEFAULT 0,
    ws_out_bytes_delta BIGINT NOT NULL DEFAULT 0,
    ws_in_frames_delta BIGINT NOT NULL DEFAULT 0,
    ws_out_frames_delta BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (node_id, bucket_start_unix_secs),
    INDEX idx_proxy_node_metrics_1m_bucket_start (bucket_start_unix_secs)
);

CREATE TABLE IF NOT EXISTS proxy_node_metrics_1h (
    node_id VARCHAR(64) NOT NULL,
    bucket_start_unix_secs BIGINT NOT NULL,
    samples BIGINT NOT NULL DEFAULT 0,
    uptime_samples BIGINT NOT NULL DEFAULT 0,
    active_connections_sum BIGINT NOT NULL DEFAULT 0,
    active_connections_max BIGINT NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_sum BIGINT NOT NULL DEFAULT 0,
    heartbeat_rtt_ms_max BIGINT NOT NULL DEFAULT 0,
    connect_errors_delta BIGINT NOT NULL DEFAULT 0,
    disconnects_delta BIGINT NOT NULL DEFAULT 0,
    error_events_delta BIGINT NOT NULL DEFAULT 0,
    ws_in_bytes_delta BIGINT NOT NULL DEFAULT 0,
    ws_out_bytes_delta BIGINT NOT NULL DEFAULT 0,
    ws_in_frames_delta BIGINT NOT NULL DEFAULT 0,
    ws_out_frames_delta BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (node_id, bucket_start_unix_secs),
    INDEX idx_proxy_node_metrics_1h_bucket_start (bucket_start_unix_secs)
);
