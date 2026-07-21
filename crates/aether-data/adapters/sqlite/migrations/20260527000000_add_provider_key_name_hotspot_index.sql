CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_name_id
    ON provider_api_keys (provider_id, name, id);
