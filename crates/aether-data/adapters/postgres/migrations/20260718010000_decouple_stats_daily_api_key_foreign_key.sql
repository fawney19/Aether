-- stats_daily_api_key is a historical aggregate derived from usage snapshots.
-- Reaggregation must preserve an API Key identity after the mutable auth row is deleted.

ALTER TABLE ONLY public.stats_daily_api_key
  DROP CONSTRAINT IF EXISTS stats_daily_api_key_api_key_id_fkey;
