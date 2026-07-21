CREATE TABLE IF NOT EXISTS public.stats_daily_cost_savings (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_daily_cost_savings_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_daily_cost_savings_date UNIQUE (date)
);

CREATE INDEX IF NOT EXISTS idx_stats_daily_cost_savings_date
    ON public.stats_daily_cost_savings USING btree (date);

CREATE TABLE IF NOT EXISTS public.stats_daily_cost_savings_provider (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    provider_name character varying(100) NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_daily_cost_savings_provider_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_daily_cost_savings_provider UNIQUE (date, provider_name)
);

CREATE INDEX IF NOT EXISTS idx_stats_daily_cost_savings_provider_date
    ON public.stats_daily_cost_savings_provider USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_daily_cost_savings_provider_date_provider
    ON public.stats_daily_cost_savings_provider USING btree (date, provider_name);

CREATE TABLE IF NOT EXISTS public.stats_daily_cost_savings_model (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_daily_cost_savings_model_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_daily_cost_savings_model UNIQUE (date, model)
);

CREATE INDEX IF NOT EXISTS idx_stats_daily_cost_savings_model_date
    ON public.stats_daily_cost_savings_model USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_daily_cost_savings_model_date_model
    ON public.stats_daily_cost_savings_model USING btree (date, model);

CREATE TABLE IF NOT EXISTS public.stats_daily_cost_savings_model_provider (
    id character varying(36) NOT NULL,
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    provider_name character varying(100) NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_daily_cost_savings_model_provider_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_daily_cost_savings_model_provider UNIQUE (date, model, provider_name)
);

CREATE INDEX IF NOT EXISTS idx_stats_daily_cost_savings_model_provider_date
    ON public.stats_daily_cost_savings_model_provider USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_daily_cost_savings_model_provider_date_dims
    ON public.stats_daily_cost_savings_model_provider USING btree (date, model, provider_name);

CREATE TABLE IF NOT EXISTS public.stats_user_daily_cost_savings (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_cost_savings_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_cost_savings UNIQUE (user_id, date),
    CONSTRAINT stats_user_daily_cost_savings_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_date
    ON public.stats_user_daily_cost_savings USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_user_date
    ON public.stats_user_daily_cost_savings USING btree (user_id, date);

CREATE TABLE IF NOT EXISTS public.stats_user_daily_cost_savings_provider (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    provider_name character varying(100) NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_cost_savings_provider_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_cost_savings_provider UNIQUE (user_id, date, provider_name),
    CONSTRAINT stats_user_daily_cost_savings_provider_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_provider_date
    ON public.stats_user_daily_cost_savings_provider USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_provider_user_date
    ON public.stats_user_daily_cost_savings_provider USING btree (user_id, date);

CREATE TABLE IF NOT EXISTS public.stats_user_daily_cost_savings_model (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_cost_savings_model_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_cost_savings_model UNIQUE (user_id, date, model),
    CONSTRAINT stats_user_daily_cost_savings_model_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_model_date
    ON public.stats_user_daily_cost_savings_model USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_model_user_date
    ON public.stats_user_daily_cost_savings_model USING btree (user_id, date);

CREATE TABLE IF NOT EXISTS public.stats_user_daily_cost_savings_model_provider (
    id character varying(36) NOT NULL,
    user_id character varying(36) NOT NULL,
    username character varying(100),
    date timestamp with time zone NOT NULL,
    model character varying(100) NOT NULL,
    provider_name character varying(100) NOT NULL,
    cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    estimated_full_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT stats_user_daily_cost_savings_model_provider_pkey PRIMARY KEY (id),
    CONSTRAINT uq_stats_user_daily_cost_savings_model_provider
        UNIQUE (user_id, date, model, provider_name),
    CONSTRAINT stats_user_daily_cost_savings_model_provider_user_id_fkey
        FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_model_provider_date
    ON public.stats_user_daily_cost_savings_model_provider USING btree (date);

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_cost_savings_model_provider_user_date
    ON public.stats_user_daily_cost_savings_model_provider USING btree (user_id, date);
