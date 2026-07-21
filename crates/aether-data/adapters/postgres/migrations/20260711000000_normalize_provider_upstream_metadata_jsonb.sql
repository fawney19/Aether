ALTER TABLE public.provider_api_keys
  ALTER COLUMN upstream_metadata TYPE jsonb
  USING upstream_metadata::jsonb;
