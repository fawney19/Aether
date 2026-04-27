ALTER TABLE public.provider_api_keys ADD COLUMN IF NOT EXISTS concurrent_limit integer;
