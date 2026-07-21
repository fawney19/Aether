ALTER TABLE public.users
  ADD COLUMN privacy_policy_accepted_version character varying(64),
  ADD COLUMN privacy_policy_accepted_at timestamp with time zone;

ALTER TABLE public.announcements
  ADD COLUMN requires_ack boolean NOT NULL DEFAULT false;

CREATE TABLE IF NOT EXISTS public.user_invite_codes (
    user_id character varying(64) PRIMARY KEY REFERENCES public.users(id) ON DELETE CASCADE,
    invite_code character varying(64) NOT NULL UNIQUE,
    active boolean NOT NULL DEFAULT true,
    created_at timestamp with time zone NOT NULL DEFAULT NOW(),
    updated_at timestamp with time zone NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS public.user_referrals (
    id character varying(64) PRIMARY KEY,
    inviter_user_id character varying(64) NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    invitee_user_id character varying(64) NOT NULL UNIQUE REFERENCES public.users(id) ON DELETE CASCADE,
    invite_code_snapshot character varying(64) NOT NULL,
    source_json jsonb,
    first_paid_order_id character varying(64) REFERENCES public.payment_orders(id) ON DELETE SET NULL,
    first_paid_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL DEFAULT NOW(),
    updated_at timestamp with time zone NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_user_referrals_inviter
  ON public.user_referrals USING btree (inviter_user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_user_referrals_created
  ON public.user_referrals USING btree (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_user_referrals_invite_code
  ON public.user_referrals USING btree (invite_code_snapshot);

CREATE TABLE IF NOT EXISTS public.referral_rewards (
    id character varying(64) PRIMARY KEY,
    referral_id character varying(64) NOT NULL REFERENCES public.user_referrals(id) ON DELETE CASCADE,
    inviter_user_id character varying(64) NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    invitee_user_id character varying(64) NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    reward_type character varying(32) NOT NULL,
    trigger_point character varying(64) NOT NULL,
    source_order_id character varying(64) REFERENCES public.payment_orders(id) ON DELETE SET NULL,
    idempotency_key character varying(128) NOT NULL UNIQUE,
    amount_usd numeric(20,8) NOT NULL,
    status character varying(32) NOT NULL DEFAULT 'pending',
    wallet_transaction_id character varying(64),
    reversed_amount_usd numeric(20,8) NOT NULL DEFAULT 0,
    pending_reversal_amount_usd numeric(20,8) NOT NULL DEFAULT 0,
    failure_reason text,
    admin_operator_id character varying(64),
    admin_note text,
    created_at timestamp with time zone NOT NULL DEFAULT NOW(),
    updated_at timestamp with time zone NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_referral_rewards_inviter_status
  ON public.referral_rewards USING btree (inviter_user_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_referral_rewards_inviter_created
  ON public.referral_rewards USING btree (inviter_user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_referral_rewards_created
  ON public.referral_rewards USING btree (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_referral_rewards_source_order
  ON public.referral_rewards USING btree (source_order_id);
