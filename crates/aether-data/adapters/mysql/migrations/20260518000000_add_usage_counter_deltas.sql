CREATE TABLE IF NOT EXISTS usage_counter_deltas (
    `id` VARCHAR(36) NOT NULL,
    `request_id` VARCHAR(128) NOT NULL,
    `kind` VARCHAR(64) NOT NULL,
    `target_id` TEXT NOT NULL,
    `request_count_delta` BIGINT NOT NULL DEFAULT 0,
    `total_requests_delta` BIGINT NOT NULL DEFAULT 0,
    `success_count_delta` BIGINT NOT NULL DEFAULT 0,
    `error_count_delta` BIGINT NOT NULL DEFAULT 0,
    `dns_failures_delta` BIGINT NOT NULL DEFAULT 0,
    `stream_errors_delta` BIGINT NOT NULL DEFAULT 0,
    `total_tokens_delta` BIGINT NOT NULL DEFAULT 0,
    `total_cost_usd_delta` DOUBLE NOT NULL DEFAULT 0,
    `total_response_time_ms_delta` BIGINT NOT NULL DEFAULT 0,
    `last_used_at_unix_secs` BIGINT,
    `last_used_ip` TEXT,
    `candidate_last_used_at_unix_secs` BIGINT,
    `removed_last_used_at_unix_secs` BIGINT,
    `usage_created_at_unix_secs` BIGINT,
    `created_at` BIGINT NOT NULL,
    `processed_at` BIGINT,
    PRIMARY KEY (`id`),
    KEY ix_usage_counter_deltas_unprocessed (`created_at`, `id`),
    KEY ix_usage_counter_deltas_processed (`processed_at`, `created_at`, `id`),
    KEY ix_usage_counter_deltas_request_kind (`request_id`, `kind`, `target_id`(191))
);

CREATE INDEX video_tasks_due_poll_idx
    ON video_tasks (status, next_poll_at, updated_at);

CREATE INDEX idx_entitlement_usage_entitlement_date
    ON entitlement_usage_ledgers (user_entitlement_id, usage_date);
