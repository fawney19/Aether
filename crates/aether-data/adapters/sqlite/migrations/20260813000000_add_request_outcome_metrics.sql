ALTER TABLE "usage" ADD COLUMN outcome_class TEXT NOT NULL DEFAULT 'in_flight';
ALTER TABLE "usage" ADD COLUMN sla_eligible INTEGER NOT NULL DEFAULT 0;

ALTER TABLE provider_api_keys ADD COLUMN sla_eligible_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE provider_api_keys ADD COLUMN user_error_count INTEGER NOT NULL DEFAULT 0;

ALTER TABLE usage_counter_deltas ADD COLUMN sla_eligible_count_delta INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_counter_deltas ADD COLUMN user_error_count_delta INTEGER NOT NULL DEFAULT 0;

ALTER TABLE stats_hourly ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_hourly ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_hourly_user ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_hourly_user ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_daily ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_daily ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_daily_api_key ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_daily_api_key ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;

ALTER TABLE stats_summary ADD COLUMN all_time_sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_summary ADD COLUMN all_time_user_error_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_summary ADD COLUMN all_time_sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_summary ADD COLUMN all_time_user_error_requests INTEGER NOT NULL DEFAULT 0;

ALTER TABLE stats_user_daily_model ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily_model ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily_provider ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily_provider ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily_api_format ADD COLUMN sla_eligible_requests INTEGER NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily_api_format ADD COLUMN user_error_requests INTEGER NOT NULL DEFAULT 0;
