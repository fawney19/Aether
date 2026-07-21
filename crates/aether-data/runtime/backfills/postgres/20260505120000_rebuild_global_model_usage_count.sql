WITH aggregated AS (
    SELECT
        usage.model,
        COUNT(*)::BIGINT AS usage_count
    FROM usage_billing_facts AS usage
    WHERE usage.model IS NOT NULL
      AND BTRIM(usage.model) <> ''
      AND usage.status NOT IN ('pending', 'streaming')
    GROUP BY usage.model
),
refreshed AS (
    SELECT
        gm.id,
        COALESCE(aggregated.usage_count, 0)::BIGINT AS usage_count
    FROM global_models AS gm
    LEFT JOIN aggregated
      ON aggregated.model = gm.name
)
UPDATE global_models AS gm
SET
    usage_count = refreshed.usage_count,
    updated_at = NOW()
FROM refreshed
WHERE gm.id = refreshed.id
  AND COALESCE(gm.usage_count, 0) <> refreshed.usage_count;
