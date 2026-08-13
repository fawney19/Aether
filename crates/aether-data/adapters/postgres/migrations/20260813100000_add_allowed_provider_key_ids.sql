-- Add provider-level key scope allowlists (provider_id -> provider_api_keys.id list)
-- to api_keys and user_groups. NULL preserves the legacy provider-only behavior.
ALTER TABLE public.api_keys
    ADD COLUMN IF NOT EXISTS allowed_provider_key_ids jsonb;

ALTER TABLE public.user_groups
    ADD COLUMN IF NOT EXISTS allowed_provider_key_ids jsonb;
