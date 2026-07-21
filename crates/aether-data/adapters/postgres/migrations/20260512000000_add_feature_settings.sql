ALTER TABLE public.users
    ADD COLUMN IF NOT EXISTS feature_settings jsonb;

ALTER TABLE public.api_keys
    ADD COLUMN IF NOT EXISTS feature_settings jsonb;
