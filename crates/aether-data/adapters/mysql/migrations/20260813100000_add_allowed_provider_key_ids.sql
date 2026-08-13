-- Add provider-level key scope allowlists (provider_id -> provider_api_keys.id list)
-- to api_keys and user_groups. NULL preserves the legacy provider-only behavior.
ALTER TABLE api_keys
    ADD COLUMN allowed_provider_key_ids JSON NULL;

ALTER TABLE user_groups
    ADD COLUMN allowed_provider_key_ids JSON NULL;
