CREATE TABLE IF NOT EXISTS pool_member_scores (
    id TEXT PRIMARY KEY,
    pool_kind TEXT NOT NULL,
    pool_id TEXT NOT NULL,
    member_kind TEXT NOT NULL,
    member_id TEXT NOT NULL,
    capability TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_id TEXT,
    score REAL NOT NULL DEFAULT 0,
    hard_state TEXT NOT NULL DEFAULT 'unknown',
    score_version INTEGER NOT NULL DEFAULT 1,
    score_reason TEXT NOT NULL,
    last_ranked_at INTEGER,
    last_scheduled_at INTEGER,
    last_success_at INTEGER,
    last_failure_at INTEGER,
    failure_count INTEGER NOT NULL DEFAULT 0,
    last_probe_attempt_at INTEGER,
    last_probe_success_at INTEGER,
    last_probe_failure_at INTEGER,
    probe_failure_count INTEGER NOT NULL DEFAULT 0,
    probe_status TEXT NOT NULL DEFAULT 'never',
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS pool_member_scores_rank_idx
    ON pool_member_scores (pool_kind, pool_id, capability, scope_kind, scope_id, hard_state, score DESC);

CREATE INDEX IF NOT EXISTS pool_member_scores_member_idx
    ON pool_member_scores (pool_kind, pool_id, member_kind, member_id);

CREATE INDEX IF NOT EXISTS pool_member_scores_probe_idx
    ON pool_member_scores (pool_kind, pool_id, probe_status, last_probe_success_at);

CREATE INDEX IF NOT EXISTS pool_member_scores_updated_at_idx
    ON pool_member_scores (updated_at);
