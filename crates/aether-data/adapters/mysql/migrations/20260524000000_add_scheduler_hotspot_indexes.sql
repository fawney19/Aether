SET @aether_provider_key_active_priority_index_sql := IF(
    (
        SELECT COUNT(*)
        FROM information_schema.statistics
        WHERE table_schema = DATABASE()
          AND table_name = 'provider_api_keys'
          AND index_name = 'idx_provider_api_keys_provider_active_priority_id'
    ) = 0,
    'CREATE INDEX idx_provider_api_keys_provider_active_priority_id ON provider_api_keys (provider_id, is_active, internal_priority, id)',
    'DO 0'
);

PREPARE aether_provider_key_active_priority_index_stmt FROM @aether_provider_key_active_priority_index_sql;
EXECUTE aether_provider_key_active_priority_index_stmt;
DEALLOCATE PREPARE aether_provider_key_active_priority_index_stmt;

SET @aether_pool_score_scheduler_rank_index_sql := IF(
    (
        SELECT COUNT(*)
        FROM information_schema.statistics
        WHERE table_schema = DATABASE()
          AND table_name = 'pool_member_scores'
          AND index_name = 'pool_member_scores_scheduler_account_rank_idx'
    ) = 0,
    'CREATE INDEX pool_member_scores_scheduler_account_rank_idx ON pool_member_scores (pool_kind, pool_id, capability, scope_kind, scope_id, hard_state, score DESC, last_ranked_at DESC, member_id, id)',
    'DO 0'
);

PREPARE aether_pool_score_scheduler_rank_index_stmt FROM @aether_pool_score_scheduler_rank_index_sql;
EXECUTE aether_pool_score_scheduler_rank_index_stmt;
DEALLOCATE PREPARE aether_pool_score_scheduler_rank_index_stmt;
