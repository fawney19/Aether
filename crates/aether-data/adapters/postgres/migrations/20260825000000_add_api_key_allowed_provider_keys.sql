ALTER TABLE api_keys
ADD COLUMN IF NOT EXISTS allowed_provider_keys jsonb NULL;

ALTER TABLE api_keys
ALTER COLUMN allowed_provider_keys TYPE jsonb USING allowed_provider_keys::jsonb;
