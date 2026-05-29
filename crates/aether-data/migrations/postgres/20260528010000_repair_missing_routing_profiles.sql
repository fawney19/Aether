CREATE TABLE IF NOT EXISTS public.routing_groups (
    id character varying(64) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    enabled boolean DEFAULT true NOT NULL,
    is_system_default boolean DEFAULT false NOT NULL,
    config_json jsonb NOT NULL,
    version bigint DEFAULT 1 NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    published_at bigint
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'routing_groups_pkey'
    ) THEN
        ALTER TABLE ONLY public.routing_groups
            ADD CONSTRAINT routing_groups_pkey PRIMARY KEY (id);
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'routing_groups_name_key'
    ) THEN
        ALTER TABLE ONLY public.routing_groups
            ADD CONSTRAINT routing_groups_name_key UNIQUE (name);
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS routing_groups_system_default_idx
    ON public.routing_groups USING btree (is_system_default, enabled);

CREATE TABLE IF NOT EXISTS public.routing_group_bindings (
    id character varying(64) NOT NULL,
    group_id character varying(64) NOT NULL,
    subject_type character varying(32) NOT NULL,
    subject_id character varying(64) NOT NULL,
    is_default boolean DEFAULT false NOT NULL,
    allow_explicit_select boolean DEFAULT true NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'routing_group_bindings_pkey'
    ) THEN
        ALTER TABLE ONLY public.routing_group_bindings
            ADD CONSTRAINT routing_group_bindings_pkey PRIMARY KEY (id);
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS routing_group_bindings_group_id_idx
    ON public.routing_group_bindings USING btree (group_id);
CREATE INDEX IF NOT EXISTS routing_group_bindings_subject_idx
    ON public.routing_group_bindings USING btree (subject_type, subject_id);

CREATE TABLE IF NOT EXISTS public.routing_group_versions (
    id character varying(64) NOT NULL,
    group_id character varying(64) NOT NULL,
    version bigint NOT NULL,
    config_json jsonb NOT NULL,
    created_at bigint NOT NULL,
    created_by character varying(64)
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'routing_group_versions_pkey'
    ) THEN
        ALTER TABLE ONLY public.routing_group_versions
            ADD CONSTRAINT routing_group_versions_pkey PRIMARY KEY (id);
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'routing_group_versions_group_version_key'
    ) THEN
        ALTER TABLE ONLY public.routing_group_versions
            ADD CONSTRAINT routing_group_versions_group_version_key UNIQUE (group_id, version);
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS routing_group_versions_group_id_idx
    ON public.routing_group_versions USING btree (group_id);