ALTER TABLE api_keys
  ADD COLUMN IF NOT EXISTS daily_usage_limit_usd double precision;

ALTER TABLE user_groups
  ADD COLUMN IF NOT EXISTS daily_usage_limit_usd double precision,
  ADD COLUMN IF NOT EXISTS daily_usage_limit_mode text NOT NULL DEFAULT 'inherit';

ALTER TABLE user_groups
  DROP CONSTRAINT IF EXISTS user_groups_daily_usage_limit_mode_check;
ALTER TABLE user_groups
  ADD CONSTRAINT user_groups_daily_usage_limit_mode_check
  CHECK (daily_usage_limit_mode IN ('inherit', 'system', 'custom'));
