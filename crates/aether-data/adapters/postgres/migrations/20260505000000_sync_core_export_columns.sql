ALTER TABLE public.users
    ADD COLUMN IF NOT EXISTS external_id character varying(255),
    ADD COLUMN IF NOT EXISTS metadata json;

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

ALTER TABLE public.api_keys
    ADD COLUMN IF NOT EXISTS key_prefix character varying(64),
    ADD COLUMN IF NOT EXISTS status character varying(64) DEFAULT 'active'::character varying NOT NULL,
    ADD COLUMN IF NOT EXISTS metadata json;

ALTER TABLE public.providers
    ADD COLUMN IF NOT EXISTS enabled boolean DEFAULT true NOT NULL,
    ADD COLUMN IF NOT EXISTS priority bigint DEFAULT 0 NOT NULL;

ALTER TABLE public.provider_api_keys
    ADD COLUMN IF NOT EXISTS encrypted_key text,
    ADD COLUMN IF NOT EXISTS status character varying(64) DEFAULT 'active'::character varying NOT NULL,
    ADD COLUMN IF NOT EXISTS weight bigint DEFAULT 1 NOT NULL,
    ADD COLUMN IF NOT EXISTS metadata json;

ALTER TABLE public.provider_endpoints
    ADD COLUMN IF NOT EXISTS name character varying(255),
    ADD COLUMN IF NOT EXISTS enabled boolean DEFAULT true NOT NULL,
    ADD COLUMN IF NOT EXISTS weight bigint DEFAULT 1 NOT NULL,
    ADD COLUMN IF NOT EXISTS metadata json;

ALTER TABLE public.provider_endpoints
    ALTER COLUMN api_format DROP NOT NULL;

ALTER TABLE public.global_models
    ADD COLUMN IF NOT EXISTS enabled boolean DEFAULT true NOT NULL,
    ADD COLUMN IF NOT EXISTS metadata json;

ALTER TABLE public.global_models
    ALTER COLUMN display_name DROP NOT NULL,
    ALTER COLUMN default_tiered_pricing DROP NOT NULL;

ALTER TABLE public.models
    ADD COLUMN IF NOT EXISTS global_model_name character varying(255),
    ADD COLUMN IF NOT EXISTS api_format character varying(128),
    ADD COLUMN IF NOT EXISTS enabled boolean DEFAULT true NOT NULL,
    ADD COLUMN IF NOT EXISTS metadata json;

ALTER TABLE public.models
    ALTER COLUMN global_model_id DROP NOT NULL;

ALTER TABLE public.usage
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_5m_input_tokens integer DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_1h_input_tokens integer DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS created_at_unix_ms bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS updated_at_unix_secs bigint DEFAULT 0 NOT NULL;
