ALTER TABLE users ADD COLUMN privacy_policy_accepted_version TEXT;
ALTER TABLE users ADD COLUMN privacy_policy_accepted_at INTEGER;

ALTER TABLE announcements ADD COLUMN requires_ack INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS user_invite_codes (
    user_id TEXT PRIMARY KEY,
    invite_code TEXT NOT NULL UNIQUE,
    active INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS user_referrals (
    id TEXT PRIMARY KEY,
    inviter_user_id TEXT NOT NULL,
    invitee_user_id TEXT NOT NULL UNIQUE,
    invite_code_snapshot TEXT NOT NULL,
    source_json TEXT,
    first_paid_order_id TEXT,
    first_paid_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(inviter_user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(invitee_user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(first_paid_order_id) REFERENCES payment_orders(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_user_referrals_inviter
  ON user_referrals (inviter_user_id, created_at);
CREATE INDEX IF NOT EXISTS idx_user_referrals_created
  ON user_referrals (created_at);
CREATE INDEX IF NOT EXISTS idx_user_referrals_invite_code
  ON user_referrals (invite_code_snapshot);

CREATE TABLE IF NOT EXISTS referral_rewards (
    id TEXT PRIMARY KEY,
    referral_id TEXT NOT NULL,
    inviter_user_id TEXT NOT NULL,
    invitee_user_id TEXT NOT NULL,
    reward_type TEXT NOT NULL,
    trigger_point TEXT NOT NULL,
    source_order_id TEXT,
    idempotency_key TEXT NOT NULL UNIQUE,
    amount_usd REAL NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    wallet_transaction_id TEXT,
    reversed_amount_usd REAL NOT NULL DEFAULT 0,
    pending_reversal_amount_usd REAL NOT NULL DEFAULT 0,
    failure_reason TEXT,
    admin_operator_id TEXT,
    admin_note TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(referral_id) REFERENCES user_referrals(id) ON DELETE CASCADE,
    FOREIGN KEY(inviter_user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(invitee_user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(source_order_id) REFERENCES payment_orders(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_referral_rewards_inviter_status
  ON referral_rewards (inviter_user_id, status, created_at);
CREATE INDEX IF NOT EXISTS idx_referral_rewards_inviter_created
  ON referral_rewards (inviter_user_id, created_at);
CREATE INDEX IF NOT EXISTS idx_referral_rewards_created
  ON referral_rewards (created_at);
CREATE INDEX IF NOT EXISTS idx_referral_rewards_source_order
  ON referral_rewards (source_order_id);
