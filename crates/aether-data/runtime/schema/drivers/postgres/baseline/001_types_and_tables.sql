-- Idempotent schema initial migration.
--
-- Purpose:
--   Build the complete public schema from scratch for newly provisioned
--   databases. This migration is fully idempotent: every CREATE/ALTER is
--   guarded, so existing databases that already carry the full schema
--   (upgraded from the Python/alembic era) will simply no-op on each
--   statement and this migration gets recorded as applied.
--
-- Source:
--   Generated from `pg_dump --schema-only` of the known-good production
--   schema (matching alembic revision c3d4e5f6a7b8) and then mechanically
--   rewritten to be idempotent via DO blocks that swallow duplicate_object
--   / duplicate_table exceptions. See scripts/dev/ (if present) or the
--   commit that introduced this file for the generator script.
--
-- Ordering:
--   Runs before 20260403000000_baseline.sql so that fresh databases have a
--   complete schema by the time baseline (a no-op handoff point) and all
--   later ADD COLUMN IF NOT EXISTS migrations execute.
SET statement_timeout = 0;

SET lock_timeout = 0;

SET idle_in_transaction_session_timeout = 0;

SET client_encoding = 'UTF8';

SET standard_conforming_strings = on;

SELECT pg_catalog.set_config('search_path', '', true);

SET check_function_bodies = false;

SET xmloption = content;

SET client_min_messages = warning;

SET row_security = off;


--
-- Name: authsource; Type: TYPE; Schema: public; Owner: -
--

DO $mig$ BEGIN
  CREATE TYPE public.authsource AS ENUM (
    'local',
    'ldap',
    'oauth'
);
EXCEPTION WHEN duplicate_object THEN NULL;
END $mig$;



--
-- Name: providerbillingtype; Type: TYPE; Schema: public; Owner: -
--

DO $mig$ BEGIN
  CREATE TYPE public.providerbillingtype AS ENUM (
    'monthly_quota',
    'pay_as_you_go',
    'free_tier'
);
EXCEPTION WHEN duplicate_object THEN NULL;
END $mig$;



--
-- Name: proxynodestatus; Type: TYPE; Schema: public; Owner: -
--

DO $mig$ BEGIN
  CREATE TYPE public.proxynodestatus AS ENUM (
    'online',
    'unhealthy',
    'offline'
);
EXCEPTION WHEN duplicate_object THEN NULL;
END $mig$;



--
-- Name: userrole; Type: TYPE; Schema: public; Owner: -
--

DO $mig$ BEGIN
  CREATE TYPE public.userrole AS ENUM (
    'admin',
    'user'
);
EXCEPTION WHEN duplicate_object THEN NULL;
END $mig$;



SET default_tablespace = '';


SET default_table_access_method = heap;


--
-- Name: announcement_reads; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.announcement_reads (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    announcement_id character varying(36) NOT NULL,
    read_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: announcements; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.announcements (
    id character varying(36) NOT NULL,
    title character varying(200) NOT NULL,
    content text NOT NULL,
    type character varying(20) DEFAULT 'info'::character varying,
    priority integer DEFAULT 0,
    author_id character varying(36),
    is_active boolean DEFAULT true,
    is_pinned boolean DEFAULT false,
    start_time timestamp with time zone,
    end_time timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: api_key_provider_mappings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.api_key_provider_mappings (
    id character varying(36) NOT NULL,
    api_key_id character varying(36) NOT NULL,
    provider_id character varying(36) NOT NULL,
    priority_adjustment integer DEFAULT 0,
    weight_multiplier double precision DEFAULT '1'::double precision,
    is_enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: api_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.api_keys (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    key_hash character varying(64) NOT NULL,
    key_encrypted text,
    name character varying(100),
    key_prefix character varying(64),
    status character varying(64) DEFAULT 'active'::character varying NOT NULL,
    total_requests integer DEFAULT 0,
    total_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    is_standalone boolean DEFAULT false NOT NULL,
    allowed_providers json,
    allowed_api_formats json,
    allowed_models json,
    rate_limit integer DEFAULT 100,
    concurrent_limit integer,
    force_capabilities json,
    is_active boolean DEFAULT true NOT NULL,
    last_used_at timestamp with time zone,
    expires_at timestamp with time zone,
    auto_delete_on_expiry boolean DEFAULT false NOT NULL,
    metadata json,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_locked boolean DEFAULT false NOT NULL,
    CONSTRAINT ck_api_keys_standalone_not_locked CHECK (((NOT is_standalone) OR (NOT is_locked)))
);



--
-- Name: audit_logs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.audit_logs (
    id character varying(36) NOT NULL,
    event_type character varying(50) NOT NULL,
    user_id character varying(36),
    api_key_id character varying(36),
    description text NOT NULL,
    ip_address character varying(45),
    user_agent character varying(500),
    request_id character varying(100),
    event_metadata json,
    status_code integer,
    error_message text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: billing_rules; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.billing_rules (
    id character varying(36) NOT NULL,
    global_model_id character varying(36),
    model_id character varying(36),
    name character varying(100) NOT NULL,
    task_type character varying(20) DEFAULT 'chat'::character varying NOT NULL,
    expression text NOT NULL,
    variables jsonb DEFAULT '{}'::jsonb NOT NULL,
    dimension_mappings jsonb DEFAULT '{}'::jsonb NOT NULL,
    is_enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_billing_rules_model_ref CHECK ((((global_model_id IS NOT NULL) AND (model_id IS NULL)) OR ((global_model_id IS NULL) AND (model_id IS NOT NULL))))
);



--
-- Name: dimension_collectors; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.dimension_collectors (
    id character varying(36) NOT NULL,
    api_format character varying(50) NOT NULL,
    task_type character varying(20) NOT NULL,
    dimension_name character varying(100) NOT NULL,
    source_type character varying(20) NOT NULL,
    source_path character varying(200),
    value_type character varying(20) DEFAULT 'float'::character varying NOT NULL,
    transform_expression text,
    default_value character varying(100),
    priority integer DEFAULT 0 NOT NULL,
    is_enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_dimension_collectors_source_config CHECK (((((source_type)::text = 'computed'::text) AND (source_path IS NULL) AND (transform_expression IS NOT NULL)) OR (((source_type)::text <> 'computed'::text) AND (source_path IS NOT NULL))))
);



--
-- Name: gemini_file_mappings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.gemini_file_mappings (
    id character varying(36) NOT NULL,
    file_name character varying(255) NOT NULL,
    key_id character varying(36) NOT NULL,
    user_id character varying(36),
    display_name character varying(255),
    mime_type character varying(100),
    source_hash character varying(64),
    created_at timestamp with time zone NOT NULL,
    expires_at timestamp with time zone NOT NULL
);



--
-- Name: global_models; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.global_models (
    id character varying(36) NOT NULL,
    name character varying(100) NOT NULL,
    display_name character varying(100),
    enabled boolean DEFAULT true NOT NULL,
    default_price_per_request numeric(20,8),
    default_tiered_pricing json,
    supported_capabilities json,
    is_active boolean DEFAULT true NOT NULL,
    usage_count integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    config jsonb,
    metadata json
);



--
-- Name: ldap_configs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.ldap_configs (
    id integer NOT NULL,
    server_url character varying(255) NOT NULL,
    bind_dn text NOT NULL,
    bind_password_encrypted text,
    base_dn text NOT NULL,
    user_search_filter text DEFAULT '(uid={username})'::character varying NOT NULL,
    username_attr character varying(50) DEFAULT 'uid'::character varying NOT NULL,
    email_attr character varying(50) DEFAULT 'mail'::character varying NOT NULL,
    display_name_attr character varying(50) DEFAULT 'cn'::character varying NOT NULL,
    is_enabled boolean DEFAULT false NOT NULL,
    is_exclusive boolean DEFAULT false NOT NULL,
    use_starttls boolean DEFAULT false NOT NULL,
    connect_timeout integer DEFAULT 10 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: ldap_configs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.ldap_configs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



--
-- Name: ldap_configs_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.ldap_configs_id_seq OWNED BY public.ldap_configs.id;



--
-- Name: management_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.management_tokens (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    token_hash character varying(64) NOT NULL,
    token_prefix character varying(12),
    name character varying(100) NOT NULL,
    description text,
    allowed_ips jsonb,
    permissions jsonb,
    expires_at timestamp with time zone,
    last_used_at timestamp with time zone,
    last_used_ip character varying(45),
    usage_count integer DEFAULT 0 NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT check_allowed_ips_not_empty CHECK (CASE WHEN ((allowed_ips IS NULL) OR (allowed_ips = 'null'::jsonb)) THEN true WHEN (jsonb_typeof(allowed_ips) = 'array'::text) THEN (jsonb_array_length(allowed_ips) > 0) ELSE false END)
);



--
-- Name: models; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.models (
    id character varying(36) NOT NULL,
    provider_id character varying(36) NOT NULL,
    global_model_id character varying(36),
    provider_model_name character varying(200) NOT NULL,
    global_model_name character varying(255),
    api_format character varying(128),
    enabled boolean DEFAULT true NOT NULL,
    price_per_request numeric(20,8),
    tiered_pricing json,
    supports_vision boolean,
    supports_function_calling boolean,
    supports_streaming boolean,
    supports_extended_thinking boolean,
    supports_image_generation boolean,
    is_active boolean DEFAULT true NOT NULL,
    is_available boolean DEFAULT true,
    config json,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    provider_model_mappings jsonb,
    metadata json
);



--
-- Name: oauth_providers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.oauth_providers (
    provider_type character varying(50) NOT NULL,
    display_name character varying(100) NOT NULL,
    client_id text NOT NULL,
    client_secret_encrypted text,
    authorization_url_override character varying(500),
    token_url_override character varying(500),
    userinfo_url_override character varying(500),
    scopes json,
    redirect_uri character varying(500) NOT NULL,
    frontend_callback_url character varying(500) NOT NULL,
    attribute_mapping json,
    extra_config json,
    is_enabled boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);



--
-- Name: payment_callbacks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.payment_callbacks (
    id character varying(36) NOT NULL,
    payment_order_id character varying(36),
    payment_method character varying(30) NOT NULL,
    callback_key character varying(128) NOT NULL,
    order_no character varying(64),
    gateway_order_id character varying(128),
    payload_hash character varying(128),
    signature_valid boolean DEFAULT false NOT NULL,
    status character varying(20) DEFAULT 'received'::character varying NOT NULL,
    payload jsonb,
    error_message text,
    created_at timestamp with time zone NOT NULL,
    processed_at timestamp with time zone
);



--
-- Name: payment_orders; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.payment_orders (
    id character varying(36) NOT NULL,
    order_no character varying(64) NOT NULL,
    wallet_id character varying(36) NOT NULL,
    user_id character varying(36),
    amount_usd numeric(20,8) NOT NULL,
    pay_amount numeric(20,2),
    pay_currency character varying(3),
    exchange_rate numeric(18,8),
    refunded_amount_usd numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    refundable_amount_usd numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    payment_method character varying(30) NOT NULL,
    gateway_order_id character varying(128),
    gateway_response jsonb,
    status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    created_at timestamp with time zone NOT NULL,
    paid_at timestamp with time zone,
    credited_at timestamp with time zone,
    expires_at timestamp with time zone
);



--
-- Name: provider_api_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.provider_api_keys (
    id character varying(36) NOT NULL,
    api_key text,
    encrypted_key text,
    name character varying(100) NOT NULL,
    note character varying(500),
    internal_priority integer DEFAULT 50,
    rpm_limit integer,
    allowed_models json,
    capabilities json,
    learned_rpm_limit integer,
    concurrent_429_count integer DEFAULT 0 NOT NULL,
    rpm_429_count integer DEFAULT 0 NOT NULL,
    last_429_at timestamp with time zone,
    last_429_type character varying(50),
    last_rpm_peak integer,
    adjustment_history json,
    utilization_samples json,
    last_probe_increase_at timestamp with time zone,
    cache_ttl_minutes integer DEFAULT 5 NOT NULL,
    max_probe_interval_minutes integer DEFAULT 32 NOT NULL,
    request_count integer DEFAULT 0,
    success_count integer DEFAULT 0,
    error_count integer DEFAULT 0,
    total_response_time_ms integer DEFAULT 0,
    last_used_at timestamp with time zone,
    last_error_at timestamp with time zone,
    last_error_msg text,
    is_active boolean DEFAULT true NOT NULL,
    expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    provider_id character varying(36) NOT NULL,
    api_formats json,
    auth_type_by_format json,
    rate_multipliers json,
    health_by_format jsonb,
    circuit_breaker_by_format jsonb,
    auto_fetch_models boolean DEFAULT false NOT NULL,
    last_models_fetch_at timestamp with time zone,
    last_models_fetch_error text,
    locked_models json,
    global_priority_by_format json,
    model_include_patterns json,
    model_exclude_patterns json,
    auth_type character varying(20) DEFAULT 'api_key'::character varying NOT NULL,
    auth_config text,
    upstream_metadata jsonb,
    oauth_invalid_at timestamp with time zone,
    oauth_invalid_reason character varying(255),
    proxy json,
    fingerprint json,
    total_tokens bigint NOT NULL,
    total_cost_usd numeric(20,8) NOT NULL,
    status_snapshot json,
    status character varying(64) DEFAULT 'active'::character varying NOT NULL,
    weight bigint DEFAULT 1 NOT NULL,
    metadata json
);



--
-- Name: pool_member_scores; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.pool_member_scores (
    id character varying(192) NOT NULL,
    pool_kind character varying(64) NOT NULL,
    pool_id character varying(64) NOT NULL,
    member_kind character varying(64) NOT NULL,
    member_id character varying(64) NOT NULL,
    capability character varying(64) NOT NULL,
    scope_kind character varying(64) NOT NULL,
    scope_id character varying(128),
    score double precision DEFAULT 0 NOT NULL,
    hard_state character varying(64) DEFAULT 'unknown'::character varying NOT NULL,
    score_version bigint DEFAULT 1 NOT NULL,
    score_reason jsonb NOT NULL,
    last_ranked_at bigint,
    last_scheduled_at bigint,
    last_success_at bigint,
    last_failure_at bigint,
    failure_count bigint DEFAULT 0 NOT NULL,
    last_probe_attempt_at bigint,
    last_probe_success_at bigint,
    last_probe_failure_at bigint,
    probe_failure_count bigint DEFAULT 0 NOT NULL,
    probe_status character varying(64) DEFAULT 'never'::character varying NOT NULL,
    updated_at bigint NOT NULL
);



--
-- Name: provider_endpoints; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.provider_endpoints (
    id character varying(36) NOT NULL,
    provider_id character varying(36) NOT NULL,
    name character varying(255),
    api_format character varying(50),
    base_url character varying(500) NOT NULL,
    max_retries integer DEFAULT 3,
    enabled boolean DEFAULT true NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    weight bigint DEFAULT 1 NOT NULL,
    custom_path character varying(200),
    config json,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    proxy jsonb,
    header_rules json,
    format_acceptance_config json,
    api_family character varying(50),
    endpoint_kind character varying(50),
    body_rules json,
    health_score double precision DEFAULT 1.0 NOT NULL,
    metadata json
);



--
-- Name: provider_usage_tracking; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.provider_usage_tracking (
    id character varying(36) NOT NULL,
    provider_id character varying(36) NOT NULL,
    window_start timestamp with time zone NOT NULL,
    window_end timestamp with time zone NOT NULL,
    total_requests integer DEFAULT 0,
    successful_requests integer DEFAULT 0,
    failed_requests integer DEFAULT 0,
    avg_response_time_ms double precision DEFAULT '0'::double precision,
    total_response_time_ms double precision DEFAULT '0'::double precision,
    total_cost_usd double precision DEFAULT '0'::double precision,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: providers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.providers (
    id character varying(36) NOT NULL,
    name character varying(100) NOT NULL,
    description text,
    website character varying(500),
    billing_type public.providerbillingtype DEFAULT 'pay_as_you_go'::public.providerbillingtype NOT NULL,
    monthly_quota_usd numeric(20,8),
    monthly_used_usd numeric(20,8) DEFAULT '0'::double precision,
    quota_reset_day integer DEFAULT 30,
    quota_last_reset_at timestamp with time zone,
    quota_expires_at timestamp with time zone,
    enabled boolean DEFAULT true NOT NULL,
    priority bigint DEFAULT 0 NOT NULL,
    provider_priority integer DEFAULT 100,
    is_active boolean DEFAULT true NOT NULL,
    concurrent_limit integer,
    config json,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    max_retries integer,
    proxy jsonb,
    stream_first_byte_timeout double precision,
    request_timeout double precision,
    keep_priority_on_conversion boolean DEFAULT false NOT NULL,
    enable_format_conversion boolean DEFAULT true NOT NULL,
    provider_type character varying(20) DEFAULT 'custom'::character varying NOT NULL
);



--
-- Name: proxy_node_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.proxy_node_events (
    id bigint NOT NULL,
    node_id character varying(36) NOT NULL,
    event_type character varying(20) NOT NULL,
    detail character varying(500),
    created_at timestamp with time zone NOT NULL
);



--
-- Name: proxy_node_events_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE IF NOT EXISTS public.proxy_node_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



--
-- Name: proxy_node_events_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.proxy_node_events_id_seq OWNED BY public.proxy_node_events.id;



--
-- Name: proxy_nodes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.proxy_nodes (
    id character varying(36) NOT NULL,
    name character varying(100) NOT NULL,
    ip character varying(512) NOT NULL,
    port integer NOT NULL,
    region character varying(100),
    status public.proxynodestatus DEFAULT 'online'::public.proxynodestatus NOT NULL,
    registered_by character varying(36),
    last_heartbeat_at timestamp with time zone,
    heartbeat_interval integer DEFAULT 30 NOT NULL,
    active_connections integer DEFAULT 0 NOT NULL,
    total_requests bigint DEFAULT 0 NOT NULL,
    avg_latency_ms double precision,
    is_manual boolean DEFAULT false NOT NULL,
    proxy_url character varying(500),
    proxy_username character varying(255),
    proxy_password character varying(500),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    remote_config json,
    config_version integer DEFAULT 0 NOT NULL,
    hardware_info json,
    estimated_max_concurrency integer,
    tunnel_mode boolean DEFAULT false NOT NULL,
    tunnel_connected boolean DEFAULT false NOT NULL,
    tunnel_connected_at timestamp with time zone,
    failed_requests bigint DEFAULT '0'::bigint NOT NULL,
    dns_failures bigint DEFAULT '0'::bigint NOT NULL,
    stream_errors bigint DEFAULT '0'::bigint NOT NULL,
    proxy_metadata json
);



--
-- Name: refund_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.refund_requests (
    id character varying(36) NOT NULL,
    refund_no character varying(64) NOT NULL,
    wallet_id character varying(36) NOT NULL,
    user_id character varying(36),
    payment_order_id character varying(36),
    source_type character varying(30) NOT NULL,
    source_id character varying(100),
    refund_mode character varying(30) NOT NULL,
    amount_usd numeric(20,8) NOT NULL,
    status character varying(30) DEFAULT 'pending_approval'::character varying NOT NULL,
    reason text,
    requested_by character varying(36),
    approved_by character varying(36),
    processed_by character varying(36),
    gateway_refund_id character varying(128),
    payout_method character varying(50),
    payout_reference character varying(255),
    payout_proof jsonb,
    failure_reason text,
    idempotency_key character varying(128),
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    processed_at timestamp with time zone,
    completed_at timestamp with time zone
);



--
-- Name: request_candidates; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.request_candidates (
    id character varying(36) NOT NULL,
    request_id character varying(100) NOT NULL,
    user_id character varying(36),
    api_key_id character varying(36),
    candidate_index integer NOT NULL,
    retry_index integer DEFAULT 0 NOT NULL,
    provider_id character varying(36),
    endpoint_id character varying(36),
    key_id character varying(36),
    status character varying(20) NOT NULL,
    skip_reason text,
    is_cached boolean DEFAULT false,
    status_code integer,
    error_type character varying(50),
    error_message text,
    latency_ms integer,
    concurrent_requests integer,
    extra_data json,
    required_capabilities json,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    username character varying(100),
    api_key_name character varying(200)
);



--
-- Name: stats_daily; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_daily (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    success_requests integer DEFAULT 0 NOT NULL,
    error_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    actual_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    input_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    output_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    avg_response_time_ms double precision DEFAULT '0'::double precision NOT NULL,
    fallback_count integer DEFAULT 0 NOT NULL,
    unique_models integer DEFAULT 0 NOT NULL,
    unique_providers integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    is_complete boolean DEFAULT false NOT NULL,
    aggregated_at timestamp with time zone,
    p50_response_time_ms integer,
    p90_response_time_ms integer,
    p99_response_time_ms integer,
    p50_first_byte_time_ms integer,
    p90_first_byte_time_ms integer,
    p99_first_byte_time_ms integer
);



--
-- Name: stats_daily_api_key; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_daily_api_key (
    id character varying(36) NOT NULL,
    api_key_id character varying(36),
    date timestamp with time zone NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    success_requests integer DEFAULT 0 NOT NULL,
    error_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    api_key_name character varying(200)
);



--
-- Name: stats_daily_error; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_daily_error (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    error_category character varying(50) NOT NULL,
    provider_name character varying(100),
    model character varying(100),
    count integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);



--
-- Name: stats_daily_model; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_daily_model (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    total_requests integer NOT NULL,
    input_tokens bigint NOT NULL,
    output_tokens bigint NOT NULL,
    cache_creation_tokens bigint NOT NULL,
    cache_read_tokens bigint NOT NULL,
    total_cost numeric(20,8) NOT NULL,
    avg_response_time_ms double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: stats_daily_provider; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_daily_provider (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    provider_name character varying(100) NOT NULL,
    total_requests integer NOT NULL,
    input_tokens bigint NOT NULL,
    output_tokens bigint NOT NULL,
    cache_creation_tokens bigint NOT NULL,
    cache_read_tokens bigint NOT NULL,
    total_cost numeric(20,8) NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);



--
-- Name: stats_hourly; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_hourly (
    id character varying(36) NOT NULL,
    hour_utc timestamp with time zone NOT NULL,
    total_requests integer NOT NULL,
    success_requests integer NOT NULL,
    error_requests integer NOT NULL,
    input_tokens bigint NOT NULL,
    output_tokens bigint NOT NULL,
    cache_creation_tokens bigint NOT NULL,
    cache_read_tokens bigint NOT NULL,
    total_cost numeric(20,8) NOT NULL,
    actual_total_cost numeric(20,8) NOT NULL,
    avg_response_time_ms double precision NOT NULL,
    is_complete boolean NOT NULL,
    aggregated_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);



--
-- Name: stats_hourly_model; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_hourly_model (
    id character varying(36) NOT NULL,
    hour_utc timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    total_requests integer NOT NULL,
    input_tokens bigint NOT NULL,
    output_tokens bigint NOT NULL,
    total_cost numeric(20,8) NOT NULL,
    avg_response_time_ms double precision NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);



--
-- Name: stats_hourly_provider; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_hourly_provider (
    id character varying(36) NOT NULL,
    hour_utc timestamp with time zone NOT NULL,
    provider_name character varying(100) NOT NULL,
    total_requests integer NOT NULL,
    input_tokens bigint NOT NULL,
    output_tokens bigint NOT NULL,
    total_cost numeric(20,8) NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);



--
-- Name: stats_hourly_user; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_hourly_user (
    id character varying(36) NOT NULL,
    hour_utc timestamp with time zone NOT NULL,
    user_id character varying(36) NOT NULL,
    total_requests integer NOT NULL,
    success_requests integer NOT NULL,
    error_requests integer NOT NULL,
    input_tokens bigint NOT NULL,
    output_tokens bigint NOT NULL,
    total_cost numeric(20,8) NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);



--
-- Name: stats_summary; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_summary (
    id character varying(36) NOT NULL,
    cutoff_date timestamp with time zone NOT NULL,
    all_time_requests integer DEFAULT 0 NOT NULL,
    all_time_success_requests integer DEFAULT 0 NOT NULL,
    all_time_error_requests integer DEFAULT 0 NOT NULL,
    all_time_input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    all_time_output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    all_time_cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    all_time_cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    all_time_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    all_time_actual_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    total_users integer DEFAULT 0 NOT NULL,
    active_users integer DEFAULT 0 NOT NULL,
    total_api_keys integer DEFAULT 0 NOT NULL,
    active_api_keys integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: stats_user_daily; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.stats_user_daily (
    id character varying(36) NOT NULL,
    user_id character varying(36),
    date timestamp with time zone NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    success_requests integer DEFAULT 0 NOT NULL,
    error_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    username character varying(100)
);



--
-- Name: system_configs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.system_configs (
    id character varying(36) NOT NULL,
    key character varying(100) NOT NULL,
    value json NOT NULL,
    description text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: auth_modules; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.auth_modules (
    id character varying(36) NOT NULL,
    module_type character varying(128) NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    config json NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT auth_modules_pkey PRIMARY KEY (id),
    CONSTRAINT auth_modules_module_type_key UNIQUE (module_type)
);



--
-- Name: usage; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.usage (
    id character varying(36) NOT NULL,
    user_id character varying(36),
    api_key_id character varying(36),
    request_id character varying(100) NOT NULL,
    provider_name character varying(100) NOT NULL,
    model character varying(100) NOT NULL,
    target_model character varying(100),
    provider_id character varying(36),
    provider_endpoint_id character varying(36),
    provider_api_key_id character varying(36),
    input_tokens integer DEFAULT 0,
    output_tokens integer DEFAULT 0,
    input_output_total_tokens integer DEFAULT 0,
    cache_creation_input_tokens integer DEFAULT 0,
    cache_read_input_tokens integer DEFAULT 0,
    input_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    output_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    cache_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    cache_creation_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    cache_read_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    request_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    total_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    actual_input_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    actual_output_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    actual_cache_creation_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    actual_cache_read_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    actual_request_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    actual_total_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    rate_multiplier numeric(10,6) DEFAULT '1'::double precision,
    input_price_per_1m numeric(20,8),
    output_price_per_1m numeric(20,8),
    cache_creation_price_per_1m numeric(20,8),
    cache_read_price_per_1m numeric(20,8),
    price_per_request numeric(20,8),
    request_type character varying(50),
    api_format character varying(50),
    is_stream boolean DEFAULT false,
    status_code integer,
    error_message text,
    response_time_ms integer,
    status character varying(20) DEFAULT 'completed'::character varying NOT NULL,
    request_headers json,
    request_body json,
    provider_request_headers json,
    response_headers json,
    response_body json,
    request_body_compressed bytea,
    response_body_compressed bytea,
    request_metadata json,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    first_byte_time_ms integer,
    client_response_headers json,
    endpoint_api_format character varying(50),
    has_format_conversion boolean DEFAULT false,
    billing_status character varying(20) DEFAULT 'pending'::character varying NOT NULL,
    finalized_at timestamp with time zone,
    error_category character varying(50),
    provider_request_body json,
    provider_request_body_compressed bytea,
    client_response_body json,
    client_response_body_compressed bytea,
    api_family character varying(50),
    endpoint_kind character varying(50),
    provider_api_family character varying(50),
    provider_endpoint_kind character varying(50),
    cache_creation_input_tokens_5m integer DEFAULT 0 NOT NULL,
    cache_creation_input_tokens_1h integer DEFAULT 0 NOT NULL,
    cache_creation_ephemeral_5m_input_tokens integer DEFAULT 0 NOT NULL,
    cache_creation_ephemeral_1h_input_tokens integer DEFAULT 0 NOT NULL,
    wallet_id character varying(36),
    wallet_balance_before numeric(20,8),
    wallet_balance_after numeric(20,8),
    wallet_recharge_balance_before numeric(20,8),
    wallet_recharge_balance_after numeric(20,8),
    wallet_gift_balance_before numeric(20,8),
    wallet_gift_balance_after numeric(20,8),
    username character varying(100),
    api_key_name character varying(200),
    input_context_tokens integer DEFAULT 0 NOT NULL,
    total_tokens integer DEFAULT 0 NOT NULL,
    cache_creation_cost_usd_5m numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    cache_creation_cost_usd_1h numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    actual_cache_creation_cost_usd_5m numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    actual_cache_creation_cost_usd_1h numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    actual_cache_cost_usd numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    cache_creation_price_per_1m_5m numeric(20,8),
    cache_creation_price_per_1m_1h numeric(20,8),
    created_at_unix_ms bigint DEFAULT 0 NOT NULL,
    updated_at_unix_secs bigint DEFAULT 0 NOT NULL
);



--
-- Name: user_model_usage_counts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.user_model_usage_counts (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    model character varying(100) NOT NULL,
    usage_count integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: user_oauth_links; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.user_oauth_links (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    provider_type character varying(50) NOT NULL,
    provider_user_id character varying(255) NOT NULL,
    provider_username character varying(255),
    provider_email character varying(255),
    extra_data json,
    linked_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_login_at timestamp with time zone
);



--
-- Name: user_preferences; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.user_preferences (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    avatar_url character varying(500),
    bio text,
    default_provider_id character varying(36),
    theme character varying(20) DEFAULT 'light'::character varying,
    language character varying(10) DEFAULT 'zh-CN'::character varying,
    timezone character varying(50) DEFAULT 'Asia/Shanghai'::character varying,
    email_notifications boolean DEFAULT true,
    usage_alerts boolean DEFAULT true,
    announcement_notifications boolean DEFAULT true,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: user_sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.user_sessions (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    client_device_id character varying(128) NOT NULL,
    device_label character varying(120),
    device_type character varying(20) DEFAULT 'unknown'::character varying NOT NULL,
    browser_name character varying(50),
    browser_version character varying(50),
    os_name character varying(50),
    os_version character varying(50),
    device_model character varying(100),
    ip_address character varying(45),
    user_agent character varying(1000),
    client_hints json,
    refresh_token_hash character varying(64) NOT NULL,
    prev_refresh_token_hash character varying(64),
    rotated_at timestamp with time zone,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    revoke_reason character varying(100),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);



--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.users (
    id character varying(36) NOT NULL,
    external_id character varying(255),
    email character varying(255),
    username character varying(100) NOT NULL,
    password_hash character varying(255),
    role public.userrole DEFAULT 'user'::public.userrole NOT NULL,
    allowed_providers json,
    allowed_api_formats json,
    allowed_models json,
    model_capability_settings json,
    is_active boolean DEFAULT true NOT NULL,
    is_deleted boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login_at timestamp with time zone,
    auth_source public.authsource DEFAULT 'local'::public.authsource NOT NULL,
    ldap_dn character varying(512),
    ldap_username character varying(255),
    email_verified boolean NOT NULL,
    rate_limit integer,
    metadata json
);



--
-- Name: video_tasks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.video_tasks (
    id character varying(36) NOT NULL,
    external_task_id character varying(200),
    user_id character varying(36),
    api_key_id character varying(36),
    provider_id character varying(36),
    endpoint_id character varying(36),
    key_id character varying(36),
    client_api_format character varying(50) NOT NULL,
    provider_api_format character varying(50) NOT NULL,
    format_converted boolean DEFAULT false,
    model character varying(100) NOT NULL,
    prompt text NOT NULL,
    original_request_body json,
    converted_request_body json,
    duration_seconds integer DEFAULT 4,
    resolution character varying(20) DEFAULT '720p'::character varying,
    aspect_ratio character varying(10) DEFAULT '16:9'::character varying,
    size character varying(20),
    status character varying(20) DEFAULT 'pending'::character varying,
    progress_percent integer DEFAULT 0,
    progress_message character varying(500),
    video_url character varying(2000),
    video_urls json,
    thumbnail_url character varying(2000),
    video_size_bytes bigint,
    video_expires_at timestamp with time zone,
    stored_video_path character varying(500),
    storage_provider character varying(50),
    error_code character varying(50),
    error_message text,
    retry_count integer DEFAULT 0,
    max_retries integer DEFAULT 3,
    poll_interval_seconds integer DEFAULT 10,
    next_poll_at timestamp with time zone,
    poll_count integer DEFAULT 0,
    max_poll_count integer DEFAULT 360,
    remixed_from_task_id character varying(36),
    webhook_url character varying(500),
    webhook_sent boolean DEFAULT false,
    webhook_sent_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    submitted_at timestamp with time zone,
    completed_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP,
    request_metadata json,
    request_id character varying(100) NOT NULL,
    short_id character varying(16) NOT NULL,
    video_duration_seconds double precision,
    username character varying(100),
    api_key_name character varying(200)
);



--
-- Name: wallet_daily_usage_ledgers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.wallet_daily_usage_ledgers (
    id character varying(36) NOT NULL,
    wallet_id character varying(36) NOT NULL,
    billing_date date NOT NULL,
    billing_timezone character varying(64) NOT NULL,
    total_cost_usd numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    first_finalized_at timestamp with time zone,
    last_finalized_at timestamp with time zone,
    aggregated_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);



--
-- Name: wallet_transactions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.wallet_transactions (
    id character varying(36) NOT NULL,
    wallet_id character varying(36) NOT NULL,
    category character varying(20) NOT NULL,
    reason_code character varying(40) NOT NULL,
    amount numeric(20,8) NOT NULL,
    balance_before numeric(20,8) NOT NULL,
    balance_after numeric(20,8) NOT NULL,
    recharge_balance_before numeric(20,8) NOT NULL,
    recharge_balance_after numeric(20,8) NOT NULL,
    gift_balance_before numeric(20,8) NOT NULL,
    gift_balance_after numeric(20,8) NOT NULL,
    link_type character varying(30),
    link_id character varying(100),
    operator_id character varying(36),
    description text,
    created_at timestamp with time zone NOT NULL,
    CONSTRAINT ck_wallet_tx_balance_after_consistent CHECK ((balance_after = (recharge_balance_after + gift_balance_after))),
    CONSTRAINT ck_wallet_tx_balance_before_consistent CHECK ((balance_before = (recharge_balance_before + gift_balance_before)))
);



--
-- Name: wallets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.wallets (
    id character varying(36) NOT NULL,
    user_id character varying(36),
    api_key_id character varying(36),
    balance numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    gift_balance numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    limit_mode character varying(20) DEFAULT 'finite'::character varying NOT NULL,
    currency character varying(3) DEFAULT 'USD'::character varying NOT NULL,
    status character varying(20) DEFAULT 'active'::character varying NOT NULL,
    total_recharged numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    total_consumed numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    total_refunded numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    total_adjusted numeric(20,8) DEFAULT '0'::numeric NOT NULL,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT ck_wallet_single_owner CHECK ((((user_id IS NOT NULL) AND (api_key_id IS NULL)) OR ((user_id IS NULL) AND (api_key_id IS NOT NULL)) OR ((user_id IS NULL) AND (api_key_id IS NULL)))),
    CONSTRAINT ck_wallets_gift_balance_non_negative CHECK ((gift_balance >= (0)::numeric))
);



--
