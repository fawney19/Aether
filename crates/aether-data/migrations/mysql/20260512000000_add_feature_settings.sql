ALTER TABLE users
    ADD COLUMN feature_settings TEXT NULL AFTER model_capability_settings;

ALTER TABLE api_keys
    ADD COLUMN feature_settings TEXT NULL AFTER force_capabilities;
