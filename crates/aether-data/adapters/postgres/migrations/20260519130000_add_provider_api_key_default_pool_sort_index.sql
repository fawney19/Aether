CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_default_sort
    ON public.provider_api_keys USING btree (provider_id, internal_priority, name, id);
