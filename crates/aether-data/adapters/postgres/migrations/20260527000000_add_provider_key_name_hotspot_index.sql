CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_name_id
    ON public.provider_api_keys USING btree (provider_id, name, id);
