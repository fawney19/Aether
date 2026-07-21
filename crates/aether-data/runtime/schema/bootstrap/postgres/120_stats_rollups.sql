CREATE TABLE IF NOT EXISTS public.stats_user_summary (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
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
    active_days integer DEFAULT 0 NOT NULL,
    first_active_date timestamp with time zone,
    last_active_date timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_summary_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_summary_user_id UNIQUE (user_id),
    CONSTRAINT stats_user_summary_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_summary_cutoff_date
    ON public.stats_user_summary USING btree (cutoff_date);

ALTER TABLE public.stats_daily
    ADD COLUMN IF NOT EXISTS effective_input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS total_input_context bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_samples bigint DEFAULT 0 NOT NULL;

ALTER TABLE public.stats_daily_model
    ADD COLUMN IF NOT EXISTS response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_samples bigint DEFAULT 0 NOT NULL;

CREATE TABLE IF NOT EXISTS public.stats_user_daily_model (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    response_time_samples bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_model_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_model_user_date_model UNIQUE (user_id, date, model),
    CONSTRAINT stats_user_daily_model_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_model_date
    ON public.stats_user_daily_model USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_model_user_id
    ON public.stats_user_daily_model USING btree (user_id);

ALTER TABLE public.stats_hourly
    ADD COLUMN IF NOT EXISTS response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_samples bigint DEFAULT 0 NOT NULL;

ALTER TABLE public.stats_hourly_model
    ADD COLUMN IF NOT EXISTS response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_samples bigint DEFAULT 0 NOT NULL;

CREATE TABLE IF NOT EXISTS public.stats_hourly_user_model (
    id character varying(36) NOT NULL,
    hour_utc timestamp with time zone NOT NULL,
    user_id character varying(36) NOT NULL,
    model character varying(100) NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    response_time_samples bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_hourly_user_model_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_hourly_user_model UNIQUE (hour_utc, user_id, model),
    CONSTRAINT stats_hourly_user_model_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_hourly_user_model_hour
    ON public.stats_hourly_user_model USING btree (hour_utc);

CREATE INDEX IF NOT EXISTS idx_stats_hourly_user_model_user_hour
    ON public.stats_hourly_user_model USING btree (user_id, hour_utc);

CREATE TABLE IF NOT EXISTS public.schema_backfills (
    version bigint NOT NULL,
    description text NOT NULL,
    success boolean NOT NULL DEFAULT TRUE,
    checksum bytea NOT NULL,
    execution_time bigint NOT NULL DEFAULT 0,
    applied_at timestamp with time zone NOT NULL DEFAULT now(),
    CONSTRAINT schema_backfills_pkey PRIMARY KEY (version)
);

CREATE INDEX IF NOT EXISTS idx_schema_backfills_applied_at
    ON public.schema_backfills USING btree (applied_at DESC);

ALTER TABLE public.stats_user_daily_model
    ADD COLUMN IF NOT EXISTS success_requests integer DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS effective_input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS total_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS total_input_context bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_5m_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_1h_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS actual_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS successful_response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS successful_response_time_samples bigint DEFAULT 0 NOT NULL;

CREATE TABLE IF NOT EXISTS public.stats_user_daily_provider (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    provider_name character varying(100) NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    success_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    effective_input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_input_context bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_ephemeral_5m_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_ephemeral_1h_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    actual_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    response_time_samples bigint DEFAULT 0 NOT NULL,
    successful_response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    successful_response_time_samples bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_provider_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_provider_user_date_provider UNIQUE (user_id, date, provider_name),
    CONSTRAINT stats_user_daily_provider_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_provider_date
    ON public.stats_user_daily_provider USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_provider_user_id
    ON public.stats_user_daily_provider USING btree (user_id);

CREATE TABLE IF NOT EXISTS public.stats_user_daily_api_format (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    api_format character varying(50) NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    success_requests integer DEFAULT 0 NOT NULL,
    input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    effective_input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    output_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_input_context bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_ephemeral_5m_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_creation_ephemeral_1h_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    actual_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    response_time_samples bigint DEFAULT 0 NOT NULL,
    successful_response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    successful_response_time_samples bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_api_format_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_api_format_user_date_api_format UNIQUE (user_id, date, api_format),
    CONSTRAINT stats_user_daily_api_format_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_api_format_date
    ON public.stats_user_daily_api_format USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_api_format_user_id
    ON public.stats_user_daily_api_format USING btree (user_id);

CREATE TABLE IF NOT EXISTS public.stats_daily_model_provider (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    provider_name character varying(100) NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    total_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    response_time_samples bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_daily_model_provider_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_daily_model_provider UNIQUE (date, model, provider_name)
);

CREATE INDEX IF NOT EXISTS idx_stats_daily_model_provider_date
    ON public.stats_daily_model_provider USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_daily_model_provider_date_model_provider
    ON public.stats_daily_model_provider USING btree (date, model, provider_name);

CREATE TABLE IF NOT EXISTS public.stats_user_daily_model_provider (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    provider_name character varying(100) NOT NULL,
    total_requests integer DEFAULT 0 NOT NULL,
    total_tokens bigint DEFAULT '0'::bigint NOT NULL,
    total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    response_time_samples bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_model_provider_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_model_provider UNIQUE (user_id, date, model, provider_name),
    CONSTRAINT stats_user_daily_model_provider_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_model_provider_date
    ON public.stats_user_daily_model_provider USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_model_provider_user_date
    ON public.stats_user_daily_model_provider USING btree (user_id, date);

ALTER TABLE public.stats_daily
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_5m_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_1h_tokens bigint DEFAULT '0'::bigint NOT NULL;

ALTER TABLE public.stats_user_daily
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_5m_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_1h_tokens bigint DEFAULT '0'::bigint NOT NULL;

ALTER TABLE public.stats_daily_model
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_5m_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_1h_tokens bigint DEFAULT '0'::bigint NOT NULL;

ALTER TABLE public.stats_daily
    ADD COLUMN IF NOT EXISTS cache_hit_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_hit_requests bigint DEFAULT 0 NOT NULL;

ALTER TABLE public.stats_hourly
    ADD COLUMN IF NOT EXISTS cache_hit_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_hit_requests bigint DEFAULT 0 NOT NULL;

ALTER TABLE public.stats_daily
    ADD COLUMN IF NOT EXISTS completed_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_hit_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_input_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_read_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_total_input_context bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL;

ALTER TABLE public.stats_hourly
    ADD COLUMN IF NOT EXISTS completed_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_hit_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_input_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_read_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_total_input_context bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS completed_cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL;

ALTER TABLE public.stats_daily
    ADD COLUMN IF NOT EXISTS settled_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_input_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_output_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_read_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_first_finalized_at_unix_secs bigint,
    ADD COLUMN IF NOT EXISTS settled_last_finalized_at_unix_secs bigint;

ALTER TABLE public.stats_hourly
    ADD COLUMN IF NOT EXISTS settled_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_input_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_output_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_read_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_first_finalized_at_unix_secs bigint,
    ADD COLUMN IF NOT EXISTS settled_last_finalized_at_unix_secs bigint;

ALTER TABLE public.stats_user_daily
    ADD COLUMN IF NOT EXISTS settled_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_input_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_output_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_read_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_first_finalized_at_unix_secs bigint,
    ADD COLUMN IF NOT EXISTS settled_last_finalized_at_unix_secs bigint;

ALTER TABLE public.stats_hourly_user
    ADD COLUMN IF NOT EXISTS settled_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_total_requests bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_input_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_output_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_cache_read_tokens bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS settled_first_finalized_at_unix_secs bigint,
    ADD COLUMN IF NOT EXISTS settled_last_finalized_at_unix_secs bigint;
