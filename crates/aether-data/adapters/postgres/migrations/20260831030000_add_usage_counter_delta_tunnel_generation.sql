ALTER TABLE public.usage_counter_deltas
    ADD COLUMN IF NOT EXISTS target_tunnel_generation character varying(64);
