ALTER TABLE public.api_keys
    ADD COLUMN IF NOT EXISTS total_tokens bigint DEFAULT '0'::bigint NOT NULL;

UPDATE public.provider_api_keys
SET
    fingerprint = NULL,
    updated_at = NOW()
WHERE fingerprint IS NOT NULL;
