-- Usage is a historical fact table. Terminal usage events can arrive after
-- mutable catalog/auth rows have been disabled or deleted, so these snapshot
-- identity columns must not make ingestion depend on current dimension rows.

ALTER TABLE ONLY public.usage
  DROP CONSTRAINT IF EXISTS usage_provider_id_fkey;

ALTER TABLE ONLY public.usage
  DROP CONSTRAINT IF EXISTS usage_provider_endpoint_id_fkey;

ALTER TABLE ONLY public.usage
  DROP CONSTRAINT IF EXISTS usage_provider_api_key_id_fkey;

ALTER TABLE ONLY public.usage
  DROP CONSTRAINT IF EXISTS usage_api_key_id_fkey;

ALTER TABLE ONLY public.usage
  DROP CONSTRAINT IF EXISTS usage_user_id_fkey;

ALTER TABLE ONLY public.usage
  DROP CONSTRAINT IF EXISTS usage_wallet_id_fkey;
