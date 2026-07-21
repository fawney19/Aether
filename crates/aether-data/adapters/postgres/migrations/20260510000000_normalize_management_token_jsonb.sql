ALTER TABLE public.management_tokens
    DROP CONSTRAINT IF EXISTS check_allowed_ips_not_empty;

ALTER TABLE public.management_tokens
    ADD COLUMN IF NOT EXISTS permissions jsonb;

ALTER TABLE public.management_tokens
    ALTER COLUMN allowed_ips TYPE jsonb USING allowed_ips::jsonb,
    ALTER COLUMN permissions TYPE jsonb USING permissions::jsonb;

ALTER TABLE public.management_tokens
    ADD CONSTRAINT check_allowed_ips_not_empty CHECK (
        CASE
            WHEN allowed_ips IS NULL OR allowed_ips = 'null'::jsonb THEN TRUE
            WHEN jsonb_typeof(allowed_ips) = 'array' THEN jsonb_array_length(allowed_ips) > 0
            ELSE FALSE
        END
    );
