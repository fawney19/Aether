CREATE TABLE IF NOT EXISTS public.pool_member_scores (
    id character varying(192) PRIMARY KEY,
    pool_kind character varying(64) NOT NULL,
    pool_id character varying(64) NOT NULL,
    member_kind character varying(64) NOT NULL,
    member_id character varying(64) NOT NULL,
    capability character varying(64) NOT NULL,
    scope_kind character varying(64) NOT NULL,
    scope_id character varying(128),
    score double precision NOT NULL DEFAULT 0,
    hard_state character varying(64) NOT NULL DEFAULT 'unknown',
    score_version bigint NOT NULL DEFAULT 1,
    score_reason jsonb NOT NULL,
    last_ranked_at bigint,
    last_scheduled_at bigint,
    last_success_at bigint,
    last_failure_at bigint,
    failure_count bigint NOT NULL DEFAULT 0,
    last_probe_attempt_at bigint,
    last_probe_success_at bigint,
    last_probe_failure_at bigint,
    probe_failure_count bigint NOT NULL DEFAULT 0,
    probe_status character varying(64) NOT NULL DEFAULT 'never',
    updated_at bigint NOT NULL
);

CREATE INDEX IF NOT EXISTS pool_member_scores_rank_idx
    ON public.pool_member_scores USING btree
    (pool_kind, pool_id, capability, scope_kind, scope_id, hard_state, score DESC);

CREATE INDEX IF NOT EXISTS pool_member_scores_member_idx
    ON public.pool_member_scores USING btree
    (pool_kind, pool_id, member_kind, member_id);

CREATE INDEX IF NOT EXISTS pool_member_scores_probe_idx
    ON public.pool_member_scores USING btree
    (pool_kind, pool_id, probe_status, last_probe_success_at);

CREATE INDEX IF NOT EXISTS pool_member_scores_updated_at_idx
    ON public.pool_member_scores USING btree (updated_at);
