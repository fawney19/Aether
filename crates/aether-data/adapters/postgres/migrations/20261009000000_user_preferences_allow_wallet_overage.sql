-- Wallet fallback is opt-in for both existing and newly created preferences.
ALTER TABLE public.user_preferences
    ADD COLUMN IF NOT EXISTS allow_wallet_overage boolean NOT NULL DEFAULT false;
