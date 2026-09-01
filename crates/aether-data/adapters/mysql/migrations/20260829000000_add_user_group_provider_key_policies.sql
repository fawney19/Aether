ALTER TABLE user_groups
    ADD COLUMN provider_key_policies TEXT NULL AFTER allowed_providers_mode;
