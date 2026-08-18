ALTER TABLE `usage`
    ADD COLUMN `outcome_class` VARCHAR(32) NOT NULL DEFAULT 'in_flight',
    ADD COLUMN `sla_eligible` TINYINT(1) NOT NULL DEFAULT 0;

ALTER TABLE provider_api_keys
    ADD COLUMN `sla_eligible_count` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_count` BIGINT NOT NULL DEFAULT 0;

ALTER TABLE usage_counter_deltas
    ADD COLUMN `sla_eligible_count_delta` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_count_delta` BIGINT NOT NULL DEFAULT 0;

ALTER TABLE stats_hourly
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;
ALTER TABLE stats_hourly_user
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;
ALTER TABLE stats_daily
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;
ALTER TABLE stats_daily_api_key
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;

ALTER TABLE stats_summary
    ADD COLUMN `all_time_sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `all_time_user_error_requests` BIGINT NOT NULL DEFAULT 0;
ALTER TABLE stats_user_summary
    ADD COLUMN `all_time_sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `all_time_user_error_requests` BIGINT NOT NULL DEFAULT 0;

ALTER TABLE stats_user_daily_model
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily_provider
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;
ALTER TABLE stats_user_daily_api_format
    ADD COLUMN `sla_eligible_requests` BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN `user_error_requests` BIGINT NOT NULL DEFAULT 0;
