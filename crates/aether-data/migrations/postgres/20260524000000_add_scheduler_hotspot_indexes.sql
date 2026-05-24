CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_active_priority_id
    ON public.provider_api_keys USING btree (provider_id, internal_priority, id)
    WHERE is_active IS TRUE;

CREATE INDEX IF NOT EXISTS pool_member_scores_scheduler_account_rank_idx
    ON public.pool_member_scores USING btree (
        pool_kind,
        pool_id,
        capability,
        scope_kind,
        score DESC,
        last_ranked_at DESC NULLS LAST,
        member_id,
        id
    )
    WHERE scope_id IS NULL AND hard_state IN ('available', 'unknown');
