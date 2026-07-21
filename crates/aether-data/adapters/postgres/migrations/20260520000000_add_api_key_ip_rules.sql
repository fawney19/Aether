ALTER TABLE api_keys
ADD COLUMN IF NOT EXISTS ip_rules jsonb NULL;

ALTER TABLE api_keys
ALTER COLUMN ip_rules TYPE jsonb USING ip_rules::jsonb;
