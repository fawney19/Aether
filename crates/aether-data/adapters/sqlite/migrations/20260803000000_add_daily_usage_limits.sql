ALTER TABLE api_keys ADD COLUMN daily_usage_limit_usd REAL;
ALTER TABLE user_groups ADD COLUMN daily_usage_limit_usd REAL;
ALTER TABLE user_groups ADD COLUMN daily_usage_limit_mode TEXT NOT NULL DEFAULT 'inherit';
