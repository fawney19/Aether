ALTER TABLE public.provider_api_keys
ADD COLUMN IF NOT EXISTS concurrency_queue_enabled boolean NOT NULL DEFAULT true;
