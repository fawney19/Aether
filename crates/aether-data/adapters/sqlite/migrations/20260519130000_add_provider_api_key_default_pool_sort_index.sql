CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_default_sort
ON provider_api_keys (provider_id, internal_priority, name, id);
