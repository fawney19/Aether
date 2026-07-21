ALTER TABLE public.management_tokens
    ADD COLUMN IF NOT EXISTS permissions json;

