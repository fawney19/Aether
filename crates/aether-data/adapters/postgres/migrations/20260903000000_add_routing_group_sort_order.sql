ALTER TABLE public.routing_groups
    -- The empty-database snapshot may already contain the current logical
    -- schema. Keep the incremental migration safe for both snapshot and
    -- legacy databases.
    ADD COLUMN IF NOT EXISTS sort_order bigint NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS routing_groups_enabled_sort_idx
    ON public.routing_groups (enabled DESC, sort_order, name, id);
