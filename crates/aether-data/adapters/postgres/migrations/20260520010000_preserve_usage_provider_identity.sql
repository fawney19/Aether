-- Usage is a historical fact table. Keep the provider_id snapshot even if the
-- provider catalog row is deleted, and backfill rows that can still be matched
-- by the unique provider name.

UPDATE public.usage AS usage_rows
SET provider_id = providers.id
FROM public.providers AS providers
WHERE usage_rows.provider_id IS NULL
  AND BTRIM(COALESCE(usage_rows.provider_name, '')) <> ''
  AND lower(BTRIM(COALESCE(usage_rows.provider_name, ''))) NOT IN ('unknown', 'unknow', 'pending')
  AND providers.name = BTRIM(usage_rows.provider_name);

ALTER TABLE ONLY public.usage
  DROP CONSTRAINT IF EXISTS usage_provider_id_fkey;
