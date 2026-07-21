CREATE TABLE IF NOT EXISTS public.redeem_code_batches (
    id character varying(36) NOT NULL,
    name character varying(120) NOT NULL,
    amount_usd numeric(20,8) NOT NULL,
    currency character varying(3) DEFAULT 'USD'::character varying NOT NULL,
    balance_bucket character varying(20) DEFAULT 'gift'::character varying NOT NULL,
    total_count integer NOT NULL,
    status character varying(20) DEFAULT 'active'::character varying NOT NULL,
    description text,
    created_by character varying(36),
    expires_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT ck_redeem_code_batches_amount_positive CHECK ((amount_usd > (0)::numeric)),
    CONSTRAINT ck_redeem_code_batches_total_count_positive CHECK ((total_count > 0))
);

CREATE TABLE IF NOT EXISTS public.redeem_codes (
    id character varying(36) NOT NULL,
    batch_id character varying(36) NOT NULL,
    code_hash character varying(64) NOT NULL,
    code_prefix character varying(8) NOT NULL,
    code_suffix character varying(8) NOT NULL,
    status character varying(20) DEFAULT 'active'::character varying NOT NULL,
    redeemed_by_user_id character varying(36),
    redeemed_wallet_id character varying(36),
    redeemed_payment_order_id character varying(36),
    redeemed_at timestamp with time zone,
    disabled_by character varying(36),
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL
);

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_code_batches
    ADD CONSTRAINT redeem_code_batches_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT uq_redeem_codes_code_hash UNIQUE (code_hash);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;

CREATE INDEX IF NOT EXISTS idx_redeem_code_batches_status
ON public.redeem_code_batches USING btree (status, created_at);

CREATE INDEX IF NOT EXISTS idx_redeem_codes_batch_created
ON public.redeem_codes USING btree (batch_id, created_at);

CREATE INDEX IF NOT EXISTS idx_redeem_codes_status
ON public.redeem_codes USING btree (status, updated_at);

CREATE INDEX IF NOT EXISTS idx_redeem_codes_redeemed_user
ON public.redeem_codes USING btree (redeemed_by_user_id, redeemed_at);

CREATE INDEX IF NOT EXISTS idx_redeem_codes_redeemed_order
ON public.redeem_codes USING btree (redeemed_payment_order_id);

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_code_batches
    ADD CONSTRAINT redeem_code_batches_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
END $mig$;

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_batch_id_fkey FOREIGN KEY (batch_id) REFERENCES public.redeem_code_batches(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
END $mig$;

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_redeemed_by_user_id_fkey FOREIGN KEY (redeemed_by_user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
END $mig$;

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_redeemed_wallet_id_fkey FOREIGN KEY (redeemed_wallet_id) REFERENCES public.wallets(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
END $mig$;

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_redeemed_payment_order_id_fkey FOREIGN KEY (redeemed_payment_order_id) REFERENCES public.payment_orders(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
END $mig$;

DO $mig$ BEGIN
  ALTER TABLE ONLY public.redeem_codes
    ADD CONSTRAINT redeem_codes_disabled_by_fkey FOREIGN KEY (disabled_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
END $mig$;

ALTER TABLE public.stats_user_daily
    ADD COLUMN IF NOT EXISTS actual_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_samples bigint DEFAULT 0 NOT NULL,
    ADD COLUMN IF NOT EXISTS effective_input_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS total_input_context bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_creation_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_read_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL;

ALTER TABLE public.stats_hourly_user
    ADD COLUMN IF NOT EXISTS cache_creation_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS cache_read_tokens bigint DEFAULT '0'::bigint NOT NULL,
    ADD COLUMN IF NOT EXISTS actual_total_cost numeric(20,8) DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_sum_ms double precision DEFAULT '0'::double precision NOT NULL,
    ADD COLUMN IF NOT EXISTS response_time_samples bigint DEFAULT 0 NOT NULL;
