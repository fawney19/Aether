ALTER TABLE public.provider_api_keys
ADD COLUMN IF NOT EXISTS allow_auth_channel_mismatch_formats json;

ALTER TABLE public.provider_api_keys
ADD COLUMN IF NOT EXISTS concurrent_limit integer;

CREATE OR REPLACE FUNCTION public.aether_default_auth_mismatch_api_format(value text)
RETURNS text
LANGUAGE sql
IMMUTABLE
AS $$
  SELECT CASE LOWER(BTRIM(COALESCE(value, '')))
    WHEN 'openai:cli' THEN 'openai:responses'
    WHEN 'openai:compact' THEN 'openai:responses:compact'
    WHEN 'claude:chat' THEN 'claude:messages'
    WHEN 'claude:cli' THEN 'claude:messages'
    WHEN 'gemini:chat' THEN 'gemini:generate_content'
    WHEN 'gemini:cli' THEN 'gemini:generate_content'
    ELSE LOWER(BTRIM(COALESCE(value, '')))
  END
$$;

WITH supported_formats AS (
  SELECT
    pak.id,
    public.aether_default_auth_mismatch_api_format(format.value) AS api_format,
    0 AS source_priority,
    MIN(format.ordinality) AS first_ordinality
  FROM public.provider_api_keys AS pak
  CROSS JOIN LATERAL json_array_elements_text(
    CASE
      WHEN pak.api_formats IS NOT NULL
        AND json_typeof(pak.api_formats) = 'array'
      THEN pak.api_formats
      ELSE '[]'::json
    END
  ) WITH ORDINALITY AS format(value, ordinality)
  WHERE pak.api_formats IS NOT NULL
    AND json_typeof(pak.api_formats) = 'array'
  GROUP BY pak.id, api_format

  UNION ALL

  SELECT
    pak.id,
    public.aether_default_auth_mismatch_api_format(endpoint.api_format) AS api_format,
    1 AS source_priority,
    0 AS first_ordinality
  FROM public.provider_api_keys AS pak
  INNER JOIN public.provider_endpoints AS endpoint
    ON endpoint.provider_id = pak.provider_id
  WHERE pak.api_formats IS NULL
    OR json_typeof(pak.api_formats) <> 'array'
),
deduplicated_formats AS (
  SELECT
    id,
    api_format,
    MIN(source_priority) AS source_priority,
    MIN(first_ordinality) AS first_ordinality
  FROM supported_formats
  WHERE api_format <> ''
  GROUP BY id, api_format
),
rebuilt AS (
  SELECT
    id,
    json_agg(api_format ORDER BY source_priority, first_ordinality, api_format) AS api_formats
  FROM deduplicated_formats
  GROUP BY id
)
UPDATE public.provider_api_keys AS pak
SET
  allow_auth_channel_mismatch_formats = rebuilt.api_formats,
  updated_at = NOW()
FROM rebuilt
WHERE pak.id = rebuilt.id
  AND pak.allow_auth_channel_mismatch_formats IS NULL;

DROP FUNCTION public.aether_default_auth_mismatch_api_format(text);
