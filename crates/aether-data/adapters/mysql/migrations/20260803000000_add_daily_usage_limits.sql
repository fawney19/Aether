ALTER TABLE api_keys
  ADD COLUMN daily_usage_limit_usd DOUBLE NULL;

ALTER TABLE user_groups
  ADD COLUMN daily_usage_limit_usd DOUBLE NULL,
  ADD COLUMN daily_usage_limit_mode VARCHAR(32) NOT NULL DEFAULT 'inherit';
