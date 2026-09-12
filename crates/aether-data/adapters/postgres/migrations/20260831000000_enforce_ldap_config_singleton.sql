-- LDAP configuration is a database-wide singleton. Preserve the row selected by the legacy
-- reader (the smallest id), remove historical duplicates, and let the database arbitrate
-- concurrent first creation.
DELETE FROM public.ldap_configs
WHERE id <> (SELECT MIN(id) FROM public.ldap_configs);

ALTER TABLE public.ldap_configs
    ADD COLUMN IF NOT EXISTS singleton_key INTEGER NOT NULL DEFAULT 1;

UPDATE public.ldap_configs
SET singleton_key = 1
WHERE singleton_key IS DISTINCT FROM 1;

ALTER TABLE public.ldap_configs
    ALTER COLUMN singleton_key SET DEFAULT 1,
    ALTER COLUMN singleton_key SET NOT NULL;

DO $migration$
BEGIN
    ALTER TABLE public.ldap_configs
        ADD CONSTRAINT ldap_configs_singleton_key_check CHECK (singleton_key = 1);
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$migration$;

DO $migration$
BEGIN
    ALTER TABLE public.ldap_configs
        ADD CONSTRAINT ldap_configs_singleton_key_key UNIQUE (singleton_key);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    -- The empty-database snapshot already materializes this constraint. PostgreSQL
    -- reports the existing constraint's backing relation as duplicate_table (42P07)
    -- rather than duplicate_object, so treat that known idempotent case the same way.
    WHEN duplicate_table THEN NULL;
END
$migration$;
