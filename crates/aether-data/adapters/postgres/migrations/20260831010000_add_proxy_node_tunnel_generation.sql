ALTER TABLE public.proxy_nodes
    ADD COLUMN IF NOT EXISTS tunnel_generation character varying(64);

-- The generation is an opaque epoch marker used to reject stale mutations; it
-- is not a credential.  Keep this migration independent of the optional
-- pgcrypto extension.  The registration path uses a CSPRNG for new rows,
-- while this backfill combines the immutable row identity, physical tuple,
-- transaction time, and PostgreSQL's per-call PRNG to produce a distinct
-- marker for every legacy row using only core functions.
UPDATE public.proxy_nodes
SET tunnel_generation = md5(
    id || ':' ||
    ctid::text || ':' ||
    clock_timestamp()::text || ':' ||
    random()::text
)
WHERE tunnel_generation IS NULL OR btrim(tunnel_generation) = '';

ALTER TABLE public.proxy_nodes
    ALTER COLUMN tunnel_generation SET NOT NULL;
