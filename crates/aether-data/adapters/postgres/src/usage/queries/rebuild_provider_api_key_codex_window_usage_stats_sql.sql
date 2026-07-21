WITH target_keys AS (
  SELECT
    id,
    COALESCE(status_snapshot::jsonb, '{}'::jsonb) AS snapshot
  FROM provider_api_keys
  WHERE jsonb_typeof((status_snapshot::jsonb) -> 'quota' -> 'windows') = 'array'
    AND lower(BTRIM(COALESCE((status_snapshot::jsonb) -> 'quota' ->> 'provider_type', ''))) = 'codex'
),
window_items AS (
  SELECT
    target_keys.id,
    window_rows.window_item,
    window_rows.window_ordinality
  FROM target_keys
  CROSS JOIN LATERAL jsonb_array_elements(target_keys.snapshot -> 'quota' -> 'windows')
    WITH ORDINALITY AS window_rows(window_item, window_ordinality)
),
parsed_windows AS (
  SELECT
    window_items.id,
    window_items.window_item,
    window_items.window_ordinality,
    lower(BTRIM(COALESCE(window_items.window_item ->> 'code', ''))) AS window_code,
    CASE
      WHEN text_values.reset_at_text ~ '^[0-9]+$'
           AND (
             length(text_values.reset_at_text) < 19
             OR (
               length(text_values.reset_at_text) = 19
               AND text_values.reset_at_text <= '9223372036854775807'
             )
           )
      THEN text_values.reset_at_text::BIGINT
      ELSE NULL
    END AS reset_at,
    CASE
      WHEN text_values.window_minutes_text ~ '^[0-9]+$'
           AND (
             length(text_values.window_minutes_text) < 19
             OR (
               length(text_values.window_minutes_text) = 19
               AND text_values.window_minutes_text <= '9223372036854775807'
             )
           )
      THEN text_values.window_minutes_text::BIGINT
      ELSE CASE lower(BTRIM(COALESCE(window_items.window_item ->> 'code', '')))
        WHEN '5h' THEN 300
        WHEN 'weekly' THEN 10080
        ELSE NULL
      END
    END AS window_minutes,
    CASE
      WHEN text_values.usage_reset_at_text ~ '^[0-9]+$'
           AND (
             length(text_values.usage_reset_at_text) < 19
             OR (
               length(text_values.usage_reset_at_text) = 19
               AND text_values.usage_reset_at_text <= '9223372036854775807'
             )
           )
      THEN text_values.usage_reset_at_text::BIGINT
      ELSE NULL
    END AS usage_reset_at
  FROM window_items
  CROSS JOIN LATERAL (
    SELECT
      BTRIM(COALESCE(window_items.window_item ->> 'reset_at', '')) AS reset_at_text,
      BTRIM(COALESCE(window_items.window_item ->> 'window_minutes', '')) AS window_minutes_text,
      BTRIM(COALESCE(window_items.window_item ->> 'usage_reset_at', '')) AS usage_reset_at_text
  ) AS text_values
),
window_usage AS (
  SELECT
    parsed_windows.*,
    CASE
      WHEN parsed_windows.window_minutes BETWEEN 0 AND 153722867280912930
      THEN parsed_windows.window_minutes * 60
      ELSE NULL
    END AS window_seconds
  FROM parsed_windows
),
window_bounds AS (
  SELECT
    window_usage.*,
    CASE
      WHEN window_usage.window_code IN ('5h', 'weekly')
           AND window_usage.reset_at IS NOT NULL
           AND window_usage.window_seconds IS NOT NULL
           AND window_usage.reset_at >= window_usage.window_seconds
      THEN GREATEST(
        window_usage.reset_at - window_usage.window_seconds,
        COALESCE(window_usage.usage_reset_at, 0)
      )
      ELSE NULL
    END AS window_start,
    CASE
      WHEN window_usage.window_code IN ('5h', 'weekly')
           AND window_usage.reset_at IS NOT NULL
           AND window_usage.window_seconds IS NOT NULL
           AND window_usage.reset_at >= window_usage.window_seconds
      THEN window_usage.reset_at
      ELSE NULL
    END AS window_end
  FROM window_usage
),
aggregated AS (
  SELECT
    window_bounds.id,
    window_bounds.window_ordinality,
    COUNT("usage".id)::BIGINT AS request_count,
    COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)::BIGINT), 0)::BIGINT AS total_tokens,
    CAST(COALESCE(SUM(COALESCE("usage".total_cost_usd, 0)), 0) AS DOUBLE PRECISION) AS total_cost_usd
  FROM window_bounds
  LEFT JOIN usage_billing_facts AS "usage"
    ON window_bounds.window_start IS NOT NULL
   AND window_bounds.window_end IS NOT NULL
   AND "usage".provider_api_key_id = window_bounds.id
   AND "usage".created_at >= to_timestamp(window_bounds.window_start::DOUBLE PRECISION)
   AND "usage".created_at < to_timestamp(window_bounds.window_end::DOUBLE PRECISION)
  GROUP BY
    window_bounds.id,
    window_bounds.window_ordinality
),
updated_windows AS (
  SELECT
    window_bounds.id,
    jsonb_agg(
      CASE
        WHEN window_bounds.window_start IS NOT NULL
             AND window_bounds.window_end IS NOT NULL
        THEN jsonb_set(
          window_bounds.window_item,
          '{usage}',
          jsonb_build_object(
            'request_count',
            COALESCE(aggregated.request_count, 0),
            'total_tokens',
            COALESCE(aggregated.total_tokens, 0),
            'total_cost_usd',
            to_char(
              GREATEST(COALESCE(aggregated.total_cost_usd, 0), 0),
              'FM999999999999999990.00000000'
            )
          ),
          true
        )
        ELSE window_bounds.window_item
      END
      ORDER BY window_bounds.window_ordinality
    ) AS windows
  FROM window_bounds
  LEFT JOIN aggregated
    ON aggregated.id = window_bounds.id
   AND aggregated.window_ordinality = window_bounds.window_ordinality
  GROUP BY window_bounds.id
)
UPDATE provider_api_keys AS keys
SET
  status_snapshot = jsonb_set(
    target_keys.snapshot,
    '{quota,windows}',
    updated_windows.windows,
    true
  )::json,
  updated_at = NOW()
FROM target_keys
JOIN updated_windows ON updated_windows.id = target_keys.id
WHERE keys.id = target_keys.id
