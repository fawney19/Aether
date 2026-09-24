ALTER TABLE public.user_groups
    ADD COLUMN IF NOT EXISTS provider_key_policies jsonb;
