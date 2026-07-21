ALTER TABLE public.proxy_node_events
    ADD COLUMN IF NOT EXISTS event_metadata json;

CREATE TABLE IF NOT EXISTS public.proxy_node_metrics_1m (
    node_id character varying(36) NOT NULL,
    bucket_start_unix_secs bigint NOT NULL,
    samples bigint DEFAULT 0 NOT NULL,
    uptime_samples bigint DEFAULT 0 NOT NULL,
    active_connections_sum bigint DEFAULT 0 NOT NULL,
    active_connections_max bigint DEFAULT 0 NOT NULL,
    heartbeat_rtt_ms_sum bigint DEFAULT 0 NOT NULL,
    heartbeat_rtt_ms_max bigint DEFAULT 0 NOT NULL,
    connect_errors_delta bigint DEFAULT 0 NOT NULL,
    disconnects_delta bigint DEFAULT 0 NOT NULL,
    error_events_delta bigint DEFAULT 0 NOT NULL,
    ws_in_bytes_delta bigint DEFAULT 0 NOT NULL,
    ws_out_bytes_delta bigint DEFAULT 0 NOT NULL,
    ws_in_frames_delta bigint DEFAULT 0 NOT NULL,
    ws_out_frames_delta bigint DEFAULT 0 NOT NULL,
    PRIMARY KEY (node_id, bucket_start_unix_secs),
    CONSTRAINT proxy_node_metrics_1m_node_id_fkey
        FOREIGN KEY (node_id) REFERENCES public.proxy_nodes(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS public.proxy_node_metrics_1h (
    node_id character varying(36) NOT NULL,
    bucket_start_unix_secs bigint NOT NULL,
    samples bigint DEFAULT 0 NOT NULL,
    uptime_samples bigint DEFAULT 0 NOT NULL,
    active_connections_sum bigint DEFAULT 0 NOT NULL,
    active_connections_max bigint DEFAULT 0 NOT NULL,
    heartbeat_rtt_ms_sum bigint DEFAULT 0 NOT NULL,
    heartbeat_rtt_ms_max bigint DEFAULT 0 NOT NULL,
    connect_errors_delta bigint DEFAULT 0 NOT NULL,
    disconnects_delta bigint DEFAULT 0 NOT NULL,
    error_events_delta bigint DEFAULT 0 NOT NULL,
    ws_in_bytes_delta bigint DEFAULT 0 NOT NULL,
    ws_out_bytes_delta bigint DEFAULT 0 NOT NULL,
    ws_in_frames_delta bigint DEFAULT 0 NOT NULL,
    ws_out_frames_delta bigint DEFAULT 0 NOT NULL,
    PRIMARY KEY (node_id, bucket_start_unix_secs),
    CONSTRAINT proxy_node_metrics_1h_node_id_fkey
        FOREIGN KEY (node_id) REFERENCES public.proxy_nodes(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_proxy_node_metrics_1m_bucket_start
    ON public.proxy_node_metrics_1m (bucket_start_unix_secs);

CREATE INDEX IF NOT EXISTS idx_proxy_node_metrics_1h_bucket_start
    ON public.proxy_node_metrics_1h (bucket_start_unix_secs);
