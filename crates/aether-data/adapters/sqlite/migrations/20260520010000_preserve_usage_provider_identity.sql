-- Usage is a historical fact table. Backfill nullable provider_id snapshots
-- from the unique provider name where the catalog row still exists.

UPDATE "usage"
SET provider_id = (
  SELECT providers.id
  FROM providers
  WHERE providers.name = TRIM("usage".provider_name)
  LIMIT 1
)
WHERE provider_id IS NULL
  AND TRIM(COALESCE(provider_name, '')) <> ''
  AND LOWER(TRIM(COALESCE(provider_name, ''))) NOT IN ('unknown', 'unknow', 'pending')
  AND EXISTS (
    SELECT 1
    FROM providers
    WHERE providers.name = TRIM("usage".provider_name)
  );
