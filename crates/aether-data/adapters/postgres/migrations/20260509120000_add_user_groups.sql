ALTER TABLE public.users
    ADD COLUMN IF NOT EXISTS allowed_providers_mode text DEFAULT 'unrestricted' NOT NULL,
    ADD COLUMN IF NOT EXISTS allowed_api_formats_mode text DEFAULT 'unrestricted' NOT NULL,
    ADD COLUMN IF NOT EXISTS allowed_models_mode text DEFAULT 'unrestricted' NOT NULL,
    ADD COLUMN IF NOT EXISTS rate_limit_mode text DEFAULT 'system' NOT NULL;

UPDATE public.users
SET allowed_providers_mode = CASE WHEN allowed_providers IS NULL THEN 'unrestricted' ELSE 'specific' END
WHERE allowed_providers_mode = 'unrestricted';

UPDATE public.users
SET allowed_api_formats_mode = CASE WHEN allowed_api_formats IS NULL THEN 'unrestricted' ELSE 'specific' END
WHERE allowed_api_formats_mode = 'unrestricted';

UPDATE public.users
SET allowed_models_mode = CASE WHEN allowed_models IS NULL THEN 'unrestricted' ELSE 'specific' END
WHERE allowed_models_mode = 'unrestricted';

UPDATE public.users
SET rate_limit_mode = CASE WHEN rate_limit IS NULL THEN 'system' ELSE 'custom' END
WHERE rate_limit_mode = 'system';

CREATE TABLE IF NOT EXISTS public.user_groups (
    id character varying(36) PRIMARY KEY,
    name character varying(100) NOT NULL,
    normalized_name character varying(100) NOT NULL UNIQUE,
    description text,
    priority integer DEFAULT 0 NOT NULL,
    allowed_providers json,
    allowed_providers_mode text DEFAULT 'inherit' NOT NULL,
    allowed_api_formats json,
    allowed_api_formats_mode text DEFAULT 'inherit' NOT NULL,
    allowed_models json,
    allowed_models_mode text DEFAULT 'inherit' NOT NULL,
    rate_limit integer,
    rate_limit_mode text DEFAULT 'inherit' NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT user_groups_allowed_providers_mode_check
        CHECK (allowed_providers_mode IN ('inherit', 'unrestricted', 'specific', 'deny_all')),
    CONSTRAINT user_groups_allowed_api_formats_mode_check
        CHECK (allowed_api_formats_mode IN ('inherit', 'unrestricted', 'specific', 'deny_all')),
    CONSTRAINT user_groups_allowed_models_mode_check
        CHECK (allowed_models_mode IN ('inherit', 'unrestricted', 'specific', 'deny_all')),
    CONSTRAINT user_groups_rate_limit_mode_check
        CHECK (rate_limit_mode IN ('inherit', 'system', 'custom'))
);

CREATE TABLE IF NOT EXISTS public.user_group_members (
    group_id character varying(36) NOT NULL REFERENCES public.user_groups(id) ON DELETE CASCADE,
    user_id character varying(36) NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    PRIMARY KEY (group_id, user_id)
);

CREATE INDEX IF NOT EXISTS user_group_members_user_id_idx
    ON public.user_group_members (user_id);

CREATE INDEX IF NOT EXISTS user_groups_priority_name_idx
    ON public.user_groups (priority DESC, name ASC, id ASC);

INSERT INTO public.user_groups (
    id,
    name,
    normalized_name,
    description,
    priority,
    allowed_providers_mode,
    allowed_api_formats_mode,
    allowed_models_mode,
    rate_limit_mode
)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'Default',
    'default',
    'Default group for all users',
    0,
    'unrestricted',
    'unrestricted',
    'unrestricted',
    'system'
)
ON CONFLICT (id) DO NOTHING;

INSERT INTO public.system_configs (id, key, value, description, created_at, updated_at)
VALUES (
    '00000000-0000-0000-0000-000000000002',
    'default_user_group_id',
    '"00000000-0000-0000-0000-000000000001"'::json,
    'Default user group',
    now(),
    now()
)
ON CONFLICT (key) DO NOTHING;

INSERT INTO public.user_group_members (group_id, user_id)
SELECT '00000000-0000-0000-0000-000000000001', id
FROM public.users
WHERE is_deleted IS FALSE
  AND LOWER(role::text) <> 'admin'
ON CONFLICT (group_id, user_id) DO NOTHING;
