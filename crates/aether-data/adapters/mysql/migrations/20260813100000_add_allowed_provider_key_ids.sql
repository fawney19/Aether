-- Add provider-level key scope allowlists (provider_id -> provider_api_keys.id list)
-- to api_keys and user_groups. NULL preserves the legacy provider-only behavior.
-- TEXT matches the existing MySQL policy-column convention (allowed_providers etc.).
ALTER TABLE api_keys
    ADD COLUMN allowed_provider_key_ids TEXT NULL;

ALTER TABLE user_groups
    ADD COLUMN allowed_provider_key_ids TEXT NULL;
