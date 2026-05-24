ALTER TABLE api_keys
ADD COLUMN ip_rules TEXT NULL AFTER allowed_models;
