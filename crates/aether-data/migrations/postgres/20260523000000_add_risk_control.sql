CREATE TABLE IF NOT EXISTS public.risk_control_logs (
    id character varying(36) NOT NULL,
    trace_id character varying(100) NOT NULL,
    request_id character varying(100),
    user_id character varying(36),
    username character varying(100),
    user_email character varying(255),
    api_key_id character varying(36),
    api_key_name character varying(255),
    route_family character varying(80),
    route_kind character varying(80),
    api_format character varying(80),
    endpoint character varying(255) NOT NULL,
    model character varying(255),
    mode character varying(32) NOT NULL,
    action character varying(32) NOT NULL,
    decision_source character varying(32) NOT NULL,
    flagged boolean DEFAULT false NOT NULL,
    highest_category character varying(100),
    highest_score double precision DEFAULT 0 NOT NULL,
    category_scores json,
    thresholds json,
    matched_keywords json,
    input_hash character varying(64),
    excerpt text,
    latency_ms bigint,
    queue_delay_ms bigint,
    violation_count bigint DEFAULT 0 NOT NULL,
    auto_action character varying(64),
    notification_sent boolean DEFAULT false NOT NULL,
    error_message text,
    created_at bigint NOT NULL,
    CONSTRAINT risk_control_logs_pkey PRIMARY KEY (id)
);

CREATE TABLE IF NOT EXISTS public.risk_control_flagged_hashes (
    input_hash character varying(64) NOT NULL,
    source_log_id character varying(36),
    reason character varying(255),
    highest_category character varying(100),
    highest_score double precision DEFAULT 0 NOT NULL,
    excerpt text,
    first_seen_at bigint NOT NULL,
    last_seen_at bigint NOT NULL,
    hit_count bigint DEFAULT 1 NOT NULL,
    CONSTRAINT risk_control_flagged_hashes_pkey PRIMARY KEY (input_hash)
);

CREATE INDEX IF NOT EXISTS idx_risk_control_logs_created_at ON public.risk_control_logs USING btree (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_flagged_created_at ON public.risk_control_logs USING btree (flagged, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_user_created_at ON public.risk_control_logs USING btree (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_api_key_created_at ON public.risk_control_logs USING btree (api_key_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_endpoint_created_at ON public.risk_control_logs USING btree (endpoint, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_risk_control_logs_input_hash ON public.risk_control_logs USING btree (input_hash);
CREATE INDEX IF NOT EXISTS idx_risk_control_hashes_last_seen_at ON public.risk_control_flagged_hashes USING btree (last_seen_at DESC);

INSERT INTO public.system_configs (id, key, value, description, created_at, updated_at)
VALUES
    ('risk-control-enabled', 'module.risk_control.enabled', 'false'::json, '风控中心总开关', NOW(), NOW()),
    ('risk-control-config', 'module.risk_control.config', '{}'::json, '风控中心配置', NOW(), NOW())
ON CONFLICT (key) DO NOTHING;
