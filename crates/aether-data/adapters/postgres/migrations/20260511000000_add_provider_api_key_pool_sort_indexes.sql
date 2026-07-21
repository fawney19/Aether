CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_created_at_desc
    ON public.provider_api_keys USING btree (provider_id, created_at DESC NULLS LAST, name, id);

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_last_used_at_desc
    ON public.provider_api_keys USING btree (provider_id, last_used_at DESC NULLS LAST, name, id);
