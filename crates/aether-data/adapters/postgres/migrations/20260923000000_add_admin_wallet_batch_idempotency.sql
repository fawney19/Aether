CREATE TABLE IF NOT EXISTS public.admin_user_wallet_balance_batches (
    admin_user_id character varying(64) NOT NULL,
    idempotency_key character varying(128) NOT NULL,
    request_fingerprint character varying(64) NOT NULL,
    target_user_ids jsonb NOT NULL,
    missing_user_ids jsonb NOT NULL,
    warnings jsonb NOT NULL,
    user_outcomes jsonb NOT NULL,
    created_at_unix_secs bigint NOT NULL,
    updated_at_unix_secs bigint NOT NULL,
    CONSTRAINT admin_user_wallet_balance_batches_pkey PRIMARY KEY (admin_user_id, idempotency_key)
);
