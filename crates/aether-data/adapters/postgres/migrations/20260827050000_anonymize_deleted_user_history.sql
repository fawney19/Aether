ALTER TABLE public.request_candidates
    DROP CONSTRAINT IF EXISTS request_candidates_user_id_fkey;
ALTER TABLE public.video_tasks
    DROP CONSTRAINT IF EXISTS video_tasks_user_id_fkey;
ALTER TABLE public.usage
    DROP CONSTRAINT IF EXISTS usage_user_id_fkey;
ALTER TABLE public.stats_user_daily
    DROP CONSTRAINT IF EXISTS stats_user_daily_user_id_fkey;
ALTER TABLE public.stats_user_summary
    DROP CONSTRAINT IF EXISTS stats_user_summary_user_id_fkey;
ALTER TABLE public.stats_user_daily_model
    DROP CONSTRAINT IF EXISTS stats_user_daily_model_user_id_fkey;
ALTER TABLE public.stats_user_daily_provider
    DROP CONSTRAINT IF EXISTS stats_user_daily_provider_user_id_fkey;
ALTER TABLE public.stats_user_daily_api_format
    DROP CONSTRAINT IF EXISTS stats_user_daily_api_format_user_id_fkey;
ALTER TABLE public.stats_user_daily_model_provider
    DROP CONSTRAINT IF EXISTS stats_user_daily_model_provider_user_id_fkey;
ALTER TABLE public.stats_user_daily_cost_savings
    DROP CONSTRAINT IF EXISTS stats_user_daily_cost_savings_user_id_fkey;
ALTER TABLE public.stats_user_daily_cost_savings_provider
    DROP CONSTRAINT IF EXISTS stats_user_daily_cost_savings_provider_user_id_fkey;
ALTER TABLE public.stats_user_daily_cost_savings_model
    DROP CONSTRAINT IF EXISTS stats_user_daily_cost_savings_model_user_id_fkey;
ALTER TABLE public.stats_user_daily_cost_savings_model_provider
    DROP CONSTRAINT IF EXISTS stats_user_daily_cost_savings_model_provider_user_id_fkey;
ALTER TABLE public.stats_hourly_user_model
    DROP CONSTRAINT IF EXISTS stats_hourly_user_model_user_id_fkey;
ALTER TABLE public.user_model_usage_counts
    DROP CONSTRAINT IF EXISTS user_model_usage_counts_user_id_fkey;

ALTER TABLE public.audit_logs
    DROP CONSTRAINT IF EXISTS audit_logs_user_id_fkey;
ALTER TABLE public.announcements
    DROP CONSTRAINT IF EXISTS announcements_author_id_fkey;
ALTER TABLE public.payment_orders
    DROP CONSTRAINT IF EXISTS payment_orders_user_id_fkey;
ALTER TABLE public.proxy_nodes
    DROP CONSTRAINT IF EXISTS proxy_nodes_registered_by_fkey;
ALTER TABLE public.refund_requests
    DROP CONSTRAINT IF EXISTS refund_requests_user_id_fkey,
    DROP CONSTRAINT IF EXISTS refund_requests_requested_by_fkey,
    DROP CONSTRAINT IF EXISTS refund_requests_approved_by_fkey,
    DROP CONSTRAINT IF EXISTS refund_requests_processed_by_fkey;
ALTER TABLE public.wallet_transactions
    DROP CONSTRAINT IF EXISTS wallet_transactions_operator_id_fkey;
ALTER TABLE public.wallets
    DROP CONSTRAINT IF EXISTS wallets_user_id_fkey,
    DROP CONSTRAINT IF EXISTS wallets_api_key_id_fkey;
ALTER TABLE public.redeem_code_batches
    DROP CONSTRAINT IF EXISTS redeem_code_batches_created_by_fkey;
ALTER TABLE public.redeem_codes
    DROP CONSTRAINT IF EXISTS redeem_codes_redeemed_by_user_id_fkey,
    DROP CONSTRAINT IF EXISTS redeem_codes_disabled_by_fkey;

ALTER TABLE public.user_plan_entitlements
    DROP CONSTRAINT IF EXISTS user_plan_entitlements_user_id_fkey;
ALTER TABLE public.entitlement_usage_ledgers
    DROP CONSTRAINT IF EXISTS entitlement_usage_ledgers_user_id_fkey;
ALTER TABLE public.user_referrals
    DROP CONSTRAINT IF EXISTS user_referrals_inviter_user_id_fkey,
    DROP CONSTRAINT IF EXISTS user_referrals_invitee_user_id_fkey;
ALTER TABLE public.referral_rewards
    DROP CONSTRAINT IF EXISTS referral_rewards_inviter_user_id_fkey,
    DROP CONSTRAINT IF EXISTS referral_rewards_invitee_user_id_fkey;

ALTER TABLE public.request_candidates
    DROP CONSTRAINT IF EXISTS request_candidates_api_key_id_fkey;
ALTER TABLE public.video_tasks
    DROP CONSTRAINT IF EXISTS video_tasks_api_key_id_fkey;
ALTER TABLE public.usage
    DROP CONSTRAINT IF EXISTS usage_api_key_id_fkey;
ALTER TABLE public.stats_daily_api_key
    DROP CONSTRAINT IF EXISTS stats_daily_api_key_api_key_id_fkey;

-- Legacy rows are intentionally retained; current writes enforce the policy.
