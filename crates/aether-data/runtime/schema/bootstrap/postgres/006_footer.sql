-- Restore a normal lookup path before sqlx records this migration in the
-- same transaction. sqlx inserts into `_sqlx_migrations` unqualified.
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

INSERT INTO public.system_configs (id, key, value, description)
VALUES (
    '00000000-0000-0000-0000-000000000002',
    'default_user_group_id',
    '"00000000-0000-0000-0000-000000000001"'::json,
    'Default user group'
)
ON CONFLICT (key) DO NOTHING;

INSERT INTO public.user_group_members (group_id, user_id)
SELECT '00000000-0000-0000-0000-000000000001', id
FROM public.users
WHERE is_deleted IS FALSE
  AND LOWER(role::text) <> 'admin'
ON CONFLICT (group_id, user_id) DO NOTHING;

SELECT pg_catalog.set_config('search_path', 'public', true);



--
-- PostgreSQL database dump complete
--
