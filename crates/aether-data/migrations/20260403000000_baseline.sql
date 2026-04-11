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
    total_requests integer DEFAULT 0,
    total_cost_usd numeric(20,8) DEFAULT '0'::double precision,
    is_standalone boolean DEFAULT false NOT NULL,
    allowed_providers json,
    allowed_api_formats json,
    allowed_models json,
    rate_limit integer DEFAULT 100,
    concurrent_limit integer DEFAULT 5,
    force_capabilities json,
    is_active boolean DEFAULT true NOT NULL,
    last_used_at timestamp with time zone,
    expires_at timestamp with time zone,
    auto_delete_on_expiry boolean DEFAULT false NOT NULL,
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
    display_name character varying(100) NOT NULL,
    default_price_per_request numeric(20,8),
    default_tiered_pricing json NOT NULL,
    supported_capabilities json,
    is_active boolean DEFAULT true NOT NULL,
    usage_count integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    config jsonb
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
    allowed_ips json,
    expires_at timestamp with time zone,
    last_used_at timestamp with time zone,
    last_used_ip character varying(45),
    usage_count integer DEFAULT 0 NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT check_allowed_ips_not_empty CHECK (((allowed_ips IS NULL) OR ((allowed_ips)::text = 'null'::text) OR (json_array_length(allowed_ips) > 0)))
);



--
-- Name: models; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.models (
    id character varying(36) NOT NULL,
    provider_id character varying(36) NOT NULL,
    global_model_id character varying(36) NOT NULL,
    provider_model_name character varying(200) NOT NULL,
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
    provider_model_mappings jsonb
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
    api_key text NOT NULL,
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
    api_formats json DEFAULT '[]'::json NOT NULL,
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
    status_snapshot json
);



--
-- Name: provider_endpoints; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE IF NOT EXISTS public.provider_endpoints (
    id character varying(36) NOT NULL,
    provider_id character varying(36) NOT NULL,
    api_format character varying(50) NOT NULL,
    base_url character varying(500) NOT NULL,
    max_retries integer DEFAULT 3,
    is_active boolean DEFAULT true NOT NULL,
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
    health_score double precision DEFAULT 1.0 NOT NULL
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
    cache_creation_price_per_1m_1h numeric(20,8)
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
    rate_limit integer
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
-- Name: ldap_configs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ldap_configs ALTER COLUMN id SET DEFAULT nextval('public.ldap_configs_id_seq'::regclass);



--
-- Name: proxy_node_events id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.proxy_node_events ALTER COLUMN id SET DEFAULT nextval('public.proxy_node_events_id_seq'::regclass);



--
-- Name: announcement_reads announcement_reads_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT announcement_reads_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcements announcements_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcements
    ADD CONSTRAINT announcements_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings api_key_provider_mappings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT api_key_provider_mappings_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_keys api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: audit_logs audit_logs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: billing_rules billing_rules_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.billing_rules
    ADD CONSTRAINT billing_rules_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: dimension_collectors dimension_collectors_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.dimension_collectors
    ADD CONSTRAINT dimension_collectors_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_file_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_file_name_key UNIQUE (file_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: global_models global_models_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.global_models
    ADD CONSTRAINT global_models_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: ldap_configs ldap_configs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.ldap_configs
    ADD CONSTRAINT ldap_configs_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: management_tokens management_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.management_tokens
    ADD CONSTRAINT management_tokens_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models models_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: oauth_providers oauth_providers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.oauth_providers
    ADD CONSTRAINT oauth_providers_pkey PRIMARY KEY (provider_type);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_callbacks payment_callbacks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_callbacks
    ADD CONSTRAINT payment_callbacks_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders payment_orders_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT payment_orders_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_api_keys provider_api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_api_keys
    ADD CONSTRAINT provider_api_keys_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_endpoints provider_endpoints_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_endpoints
    ADD CONSTRAINT provider_endpoints_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_usage_tracking provider_usage_tracking_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_usage_tracking
    ADD CONSTRAINT provider_usage_tracking_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: providers providers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.providers
    ADD CONSTRAINT providers_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_node_events proxy_node_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_node_events
    ADD CONSTRAINT proxy_node_events_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_nodes proxy_nodes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_nodes
    ADD CONSTRAINT proxy_nodes_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_api_key stats_daily_api_key_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_api_key
    ADD CONSTRAINT stats_daily_api_key_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_error stats_daily_error_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_error
    ADD CONSTRAINT stats_daily_error_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_model stats_daily_model_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_model
    ADD CONSTRAINT stats_daily_model_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily stats_daily_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily
    ADD CONSTRAINT stats_daily_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_provider stats_daily_provider_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_provider
    ADD CONSTRAINT stats_daily_provider_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_model stats_hourly_model_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_model
    ADD CONSTRAINT stats_hourly_model_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly stats_hourly_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly
    ADD CONSTRAINT stats_hourly_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_provider stats_hourly_provider_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_provider
    ADD CONSTRAINT stats_hourly_provider_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_user stats_hourly_user_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_user
    ADD CONSTRAINT stats_hourly_user_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_summary stats_summary_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_summary
    ADD CONSTRAINT stats_summary_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_user_daily stats_user_daily_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_user_daily
    ADD CONSTRAINT stats_user_daily_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: system_configs system_configs_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.system_configs
    ADD CONSTRAINT system_configs_key_key UNIQUE (key);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: system_configs system_configs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.system_configs
    ADD CONSTRAINT system_configs_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings uq_apikey_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT uq_apikey_provider UNIQUE (api_key_id, provider_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: management_tokens uq_management_tokens_user_name; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.management_tokens
    ADD CONSTRAINT uq_management_tokens_user_name UNIQUE (user_id, name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links uq_oauth_provider_user; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT uq_oauth_provider_user UNIQUE (provider_type, provider_user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_callbacks uq_payment_callbacks_callback_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_callbacks
    ADD CONSTRAINT uq_payment_callbacks_callback_key UNIQUE (callback_key);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders uq_payment_orders_order_no; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT uq_payment_orders_order_no UNIQUE (order_no);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_endpoints uq_provider_api_format; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_endpoints
    ADD CONSTRAINT uq_provider_api_format UNIQUE (provider_id, api_format);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models uq_provider_model; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT uq_provider_model UNIQUE (provider_id, provider_model_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_nodes uq_proxy_node_ip_port; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_nodes
    ADD CONSTRAINT uq_proxy_node_ip_port UNIQUE (ip, port);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests uq_refund_requests_idempotency_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT uq_refund_requests_idempotency_key UNIQUE (idempotency_key);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests uq_refund_requests_refund_no; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT uq_refund_requests_refund_no UNIQUE (refund_no);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates uq_request_candidate_with_retry; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT uq_request_candidate_with_retry UNIQUE (request_id, candidate_index, retry_index);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_api_key uq_stats_daily_api_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_api_key
    ADD CONSTRAINT uq_stats_daily_api_key UNIQUE (api_key_id, date);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_error uq_stats_daily_error; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_error
    ADD CONSTRAINT uq_stats_daily_error UNIQUE (date, error_category, provider_name, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_model uq_stats_daily_model; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_model
    ADD CONSTRAINT uq_stats_daily_model UNIQUE (date, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_provider uq_stats_daily_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_provider
    ADD CONSTRAINT uq_stats_daily_provider UNIQUE (date, provider_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly uq_stats_hourly_hour; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly
    ADD CONSTRAINT uq_stats_hourly_hour UNIQUE (hour_utc);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_model uq_stats_hourly_model; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_model
    ADD CONSTRAINT uq_stats_hourly_model UNIQUE (hour_utc, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_provider uq_stats_hourly_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_provider
    ADD CONSTRAINT uq_stats_hourly_provider UNIQUE (hour_utc, provider_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_user uq_stats_hourly_user; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_user
    ADD CONSTRAINT uq_stats_hourly_user UNIQUE (hour_utc, user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_user_daily uq_stats_user_daily; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_user_daily
    ADD CONSTRAINT uq_stats_user_daily UNIQUE (user_id, date);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcement_reads uq_user_announcement; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT uq_user_announcement UNIQUE (user_id, announcement_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_model_usage_counts uq_user_model_usage_count; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_model_usage_counts
    ADD CONSTRAINT uq_user_model_usage_count UNIQUE (user_id, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links uq_user_oauth_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT uq_user_oauth_provider UNIQUE (user_id, provider_type);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks uq_video_tasks_request_id; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT uq_video_tasks_request_id UNIQUE (request_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_daily_usage_ledgers uq_wallet_daily_usage_ledgers_wallet_date_tz; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_daily_usage_ledgers
    ADD CONSTRAINT uq_wallet_daily_usage_ledgers_wallet_date_tz UNIQUE (wallet_id, billing_date, billing_timezone);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets uq_wallets_api_key_id; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT uq_wallets_api_key_id UNIQUE (api_key_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets uq_wallets_user_id; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT uq_wallets_user_id UNIQUE (user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_model_usage_counts user_model_usage_counts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_model_usage_counts
    ADD CONSTRAINT user_model_usage_counts_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links user_oauth_links_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT user_oauth_links_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_user_id_key UNIQUE (user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_sessions user_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_daily_usage_ledgers wallet_daily_usage_ledgers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_daily_usage_ledgers
    ADD CONSTRAINT wallet_daily_usage_ledgers_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_transactions wallet_transactions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_transactions
    ADD CONSTRAINT wallet_transactions_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets wallets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT wallets_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: idx_apikey_provider_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_apikey_provider_enabled ON public.api_key_provider_mappings USING btree (api_key_id, is_enabled);



--
-- Name: idx_endpoint_format_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_endpoint_format_active ON public.provider_endpoints USING btree (api_format, is_active);



--
-- Name: idx_gemini_file_mappings_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_gemini_file_mappings_expires ON public.gemini_file_mappings USING btree (expires_at);



--
-- Name: idx_gemini_file_mappings_source_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_gemini_file_mappings_source_hash ON public.gemini_file_mappings USING btree (source_hash);



--
-- Name: idx_management_tokens_is_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_management_tokens_is_active ON public.management_tokens USING btree (is_active);



--
-- Name: idx_management_tokens_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_management_tokens_user_id ON public.management_tokens USING btree (user_id);



--
-- Name: idx_model_provider_model_aliases_gin; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_model_provider_model_aliases_gin ON public.models USING gin (provider_model_mappings jsonb_path_ops) WHERE (is_active = true);



--
-- Name: idx_model_provider_model_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_model_provider_model_name ON public.models USING btree (provider_model_name) WHERE (is_active = true);



--
-- Name: idx_payment_callbacks_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_callbacks_created ON public.payment_callbacks USING btree (created_at);



--
-- Name: idx_payment_callbacks_gateway_order; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_callbacks_gateway_order ON public.payment_callbacks USING btree (gateway_order_id);



--
-- Name: idx_payment_callbacks_order; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_callbacks_order ON public.payment_callbacks USING btree (order_no);



--
-- Name: idx_payment_orders_gateway_order_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_gateway_order_id ON public.payment_orders USING btree (gateway_order_id);



--
-- Name: idx_payment_orders_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_status ON public.payment_orders USING btree (status);



--
-- Name: idx_payment_orders_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_user_created ON public.payment_orders USING btree (user_id, created_at);



--
-- Name: idx_payment_orders_wallet_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_wallet_created ON public.payment_orders USING btree (wallet_id, created_at);



--
-- Name: idx_provider_api_keys_provider_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_active ON public.provider_api_keys USING btree (provider_id, is_active);



--
-- Name: idx_provider_api_keys_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_id ON public.provider_api_keys USING btree (provider_id);



--
-- Name: idx_provider_family_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_family_kind ON public.provider_endpoints USING btree (provider_id, api_family, endpoint_kind);



--
-- Name: idx_provider_window; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_window ON public.provider_usage_tracking USING btree (provider_id, window_start);



--
-- Name: idx_proxy_node_events_node_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_proxy_node_events_node_created ON public.proxy_node_events USING btree (node_id, created_at);



--
-- Name: idx_rc_provider_status_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_rc_provider_status_created ON public.request_candidates USING btree (provider_id, status, created_at);



--
-- Name: idx_rc_request_id_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_rc_request_id_status ON public.request_candidates USING btree (request_id, status);



--
-- Name: idx_refund_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refund_status ON public.refund_requests USING btree (status);



--
-- Name: idx_refund_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refund_user_created ON public.refund_requests USING btree (user_id, created_at);



--
-- Name: idx_refund_wallet_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refund_wallet_created ON public.refund_requests USING btree (wallet_id, created_at);



--
-- Name: idx_request_candidates_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_created_at ON public.request_candidates USING btree (created_at);



--
-- Name: idx_request_candidates_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_provider_id ON public.request_candidates USING btree (provider_id);



--
-- Name: idx_request_candidates_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_request_id ON public.request_candidates USING btree (request_id);



--
-- Name: idx_request_candidates_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_status ON public.request_candidates USING btree (status);



--
-- Name: idx_stats_daily_api_key_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_date ON public.stats_daily_api_key USING btree (date);



--
-- Name: idx_stats_daily_api_key_date_cost; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_date_cost ON public.stats_daily_api_key USING btree (date, total_cost);



--
-- Name: idx_stats_daily_api_key_date_requests; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_date_requests ON public.stats_daily_api_key USING btree (date, total_requests);



--
-- Name: idx_stats_daily_api_key_key_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_key_date ON public.stats_daily_api_key USING btree (api_key_id, date);



--
-- Name: idx_stats_daily_error_category; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_error_category ON public.stats_daily_error USING btree (date, error_category);



--
-- Name: idx_stats_daily_error_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_error_date ON public.stats_daily_error USING btree (date);



--
-- Name: idx_stats_daily_model_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_model_date ON public.stats_daily_model USING btree (date);



--
-- Name: idx_stats_daily_model_date_model; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_model_date_model ON public.stats_daily_model USING btree (date, model);



--
-- Name: idx_stats_daily_provider_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_provider_date ON public.stats_daily_provider USING btree (date);



--
-- Name: idx_stats_daily_provider_date_provider; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_provider_date_provider ON public.stats_daily_provider USING btree (date, provider_name);



--
-- Name: idx_stats_hourly_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_hour ON public.stats_hourly USING btree (hour_utc);



--
-- Name: idx_stats_hourly_model_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_model_hour ON public.stats_hourly_model USING btree (hour_utc);



--
-- Name: idx_stats_hourly_model_model_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_model_model_hour ON public.stats_hourly_model USING btree (model, hour_utc);



--
-- Name: idx_stats_hourly_provider_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_provider_hour ON public.stats_hourly_provider USING btree (hour_utc);



--
-- Name: idx_stats_hourly_user_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_user_hour ON public.stats_hourly_user USING btree (hour_utc);



--
-- Name: idx_stats_hourly_user_user_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_user_user_hour ON public.stats_hourly_user USING btree (user_id, hour_utc);



--
-- Name: idx_stats_user_daily_user_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_user_date ON public.stats_user_daily USING btree (user_id, date);



--
-- Name: idx_usage_api_family; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_api_family ON public.usage USING btree (api_family);



--
-- Name: idx_usage_apikey_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_apikey_created ON public.usage USING btree (api_key_id, created_at);



--
-- Name: idx_usage_billing_finalized_wallet; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_billing_finalized_wallet ON public.usage USING btree (billing_status, finalized_at, wallet_id);



--
-- Name: idx_usage_billing_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_billing_status ON public.usage USING btree (billing_status);



--
-- Name: idx_usage_endpoint_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_endpoint_kind ON public.usage USING btree (endpoint_kind);



--
-- Name: idx_usage_error_category; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_error_category ON public.usage USING btree (error_category);



--
-- Name: idx_usage_family_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_family_kind ON public.usage USING btree (api_family, endpoint_kind);



--
-- Name: idx_usage_model_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_model_created ON public.usage USING btree (model, created_at);



--
-- Name: idx_usage_provider_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_provider_created ON public.usage USING btree (provider_name, created_at);



--
-- Name: idx_usage_provider_key; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_provider_key ON public.usage USING btree (provider_id, provider_api_key_id);



--
-- Name: idx_usage_provider_model_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_provider_model_created ON public.usage USING btree (provider_name, model, created_at);



--
-- Name: idx_usage_status_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_status_user_created ON public.usage USING btree (status, user_id, created_at);



--
-- Name: idx_usage_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_user_created ON public.usage USING btree (user_id, created_at);



--
-- Name: idx_usage_wallet_finalized; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_wallet_finalized ON public.usage USING btree (wallet_id, finalized_at);



--
-- Name: idx_user_model_usage_model; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_model_usage_model ON public.user_model_usage_counts USING btree (model);



--
-- Name: idx_user_model_usage_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_model_usage_user ON public.user_model_usage_counts USING btree (user_id);



--
-- Name: idx_user_sessions_user_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_sessions_user_active ON public.user_sessions USING btree (user_id, revoked_at, expires_at);



--
-- Name: idx_user_sessions_user_device; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_sessions_user_device ON public.user_sessions USING btree (user_id, client_device_id);



--
-- Name: idx_video_tasks_external_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_external_id ON public.video_tasks USING btree (external_task_id);



--
-- Name: idx_video_tasks_next_poll; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_next_poll ON public.video_tasks USING btree (next_poll_at);



--
-- Name: idx_video_tasks_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_request_id ON public.video_tasks USING btree (request_id);



--
-- Name: idx_video_tasks_user_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_user_status ON public.video_tasks USING btree (user_id, status);



--
-- Name: idx_wallet_daily_usage_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_daily_usage_date ON public.wallet_daily_usage_ledgers USING btree (billing_date);



--
-- Name: idx_wallet_daily_usage_wallet_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_daily_usage_wallet_date ON public.wallet_daily_usage_ledgers USING btree (wallet_id, billing_date);



--
-- Name: idx_wallet_tx_category_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_category_created ON public.wallet_transactions USING btree (category, created_at);



--
-- Name: idx_wallet_tx_link; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_link ON public.wallet_transactions USING btree (link_type, link_id);



--
-- Name: idx_wallet_tx_reason_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_reason_created ON public.wallet_transactions USING btree (reason_code, created_at);



--
-- Name: idx_wallet_tx_wallet_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_wallet_created ON public.wallet_transactions USING btree (wallet_id, created_at);



--
-- Name: idx_wallets_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallets_status ON public.wallets USING btree (status);



--
-- Name: idx_window_time; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_window_time ON public.provider_usage_tracking USING btree (window_start, window_end);



--
-- Name: ix_announcement_reads_announcement_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcement_reads_announcement_id ON public.announcement_reads USING btree (announcement_id);



--
-- Name: ix_announcement_reads_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcement_reads_id ON public.announcement_reads USING btree (id);



--
-- Name: ix_announcements_author_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_author_id ON public.announcements USING btree (author_id);



--
-- Name: ix_announcements_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_created_at ON public.announcements USING btree (created_at);



--
-- Name: ix_announcements_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_id ON public.announcements USING btree (id);



--
-- Name: ix_announcements_is_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_is_active ON public.announcements USING btree (is_active);



--
-- Name: ix_api_key_provider_mappings_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_key_provider_mappings_api_key_id ON public.api_key_provider_mappings USING btree (api_key_id);



--
-- Name: ix_api_key_provider_mappings_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_key_provider_mappings_id ON public.api_key_provider_mappings USING btree (id);



--
-- Name: ix_api_key_provider_mappings_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_key_provider_mappings_provider_id ON public.api_key_provider_mappings USING btree (provider_id);



--
-- Name: ix_api_keys_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_keys_id ON public.api_keys USING btree (id);



--
-- Name: ix_api_keys_key_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_api_keys_key_hash ON public.api_keys USING btree (key_hash);



--
-- Name: ix_api_keys_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_keys_user_id ON public.api_keys USING btree (user_id);



--
-- Name: ix_audit_logs_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_created_at ON public.audit_logs USING btree (created_at);



--
-- Name: ix_audit_logs_event_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_event_type ON public.audit_logs USING btree (event_type);



--
-- Name: ix_audit_logs_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_id ON public.audit_logs USING btree (id);



--
-- Name: ix_audit_logs_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_request_id ON public.audit_logs USING btree (request_id);



--
-- Name: ix_audit_logs_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_user_id ON public.audit_logs USING btree (user_id);



--
-- Name: ix_gemini_file_mappings_file_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_gemini_file_mappings_file_name ON public.gemini_file_mappings USING btree (file_name);



--
-- Name: ix_gemini_file_mappings_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_gemini_file_mappings_id ON public.gemini_file_mappings USING btree (id);



--
-- Name: ix_gemini_file_mappings_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_gemini_file_mappings_key_id ON public.gemini_file_mappings USING btree (key_id);



--
-- Name: ix_gemini_file_mappings_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_gemini_file_mappings_user_id ON public.gemini_file_mappings USING btree (user_id);



--
-- Name: ix_global_models_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_global_models_id ON public.global_models USING btree (id);



--
-- Name: ix_global_models_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_global_models_name ON public.global_models USING btree (name);



--
-- Name: ix_global_models_usage_count; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_global_models_usage_count ON public.global_models USING btree (usage_count);



--
-- Name: ix_management_tokens_token_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_management_tokens_token_hash ON public.management_tokens USING btree (token_hash);



--
-- Name: ix_models_global_model_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_models_global_model_id ON public.models USING btree (global_model_id);



--
-- Name: ix_models_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_models_id ON public.models USING btree (id);



--
-- Name: ix_payment_callbacks_payment_order_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_payment_callbacks_payment_order_id ON public.payment_callbacks USING btree (payment_order_id);



--
-- Name: ix_provider_api_keys_circuit_breaker_by_format; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_api_keys_circuit_breaker_by_format ON public.provider_api_keys USING gin (circuit_breaker_by_format);



--
-- Name: ix_provider_api_keys_health_by_format; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_api_keys_health_by_format ON public.provider_api_keys USING gin (health_by_format);



--
-- Name: ix_provider_api_keys_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_api_keys_id ON public.provider_api_keys USING btree (id);



--
-- Name: ix_provider_endpoints_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_endpoints_id ON public.provider_endpoints USING btree (id);



--
-- Name: ix_provider_usage_tracking_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_usage_tracking_id ON public.provider_usage_tracking USING btree (id);



--
-- Name: ix_provider_usage_tracking_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_usage_tracking_provider_id ON public.provider_usage_tracking USING btree (provider_id);



--
-- Name: ix_provider_usage_tracking_window_start; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_usage_tracking_window_start ON public.provider_usage_tracking USING btree (window_start);



--
-- Name: ix_providers_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_providers_id ON public.providers USING btree (id);



--
-- Name: ix_providers_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_providers_name ON public.providers USING btree (name);



--
-- Name: ix_proxy_node_events_node_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_proxy_node_events_node_id ON public.proxy_node_events USING btree (node_id);



--
-- Name: ix_proxy_nodes_registered_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_proxy_nodes_registered_by ON public.proxy_nodes USING btree (registered_by);



--
-- Name: ix_refund_requests_approved_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_approved_by ON public.refund_requests USING btree (approved_by);



--
-- Name: ix_refund_requests_payment_order_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_payment_order_id ON public.refund_requests USING btree (payment_order_id);



--
-- Name: ix_refund_requests_processed_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_processed_by ON public.refund_requests USING btree (processed_by);



--
-- Name: ix_refund_requests_requested_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_requested_by ON public.refund_requests USING btree (requested_by);



--
-- Name: ix_request_candidates_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_api_key_id ON public.request_candidates USING btree (api_key_id);



--
-- Name: ix_request_candidates_endpoint_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_endpoint_id ON public.request_candidates USING btree (endpoint_id);



--
-- Name: ix_request_candidates_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_key_id ON public.request_candidates USING btree (key_id);



--
-- Name: ix_request_candidates_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_request_id ON public.request_candidates USING btree (request_id);



--
-- Name: ix_request_candidates_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_user_id ON public.request_candidates USING btree (user_id);



--
-- Name: ix_stats_daily_date; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_stats_daily_date ON public.stats_daily USING btree (date);



--
-- Name: ix_stats_user_daily_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_stats_user_daily_date ON public.stats_user_daily USING btree (date);



--
-- Name: ix_system_configs_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_system_configs_id ON public.system_configs USING btree (id);



--
-- Name: ix_usage_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_created_at ON public.usage USING btree (created_at);



--
-- Name: ix_usage_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_id ON public.usage USING btree (id);



--
-- Name: ix_usage_provider_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_provider_api_key_id ON public.usage USING btree (provider_api_key_id);



--
-- Name: ix_usage_provider_endpoint_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_provider_endpoint_id ON public.usage USING btree (provider_endpoint_id);



--
-- Name: ix_usage_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_usage_request_id ON public.usage USING btree (request_id);



--
-- Name: ix_usage_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_status ON public.usage USING btree (status);



--
-- Name: ix_usage_wallet_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_wallet_id ON public.usage USING btree (wallet_id);



--
-- Name: ix_user_oauth_links_provider_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_oauth_links_provider_type ON public.user_oauth_links USING btree (provider_type);



--
-- Name: ix_user_oauth_links_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_oauth_links_user_id ON public.user_oauth_links USING btree (user_id);



--
-- Name: ix_user_preferences_default_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_preferences_default_provider_id ON public.user_preferences USING btree (default_provider_id);



--
-- Name: ix_user_preferences_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_preferences_id ON public.user_preferences USING btree (id);



--
-- Name: ix_user_sessions_client_device_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_sessions_client_device_id ON public.user_sessions USING btree (client_device_id);



--
-- Name: ix_user_sessions_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_sessions_user_id ON public.user_sessions USING btree (user_id);



--
-- Name: ix_users_email; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_users_email ON public.users USING btree (email);



--
-- Name: ix_users_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_users_id ON public.users USING btree (id);



--
-- Name: ix_users_ldap_dn; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_users_ldap_dn ON public.users USING btree (ldap_dn);



--
-- Name: ix_users_ldap_username; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_users_ldap_username ON public.users USING btree (ldap_username);



--
-- Name: ix_users_username; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_users_username ON public.users USING btree (username);



--
-- Name: ix_video_tasks_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_api_key_id ON public.video_tasks USING btree (api_key_id);



--
-- Name: ix_video_tasks_endpoint_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_endpoint_id ON public.video_tasks USING btree (endpoint_id);



--
-- Name: ix_video_tasks_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_key_id ON public.video_tasks USING btree (key_id);



--
-- Name: ix_video_tasks_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_provider_id ON public.video_tasks USING btree (provider_id);



--
-- Name: ix_video_tasks_remixed_from_task_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_remixed_from_task_id ON public.video_tasks USING btree (remixed_from_task_id);



--
-- Name: ix_video_tasks_short_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_video_tasks_short_id ON public.video_tasks USING btree (short_id);



--
-- Name: ix_wallet_transactions_operator_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_wallet_transactions_operator_id ON public.wallet_transactions USING btree (operator_id);



--
-- Name: uq_billing_rules_global_model_task; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS uq_billing_rules_global_model_task ON public.billing_rules USING btree (global_model_id, task_type) WHERE ((is_enabled = true) AND (global_model_id IS NOT NULL));



--
-- Name: uq_billing_rules_model_task; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS uq_billing_rules_model_task ON public.billing_rules USING btree (model_id, task_type) WHERE ((is_enabled = true) AND (model_id IS NOT NULL));



--
-- Name: uq_dimension_collectors_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS uq_dimension_collectors_enabled ON public.dimension_collectors USING btree (api_format, task_type, dimension_name, priority) WHERE (is_enabled = true);



--
-- Name: announcement_reads announcement_reads_announcement_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT announcement_reads_announcement_id_fkey FOREIGN KEY (announcement_id) REFERENCES public.announcements(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcement_reads announcement_reads_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT announcement_reads_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcements announcements_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcements
    ADD CONSTRAINT announcements_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings api_key_provider_mappings_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT api_key_provider_mappings_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings api_key_provider_mappings_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT api_key_provider_mappings_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_keys api_keys_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: audit_logs audit_logs_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: billing_rules billing_rules_global_model_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.billing_rules
    ADD CONSTRAINT billing_rules_global_model_id_fkey FOREIGN KEY (global_model_id) REFERENCES public.global_models(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: billing_rules billing_rules_model_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.billing_rules
    ADD CONSTRAINT billing_rules_model_id_fkey FOREIGN KEY (model_id) REFERENCES public.models(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_api_keys fk_provider_api_keys_provider; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_api_keys
    ADD CONSTRAINT fk_provider_api_keys_provider FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_key_id_fkey FOREIGN KEY (key_id) REFERENCES public.provider_api_keys(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: management_tokens management_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.management_tokens
    ADD CONSTRAINT management_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models models_global_model_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_global_model_id_fkey FOREIGN KEY (global_model_id) REFERENCES public.global_models(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models models_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_callbacks payment_callbacks_payment_order_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_callbacks
    ADD CONSTRAINT payment_callbacks_payment_order_id_fkey FOREIGN KEY (payment_order_id) REFERENCES public.payment_orders(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders payment_orders_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT payment_orders_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders payment_orders_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT payment_orders_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE RESTRICT;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_endpoints provider_endpoints_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_endpoints
    ADD CONSTRAINT provider_endpoints_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_usage_tracking provider_usage_tracking_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_usage_tracking
    ADD CONSTRAINT provider_usage_tracking_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_node_events proxy_node_events_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_node_events
    ADD CONSTRAINT proxy_node_events_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.proxy_nodes(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_nodes proxy_nodes_registered_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_nodes
    ADD CONSTRAINT proxy_nodes_registered_by_fkey FOREIGN KEY (registered_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_approved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_approved_by_fkey FOREIGN KEY (approved_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_payment_order_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_payment_order_id_fkey FOREIGN KEY (payment_order_id) REFERENCES public.payment_orders(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_processed_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_processed_by_fkey FOREIGN KEY (processed_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_requested_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE RESTRICT;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_endpoint_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_endpoint_id_fkey FOREIGN KEY (endpoint_id) REFERENCES public.provider_endpoints(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_api_key stats_daily_api_key_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_api_key
    ADD CONSTRAINT stats_daily_api_key_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_user_daily stats_user_daily_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_user_daily
    ADD CONSTRAINT stats_user_daily_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_provider_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_provider_api_key_id_fkey FOREIGN KEY (provider_api_key_id) REFERENCES public.provider_api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_provider_endpoint_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_provider_endpoint_id_fkey FOREIGN KEY (provider_endpoint_id) REFERENCES public.provider_endpoints(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_model_usage_counts user_model_usage_counts_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_model_usage_counts
    ADD CONSTRAINT user_model_usage_counts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links user_oauth_links_provider_type_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT user_oauth_links_provider_type_fkey FOREIGN KEY (provider_type) REFERENCES public.oauth_providers(provider_type) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links user_oauth_links_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT user_oauth_links_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_default_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_default_provider_id_fkey FOREIGN KEY (default_provider_id) REFERENCES public.providers(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_sessions user_sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_endpoint_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_endpoint_id_fkey FOREIGN KEY (endpoint_id) REFERENCES public.provider_endpoints(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_key_id_fkey FOREIGN KEY (key_id) REFERENCES public.provider_api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_remixed_from_task_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_remixed_from_task_id_fkey FOREIGN KEY (remixed_from_task_id) REFERENCES public.video_tasks(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_daily_usage_ledgers wallet_daily_usage_ledgers_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_daily_usage_ledgers
    ADD CONSTRAINT wallet_daily_usage_ledgers_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_transactions wallet_transactions_operator_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_transactions
    ADD CONSTRAINT wallet_transactions_operator_id_fkey FOREIGN KEY (operator_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_transactions wallet_transactions_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_transactions
    ADD CONSTRAINT wallet_transactions_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets wallets_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT wallets_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets wallets_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT wallets_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- PostgreSQL database dump complete
--

