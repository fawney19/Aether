ALTER TABLE users
  ADD COLUMN privacy_policy_accepted_version VARCHAR(64),
  ADD COLUMN privacy_policy_accepted_at BIGINT;

ALTER TABLE announcements
  ADD COLUMN requires_ack BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE IF NOT EXISTS user_invite_codes (
    user_id VARCHAR(64) PRIMARY KEY,
    invite_code VARCHAR(64) NOT NULL UNIQUE,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    CONSTRAINT user_invite_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS user_referrals (
    id VARCHAR(64) PRIMARY KEY,
    inviter_user_id VARCHAR(64) NOT NULL,
    invitee_user_id VARCHAR(64) NOT NULL UNIQUE,
    invite_code_snapshot VARCHAR(64) NOT NULL,
    source_json TEXT,
    first_paid_order_id VARCHAR(64),
    first_paid_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    KEY idx_user_referrals_inviter (inviter_user_id, created_at),
    KEY idx_user_referrals_created (created_at),
    KEY idx_user_referrals_invite_code (invite_code_snapshot),
    CONSTRAINT user_referrals_inviter_user_id_fkey FOREIGN KEY (inviter_user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT user_referrals_invitee_user_id_fkey FOREIGN KEY (invitee_user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT user_referrals_first_paid_order_fkey FOREIGN KEY (first_paid_order_id) REFERENCES payment_orders(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS referral_rewards (
    id VARCHAR(64) PRIMARY KEY,
    referral_id VARCHAR(64) NOT NULL,
    inviter_user_id VARCHAR(64) NOT NULL,
    invitee_user_id VARCHAR(64) NOT NULL,
    reward_type VARCHAR(32) NOT NULL,
    trigger_point VARCHAR(64) NOT NULL,
    source_order_id VARCHAR(64),
    idempotency_key VARCHAR(128) NOT NULL UNIQUE,
    amount_usd DOUBLE NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'pending',
    wallet_transaction_id VARCHAR(64),
    reversed_amount_usd DOUBLE NOT NULL DEFAULT 0,
    pending_reversal_amount_usd DOUBLE NOT NULL DEFAULT 0,
    failure_reason TEXT,
    admin_operator_id VARCHAR(64),
    admin_note TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    KEY idx_referral_rewards_inviter_status (inviter_user_id, status, created_at),
    KEY idx_referral_rewards_inviter_created (inviter_user_id, created_at),
    KEY idx_referral_rewards_created (created_at),
    KEY idx_referral_rewards_source_order (source_order_id),
    CONSTRAINT referral_rewards_referral_id_fkey FOREIGN KEY (referral_id) REFERENCES user_referrals(id) ON DELETE CASCADE,
    CONSTRAINT referral_rewards_inviter_user_id_fkey FOREIGN KEY (inviter_user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT referral_rewards_invitee_user_id_fkey FOREIGN KEY (invitee_user_id) REFERENCES users(id) ON DELETE CASCADE,
    CONSTRAINT referral_rewards_source_order_fkey FOREIGN KEY (source_order_id) REFERENCES payment_orders(id) ON DELETE SET NULL
);
