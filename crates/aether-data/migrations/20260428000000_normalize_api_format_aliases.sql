CREATE OR REPLACE FUNCTION public.aether_canonical_api_format_alias(value text)
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

CREATE OR REPLACE FUNCTION public.aether_legacy_raw_api_format_auth_type(value text)
RETURNS text
LANGUAGE sql
IMMUTABLE
AS $$
  SELECT CASE LOWER(BTRIM(COALESCE(value, '')))
    WHEN 'claude:chat' THEN 'api_key'
    WHEN 'claude:cli' THEN 'bearer'
    WHEN 'gemini:chat' THEN 'api_key'
    WHEN 'gemini:cli' THEN 'bearer'
    ELSE NULL
  END
$$;

ALTER TABLE IF EXISTS public.provider_endpoints
  DROP CONSTRAINT IF EXISTS uq_provider_api_format;

CREATE INDEX IF NOT EXISTS idx_provider_endpoints_provider_api_format
  ON public.provider_endpoints USING btree (provider_id, api_format);

ALTER TABLE IF EXISTS public.provider_api_keys
  ADD COLUMN IF NOT EXISTS auth_type_by_format json;

ALTER TABLE IF EXISTS public.provider_api_keys
  ALTER COLUMN api_key DROP NOT NULL;

WITH legacy_key_formats AS (
  SELECT
    pak.id,
    LOWER(BTRIM(COALESCE(provider.provider_type, ''))) AS provider_type,
    LOWER(BTRIM(COALESCE(pak.auth_type, ''))) AS current_auth_type,
    COALESCE(BOOL_OR(LOWER(BTRIM(format.value)) = 'claude:chat'), false) AS has_claude_chat,
    COALESCE(BOOL_OR(LOWER(BTRIM(format.value)) = 'claude:cli'), false) AS has_claude_cli,
    COALESCE(BOOL_OR(LOWER(BTRIM(format.value)) = 'gemini:chat'), false) AS has_gemini_chat,
    COALESCE(BOOL_OR(LOWER(BTRIM(format.value)) = 'gemini:cli'), false) AS has_gemini_cli
  FROM public.provider_api_keys AS pak
  INNER JOIN public.providers AS provider
    ON provider.id = pak.provider_id
  LEFT JOIN LATERAL (
    SELECT key_format.value
    FROM json_array_elements_text(
      CASE
        WHEN pak.api_formats IS NOT NULL
          AND json_typeof(pak.api_formats) = 'array'
        THEN pak.api_formats
        ELSE '[]'::json
      END
    ) AS key_format(value)
    UNION ALL
    SELECT endpoint.api_format AS value
    FROM public.provider_endpoints AS endpoint
    WHERE pak.api_formats IS NULL
      AND endpoint.provider_id = pak.provider_id
  ) AS format ON true
  GROUP BY pak.id, provider_type, current_auth_type
),
inferred_key_auth AS (
  SELECT
    id,
    current_auth_type,
    CASE
      WHEN provider_type IN ('claude_code', 'kiro')
        AND has_claude_cli
      THEN 'oauth'
      WHEN provider_type IN ('gemini_cli', 'antigravity')
        AND has_gemini_cli
      THEN 'oauth'
      WHEN provider_type NOT IN ('claude_code', 'codex', 'gemini_cli', 'vertex_ai', 'antigravity', 'kiro')
        AND (has_claude_chat OR has_gemini_chat)
        AND NOT (has_claude_cli OR has_gemini_cli)
      THEN 'api_key'
      ELSE NULL
    END AS inferred_auth_type
  FROM legacy_key_formats
)
UPDATE public.provider_api_keys AS pak
SET
  auth_type = inferred.inferred_auth_type,
  updated_at = NOW()
FROM inferred_key_auth AS inferred
WHERE pak.id = inferred.id
  AND inferred.inferred_auth_type IS NOT NULL
  AND inferred.current_auth_type IS DISTINCT FROM inferred.inferred_auth_type
  AND (
    (
      inferred.inferred_auth_type = 'oauth'
      AND inferred.current_auth_type IN ('', 'api_key', 'bearer')
    )
    OR (
      inferred.inferred_auth_type = 'api_key'
      AND inferred.current_auth_type IN ('', 'bearer')
    )
  );

WITH existing_auth_entries AS (
  SELECT
    pak.id,
    public.aether_canonical_api_format_alias(entry.key) AS api_format,
    CASE LOWER(BTRIM(COALESCE(entry.value #>> '{}', '')))
      WHEN 'api_key' THEN 'api_key'
      WHEN 'apikey' THEN 'api_key'
      WHEN 'api-key' THEN 'api_key'
      WHEN 'bearer' THEN 'bearer'
      WHEN 'bearer_token' THEN 'bearer'
      WHEN 'bearer-token' THEN 'bearer'
      WHEN 'authorization' THEN 'bearer'
      ELSE NULL
    END AS auth_type,
    0 AS source_priority,
    entry.ordinality
  FROM public.provider_api_keys AS pak
  CROSS JOIN LATERAL json_each(
    CASE
      WHEN json_typeof(pak.auth_type_by_format) = 'object' THEN pak.auth_type_by_format
      ELSE '{}'::json
    END
  ) WITH ORDINALITY AS entry(key, value, ordinality)
  WHERE pak.auth_type_by_format IS NOT NULL
),
legacy_auth_entries AS (
  SELECT
    pak.id,
    public.aether_canonical_api_format_alias(format.value) AS api_format,
    MIN(public.aether_legacy_raw_api_format_auth_type(format.value)) AS auth_type,
    1 AS source_priority,
    MIN(format.ordinality) AS ordinality
  FROM public.provider_api_keys AS pak
  LEFT JOIN LATERAL (
    SELECT key_format.value, key_format.ordinality
    FROM json_array_elements_text(
      CASE
        WHEN pak.api_formats IS NOT NULL
          AND json_typeof(pak.api_formats) = 'array'
        THEN pak.api_formats
        ELSE '[]'::json
      END
    ) WITH ORDINALITY AS key_format(value, ordinality)
    UNION ALL
    SELECT endpoint.api_format AS value, endpoint.ordinality
    FROM (
      SELECT endpoint.api_format, ROW_NUMBER() OVER (ORDER BY endpoint.id) AS ordinality
      FROM public.provider_endpoints AS endpoint
      WHERE pak.api_formats IS NULL
        AND endpoint.provider_id = pak.provider_id
    ) AS endpoint
  ) AS format ON true
  WHERE LOWER(BTRIM(COALESCE(pak.auth_type, ''))) IN ('api_key', 'bearer')
    AND public.aether_legacy_raw_api_format_auth_type(format.value) IS NOT NULL
  GROUP BY
    pak.id,
    public.aether_canonical_api_format_alias(format.value)
  HAVING COUNT(DISTINCT public.aether_legacy_raw_api_format_auth_type(format.value)) = 1
),
auth_entries AS (
  SELECT id, api_format, auth_type, source_priority, ordinality
  FROM existing_auth_entries
  WHERE api_format <> ''
    AND auth_type IS NOT NULL
  UNION ALL
  SELECT
    legacy.id,
    legacy.api_format,
    legacy.auth_type,
    legacy.source_priority,
    legacy.ordinality
  FROM legacy_auth_entries AS legacy
  INNER JOIN public.provider_api_keys AS pak
    ON pak.id = legacy.id
  WHERE legacy.api_format <> ''
    AND legacy.auth_type IS NOT NULL
    AND legacy.auth_type IS DISTINCT FROM LOWER(BTRIM(COALESCE(pak.auth_type, '')))
),
ranked AS (
  SELECT
    id,
    api_format,
    auth_type,
    source_priority,
    ordinality,
    ROW_NUMBER() OVER (
      PARTITION BY id, api_format
      ORDER BY source_priority, ordinality
    ) AS rank
  FROM auth_entries
),
rebuilt AS (
  SELECT
    id,
    jsonb_object_agg(api_format, to_jsonb(auth_type) ORDER BY source_priority, ordinality) AS auth_type_by_format
  FROM ranked
  WHERE rank = 1
  GROUP BY id
)
UPDATE public.provider_api_keys AS pak
SET
  auth_type_by_format = rebuilt.auth_type_by_format::json,
  updated_at = NOW()
FROM rebuilt
WHERE pak.id = rebuilt.id
  AND pak.auth_type_by_format::jsonb IS DISTINCT FROM rebuilt.auth_type_by_format;

WITH normalized AS (
  SELECT
    id,
    public.aether_canonical_api_format_alias(api_format) AS canonical_api_format
  FROM public.provider_endpoints
  WHERE api_format IN ('openai:responses', 'openai:cli', 'openai:responses:compact', 'openai:compact', 'claude:messages', 'claude:chat', 'claude:cli', 'gemini:generate_content', 'gemini:chat', 'gemini:cli')
)
UPDATE public.provider_endpoints AS endpoint
SET
  api_format = normalized.canonical_api_format,
  api_family = SPLIT_PART(normalized.canonical_api_format, ':', 1),
  endpoint_kind = SUBSTRING(normalized.canonical_api_format FROM POSITION(':' IN normalized.canonical_api_format) + 1),
  updated_at = NOW()
FROM normalized
WHERE endpoint.id = normalized.id
  AND (
    endpoint.api_format IS DISTINCT FROM normalized.canonical_api_format
    OR endpoint.api_family IS DISTINCT FROM SPLIT_PART(normalized.canonical_api_format, ':', 1)
    OR endpoint.endpoint_kind IS DISTINCT FROM SUBSTRING(normalized.canonical_api_format FROM POSITION(':' IN normalized.canonical_api_format) + 1)
  );

WITH expanded AS (
  SELECT
    pak.id,
    formats.ordinality,
    public.aether_canonical_api_format_alias(formats.value) AS api_format
  FROM public.provider_api_keys AS pak
  CROSS JOIN LATERAL json_array_elements_text(
    CASE
      WHEN json_typeof(pak.api_formats) = 'array' THEN pak.api_formats
      ELSE '[]'::json
    END
  ) WITH ORDINALITY AS formats(value, ordinality)
  WHERE pak.api_formats IS NOT NULL
),
deduped AS (
  SELECT id, api_format, MIN(ordinality) AS first_ordinality
  FROM expanded
  WHERE api_format <> ''
  GROUP BY id, api_format
),
rebuilt AS (
  SELECT id, json_agg(api_format ORDER BY first_ordinality) AS api_formats
  FROM deduped
  GROUP BY id
)
UPDATE public.provider_api_keys AS pak
SET
  api_formats = rebuilt.api_formats,
  updated_at = NOW()
FROM rebuilt
WHERE pak.id = rebuilt.id
  AND pak.api_formats::jsonb IS DISTINCT FROM rebuilt.api_formats::jsonb;

WITH expanded AS (
  SELECT
    key.id,
    formats.ordinality,
    public.aether_canonical_api_format_alias(formats.value) AS api_format
  FROM public.api_keys AS key
  CROSS JOIN LATERAL json_array_elements_text(
    CASE
      WHEN json_typeof(key.allowed_api_formats) = 'array' THEN key.allowed_api_formats
      ELSE '[]'::json
    END
  ) WITH ORDINALITY AS formats(value, ordinality)
  WHERE key.allowed_api_formats IS NOT NULL
),
deduped AS (
  SELECT id, api_format, MIN(ordinality) AS first_ordinality
  FROM expanded
  WHERE api_format <> ''
  GROUP BY id, api_format
),
rebuilt AS (
  SELECT id, json_agg(api_format ORDER BY first_ordinality) AS allowed_api_formats
  FROM deduped
  GROUP BY id
)
UPDATE public.api_keys AS key
SET
  allowed_api_formats = rebuilt.allowed_api_formats,
  updated_at = NOW()
FROM rebuilt
WHERE key.id = rebuilt.id
  AND key.allowed_api_formats::jsonb IS DISTINCT FROM rebuilt.allowed_api_formats::jsonb;

WITH expanded AS (
  SELECT
    users.id,
    formats.ordinality,
    public.aether_canonical_api_format_alias(formats.value) AS api_format
  FROM public.users AS users
  CROSS JOIN LATERAL json_array_elements_text(
    CASE
      WHEN json_typeof(users.allowed_api_formats) = 'array' THEN users.allowed_api_formats
      ELSE '[]'::json
    END
  ) WITH ORDINALITY AS formats(value, ordinality)
  WHERE users.allowed_api_formats IS NOT NULL
),
deduped AS (
  SELECT id, api_format, MIN(ordinality) AS first_ordinality
  FROM expanded
  WHERE api_format <> ''
  GROUP BY id, api_format
),
rebuilt AS (
  SELECT id, json_agg(api_format ORDER BY first_ordinality) AS allowed_api_formats
  FROM deduped
  GROUP BY id
)
UPDATE public.users AS users
SET
  allowed_api_formats = rebuilt.allowed_api_formats,
  updated_at = NOW()
FROM rebuilt
WHERE users.id = rebuilt.id
  AND users.allowed_api_formats::jsonb IS DISTINCT FROM rebuilt.allowed_api_formats::jsonb;

WITH mapping_items AS (
  SELECT
    models.id,
    item.ordinality AS item_ordinality,
    item.value AS item
  FROM public.models AS models
  CROSS JOIN LATERAL jsonb_array_elements(
    CASE
      WHEN jsonb_typeof(models.provider_model_mappings) = 'array' THEN models.provider_model_mappings
      ELSE '[]'::jsonb
    END
  ) WITH ORDINALITY AS item(value, ordinality)
  WHERE models.provider_model_mappings IS NOT NULL
),
rebuilt_items AS (
  SELECT
    id,
    item_ordinality,
    CASE
      WHEN jsonb_typeof(item) = 'object'
        AND jsonb_typeof(item->'api_formats') = 'array'
      THEN jsonb_set(
        item,
        '{api_formats}',
        COALESCE(
          (
            SELECT jsonb_agg(api_format ORDER BY first_ordinality)
            FROM (
              SELECT
                public.aether_canonical_api_format_alias(format.value) AS api_format,
                MIN(format.ordinality) AS first_ordinality
              FROM jsonb_array_elements_text(item->'api_formats') WITH ORDINALITY AS format(value, ordinality)
              GROUP BY public.aether_canonical_api_format_alias(format.value)
            ) AS deduped_formats
            WHERE api_format <> ''
          ),
          '[]'::jsonb
        ),
        true
      )
      ELSE item
    END AS item
  FROM mapping_items
),
rebuilt AS (
  SELECT id, jsonb_agg(item ORDER BY item_ordinality) AS provider_model_mappings
  FROM rebuilt_items
  GROUP BY id
)
UPDATE public.models AS models
SET
  provider_model_mappings = rebuilt.provider_model_mappings,
  updated_at = NOW()
FROM rebuilt
WHERE models.id = rebuilt.id
  AND models.provider_model_mappings IS DISTINCT FROM rebuilt.provider_model_mappings;

WITH rebuilt AS (
  SELECT
    pak.id,
    jsonb_object_agg(
      public.aether_canonical_api_format_alias(entry.key),
      entry.value
      ORDER BY entry.ordinality
    ) AS rate_multipliers
  FROM public.provider_api_keys AS pak
  CROSS JOIN LATERAL json_each(
    CASE
      WHEN json_typeof(pak.rate_multipliers) = 'object' THEN pak.rate_multipliers
      ELSE '{}'::json
    END
  ) WITH ORDINALITY AS entry(key, value, ordinality)
  WHERE pak.rate_multipliers IS NOT NULL
  GROUP BY pak.id
)
UPDATE public.provider_api_keys AS pak
SET
  rate_multipliers = rebuilt.rate_multipliers::json,
  updated_at = NOW()
FROM rebuilt
WHERE pak.id = rebuilt.id
  AND pak.rate_multipliers::jsonb IS DISTINCT FROM rebuilt.rate_multipliers;

WITH rebuilt AS (
  SELECT
    pak.id,
    jsonb_object_agg(
      public.aether_canonical_api_format_alias(entry.key),
      entry.value
      ORDER BY entry.ordinality
    ) AS global_priority_by_format
  FROM public.provider_api_keys AS pak
  CROSS JOIN LATERAL json_each(
    CASE
      WHEN json_typeof(pak.global_priority_by_format) = 'object' THEN pak.global_priority_by_format
      ELSE '{}'::json
    END
  ) WITH ORDINALITY AS entry(key, value, ordinality)
  WHERE pak.global_priority_by_format IS NOT NULL
  GROUP BY pak.id
)
UPDATE public.provider_api_keys AS pak
SET
  global_priority_by_format = rebuilt.global_priority_by_format::json,
  updated_at = NOW()
FROM rebuilt
WHERE pak.id = rebuilt.id
  AND pak.global_priority_by_format::jsonb IS DISTINCT FROM rebuilt.global_priority_by_format;

WITH rebuilt AS (
  SELECT
    pak.id,
    jsonb_object_agg(
      public.aether_canonical_api_format_alias(entry.key),
      entry.value
      ORDER BY entry.ordinality
    ) AS health_by_format
  FROM public.provider_api_keys AS pak
  CROSS JOIN LATERAL jsonb_each(
    CASE
      WHEN jsonb_typeof(pak.health_by_format) = 'object' THEN pak.health_by_format
      ELSE '{}'::jsonb
    END
  ) WITH ORDINALITY AS entry(key, value, ordinality)
  WHERE pak.health_by_format IS NOT NULL
  GROUP BY pak.id
)
UPDATE public.provider_api_keys AS pak
SET
  health_by_format = rebuilt.health_by_format,
  updated_at = NOW()
FROM rebuilt
WHERE pak.id = rebuilt.id
  AND pak.health_by_format IS DISTINCT FROM rebuilt.health_by_format;

WITH rebuilt AS (
  SELECT
    pak.id,
    jsonb_object_agg(
      public.aether_canonical_api_format_alias(entry.key),
      entry.value
      ORDER BY entry.ordinality
    ) AS circuit_breaker_by_format
  FROM public.provider_api_keys AS pak
  CROSS JOIN LATERAL jsonb_each(
    CASE
      WHEN jsonb_typeof(pak.circuit_breaker_by_format) = 'object' THEN pak.circuit_breaker_by_format
      ELSE '{}'::jsonb
    END
  ) WITH ORDINALITY AS entry(key, value, ordinality)
  WHERE pak.circuit_breaker_by_format IS NOT NULL
  GROUP BY pak.id
)
UPDATE public.provider_api_keys AS pak
SET
  circuit_breaker_by_format = rebuilt.circuit_breaker_by_format,
  updated_at = NOW()
FROM rebuilt
WHERE pak.id = rebuilt.id
  AND pak.circuit_breaker_by_format IS DISTINCT FROM rebuilt.circuit_breaker_by_format;

DROP FUNCTION public.aether_legacy_raw_api_format_auth_type(text);
DROP FUNCTION public.aether_canonical_api_format_alias(text);
