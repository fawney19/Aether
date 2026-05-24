CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_active_priority_id
    ON provider_api_keys (provider_id, is_active, internal_priority, id);

CREATE INDEX IF NOT EXISTS pool_member_scores_scheduler_account_rank_idx
    ON pool_member_scores (
        pool_kind,
        pool_id,
        capability,
        scope_kind,
        scope_id,
        hard_state,
        score DESC,
        last_ranked_at DESC,
        member_id,
        id
    );
