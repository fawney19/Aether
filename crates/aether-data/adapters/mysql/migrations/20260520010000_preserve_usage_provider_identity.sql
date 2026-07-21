-- Usage is a historical fact table. Backfill nullable provider_id snapshots
-- from the unique provider name where the catalog row still exists.

UPDATE `usage` AS usage_rows
JOIN providers
  ON providers.name = TRIM(usage_rows.provider_name)
SET usage_rows.provider_id = providers.id
WHERE usage_rows.provider_id IS NULL
  AND TRIM(COALESCE(usage_rows.provider_name, '')) <> ''
  AND LOWER(TRIM(COALESCE(usage_rows.provider_name, ''))) NOT IN ('unknown', 'unknow', 'pending');
