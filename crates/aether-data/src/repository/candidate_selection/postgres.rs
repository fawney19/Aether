use async_trait::async_trait;
use futures_util::{stream::TryStream, TryStreamExt};
use sqlx::{PgPool, Row};
use std::collections::BTreeSet;

use super::{
    MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateOrder, StoredPoolKeyCandidateRowsByKeyIdsQuery,
    StoredPoolKeyCandidateRowsQuery, StoredProviderModelMapping,
    StoredRequestedModelCandidateRowsQuery,
};
use crate::{error::SqlxResultExt, DataLayerError};

const LIST_FOR_EXACT_API_FORMAT_SQL: &str = r#"
WITH candidate_rows AS (
SELECT
  p.id AS provider_id,
  p.name AS provider_name,
  p.provider_type AS provider_type,
  p.provider_priority AS provider_priority,
  p.is_active AS provider_is_active,
  pe.id AS endpoint_id,
  pe.api_format AS endpoint_api_format,
  pe.api_family AS endpoint_api_family,
  pe.endpoint_kind AS endpoint_kind,
  pe.is_active AS endpoint_is_active,
  pak.id AS key_id,
  pak.name AS key_name,
  pak.auth_type AS key_auth_type,
  pak.is_active AS key_is_active,
  pak.api_formats AS key_api_formats,
  pak.allowed_models AS key_allowed_models,
  pak.capabilities AS key_capabilities,
  pak.internal_priority AS key_internal_priority,
  pak.global_priority_by_format AS key_global_priority_by_format,
  m.id AS model_id,
  m.global_model_id AS global_model_id,
  gm.name AS global_model_name,
  CASE
    WHEN gm.config IS NOT NULL THEN gm.config -> 'model_mappings'
    ELSE NULL
  END AS global_model_mappings,
  CASE
    WHEN gm.config IS NOT NULL AND gm.config ? 'streaming'
      THEN (gm.config ->> 'streaming')::BOOLEAN
    ELSE NULL
  END AS global_model_supports_streaming,
  m.provider_model_name AS model_provider_model_name,
  m.provider_model_mappings AS model_provider_model_mappings,
  m.supports_streaming AS model_supports_streaming,
  m.is_active AS model_is_active,
  m.is_available AS model_is_available,
  (p.config -> 'pool_advanced') IS NOT NULL AS provider_pool_enabled
FROM providers p
INNER JOIN provider_endpoints pe
  ON pe.provider_id = p.id
INNER JOIN LATERAL (
  SELECT pak.*
  FROM provider_api_keys pak
  WHERE pak.provider_id = p.id
    AND pak.is_active IS TRUE
    AND (
      pak.api_formats IS NULL
      OR EXISTS (
        SELECT 1
        FROM json_array_elements_text(pak.api_formats) AS fmt(value)
        WHERE LOWER(BTRIM(fmt.value)) = ANY($2::text[])
      )
    )
    AND (
      (
        LOWER(BTRIM(p.provider_type)) = 'codex'
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($3) IN ('openai:responses', 'openai:responses:compact', 'openai:image')
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'chatgpt_web'
        AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'bearer')
        AND LOWER($3) = 'openai:image'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'claude_code'
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($3) = 'claude:messages'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'kiro'
        AND LOWER($3) = 'claude:messages'
        AND (
          LOWER(BTRIM(pak.auth_type)) = 'oauth'
          OR (
            LOWER(BTRIM(pak.auth_type)) = 'bearer'
            AND pak.auth_config IS NOT NULL
            AND BTRIM(pak.auth_config) <> ''
          )
        )
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'grok'
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($3) IN ('openai:chat', 'openai:responses', 'claude:messages', 'openai:image')
      )
      OR (
        LOWER(BTRIM(p.provider_type)) IN ('gemini_cli', 'antigravity')
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($3) = 'gemini:generate_content'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'windsurf'
        AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer')
        AND LOWER($3) = 'openai:chat'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'vertex_ai'
        AND (
          (
            LOWER(BTRIM(pak.auth_type)) = 'api_key'
            AND LOWER($3) IN ('gemini:generate_content', 'gemini:embedding')
          )
          OR (
            LOWER(BTRIM(pak.auth_type)) IN ('service_account', 'vertex_ai')
            AND LOWER($3) IN ('claude:messages', 'gemini:generate_content', 'gemini:embedding')
          )
        )
      )
      OR (
        LOWER(BTRIM(p.provider_type)) NOT IN (
          'chatgpt_web',
          'claude_code',
          'codex',
          'gemini_cli',
          'grok',
          'vertex_ai',
          'antigravity',
          'kiro',
          'windsurf'
        )
        AND LOWER(BTRIM(pak.auth_type)) <> 'oauth'
      )
    )
  ORDER BY pak.internal_priority ASC, pak.id ASC
  LIMIT CASE WHEN (p.config -> 'pool_advanced') IS NOT NULL THEN 1 ELSE 2147483647 END
) pak ON TRUE
INNER JOIN models m
  ON m.provider_id = p.id
INNER JOIN global_models gm
  ON gm.id = m.global_model_id
WHERE p.is_active = TRUE
  AND pe.is_active = TRUE
  AND pak.is_active IS TRUE
  AND m.is_active = TRUE
  AND m.is_available = TRUE
  AND gm.is_active = TRUE
  AND LOWER(pe.api_format) = LOWER($1)
  AND (
    pak.api_formats IS NULL
    OR EXISTS (
      SELECT 1
      FROM json_array_elements_text(pak.api_formats) AS fmt(value)
      WHERE LOWER(BTRIM(fmt.value)) = ANY($2::text[])
    )
  )
  AND (
    (
      LOWER(BTRIM(p.provider_type)) = 'codex'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($3) IN ('openai:responses', 'openai:responses:compact', 'openai:image')
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'chatgpt_web'
      AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'bearer')
      AND LOWER($3) = 'openai:image'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'claude_code'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($3) = 'claude:messages'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'kiro'
      AND LOWER($3) = 'claude:messages'
      AND (
        LOWER(BTRIM(pak.auth_type)) = 'oauth'
        OR (
          LOWER(BTRIM(pak.auth_type)) = 'bearer'
          AND pak.auth_config IS NOT NULL
          AND BTRIM(pak.auth_config) <> ''
        )
      )
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'grok'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($3) IN ('openai:chat', 'openai:responses', 'claude:messages', 'openai:image')
    )
    OR (
      LOWER(BTRIM(p.provider_type)) IN ('gemini_cli', 'antigravity')
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($3) = 'gemini:generate_content'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'windsurf'
      AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer')
      AND LOWER($3) = 'openai:chat'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'vertex_ai'
      AND (
        (
          LOWER(BTRIM(pak.auth_type)) = 'api_key'
          AND LOWER($3) IN ('gemini:generate_content', 'gemini:embedding')
        )
        OR (
          LOWER(BTRIM(pak.auth_type)) IN ('service_account', 'vertex_ai')
          AND LOWER($3) IN ('claude:messages', 'gemini:generate_content', 'gemini:embedding')
        )
      )
    )
    OR (
      LOWER(BTRIM(p.provider_type)) NOT IN (
        'chatgpt_web',
        'claude_code',
        'codex',
        'gemini_cli',
        'grok',
        'vertex_ai',
        'antigravity',
        'kiro',
        'windsurf'
      )
      AND LOWER(BTRIM(pak.auth_type)) <> 'oauth'
    )
  )
),
pool_rows AS (
  SELECT DISTINCT ON (provider_id, endpoint_id, model_id)
    *
  FROM candidate_rows
  WHERE provider_pool_enabled
  ORDER BY
    provider_id ASC,
    endpoint_id ASC,
    model_id ASC,
    key_internal_priority ASC,
    key_id ASC
),
selected_rows AS (
  SELECT * FROM candidate_rows WHERE NOT provider_pool_enabled
  UNION ALL
  SELECT * FROM pool_rows
)
SELECT
  provider_id,
  provider_name,
  provider_type,
  provider_priority,
  provider_is_active,
  endpoint_id,
  endpoint_api_format,
  endpoint_api_family,
  endpoint_kind,
  endpoint_is_active,
  key_id,
  key_name,
  key_auth_type,
  key_is_active,
  key_api_formats,
  key_allowed_models,
  key_capabilities,
  key_internal_priority,
  key_global_priority_by_format,
  model_id,
  global_model_id,
  global_model_name,
  global_model_mappings,
  global_model_supports_streaming,
  model_provider_model_name,
  model_provider_model_mappings,
  model_supports_streaming,
  model_is_active,
  model_is_available
FROM selected_rows
ORDER BY
  global_model_name ASC,
  provider_priority ASC,
  key_internal_priority ASC,
  provider_id ASC,
  endpoint_id ASC,
  key_id ASC,
  model_id ASC
"#;

const LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL: &str = r#"
WITH candidate_rows AS (
SELECT
  p.id AS provider_id,
  p.name AS provider_name,
  p.provider_type AS provider_type,
  p.provider_priority AS provider_priority,
  p.is_active AS provider_is_active,
  pe.id AS endpoint_id,
  pe.api_format AS endpoint_api_format,
  pe.api_family AS endpoint_api_family,
  pe.endpoint_kind AS endpoint_kind,
  pe.is_active AS endpoint_is_active,
  pak.id AS key_id,
  pak.name AS key_name,
  pak.auth_type AS key_auth_type,
  pak.is_active AS key_is_active,
  pak.api_formats AS key_api_formats,
  pak.allowed_models AS key_allowed_models,
  pak.capabilities AS key_capabilities,
  pak.internal_priority AS key_internal_priority,
  pak.global_priority_by_format AS key_global_priority_by_format,
  m.id AS model_id,
  m.global_model_id AS global_model_id,
  gm.name AS global_model_name,
  CASE
    WHEN gm.config IS NOT NULL THEN gm.config -> 'model_mappings'
    ELSE NULL
  END AS global_model_mappings,
  CASE
    WHEN gm.config IS NOT NULL AND gm.config ? 'streaming'
      THEN (gm.config ->> 'streaming')::BOOLEAN
    ELSE NULL
  END AS global_model_supports_streaming,
  m.provider_model_name AS model_provider_model_name,
  m.provider_model_mappings AS model_provider_model_mappings,
  m.supports_streaming AS model_supports_streaming,
  m.is_active AS model_is_active,
  m.is_available AS model_is_available,
  (p.config -> 'pool_advanced') IS NOT NULL AS provider_pool_enabled
FROM providers p
INNER JOIN provider_endpoints pe
  ON pe.provider_id = p.id
INNER JOIN LATERAL (
  SELECT pak.*
  FROM provider_api_keys pak
  WHERE pak.provider_id = p.id
    AND pak.is_active IS TRUE
    AND (
      pak.api_formats IS NULL
      OR EXISTS (
        SELECT 1
        FROM json_array_elements_text(pak.api_formats) AS fmt(value)
        WHERE LOWER(BTRIM(fmt.value)) = ANY($3::text[])
      )
    )
    AND (
      (
        LOWER(BTRIM(p.provider_type)) = 'codex'
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($4) IN ('openai:responses', 'openai:responses:compact', 'openai:image')
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'chatgpt_web'
        AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'bearer')
        AND LOWER($4) = 'openai:image'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'claude_code'
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($4) = 'claude:messages'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'kiro'
        AND LOWER($4) = 'claude:messages'
        AND (
          LOWER(BTRIM(pak.auth_type)) = 'oauth'
          OR (
            LOWER(BTRIM(pak.auth_type)) = 'bearer'
            AND pak.auth_config IS NOT NULL
            AND BTRIM(pak.auth_config) <> ''
          )
        )
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'grok'
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($4) IN ('openai:chat', 'openai:responses', 'claude:messages', 'openai:image')
      )
      OR (
        LOWER(BTRIM(p.provider_type)) IN ('gemini_cli', 'antigravity')
        AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
        AND LOWER($4) = 'gemini:generate_content'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'windsurf'
        AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer')
        AND LOWER($4) = 'openai:chat'
      )
      OR (
        LOWER(BTRIM(p.provider_type)) = 'vertex_ai'
        AND (
          (
            LOWER(BTRIM(pak.auth_type)) = 'api_key'
            AND LOWER($4) IN ('gemini:generate_content', 'gemini:embedding')
          )
          OR (
            LOWER(BTRIM(pak.auth_type)) IN ('service_account', 'vertex_ai')
            AND LOWER($4) IN ('claude:messages', 'gemini:generate_content', 'gemini:embedding')
          )
        )
      )
      OR (
        LOWER(BTRIM(p.provider_type)) NOT IN (
          'chatgpt_web',
          'claude_code',
          'codex',
          'gemini_cli',
          'grok',
          'vertex_ai',
          'antigravity',
          'kiro',
          'windsurf'
        )
        AND LOWER(BTRIM(pak.auth_type)) <> 'oauth'
      )
    )
  ORDER BY pak.internal_priority ASC, pak.id ASC
  LIMIT CASE WHEN (p.config -> 'pool_advanced') IS NOT NULL THEN 1 ELSE 2147483647 END
) pak ON TRUE
INNER JOIN models m
  ON m.provider_id = p.id
INNER JOIN global_models gm
  ON gm.id = m.global_model_id
WHERE p.is_active = TRUE
  AND pe.is_active = TRUE
  AND pak.is_active IS TRUE
  AND m.is_active = TRUE
  AND m.is_available = TRUE
  AND gm.is_active = TRUE
  AND LOWER(pe.api_format) = LOWER($1)
  AND gm.name = $2
  AND (
    pak.api_formats IS NULL
    OR EXISTS (
      SELECT 1
      FROM json_array_elements_text(pak.api_formats) AS fmt(value)
      WHERE LOWER(BTRIM(fmt.value)) = ANY($3::text[])
    )
  )
  AND (
    (
      LOWER(BTRIM(p.provider_type)) = 'codex'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($4) IN ('openai:responses', 'openai:responses:compact', 'openai:image')
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'chatgpt_web'
      AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'bearer')
      AND LOWER($4) = 'openai:image'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'claude_code'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($4) = 'claude:messages'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'kiro'
      AND LOWER($4) = 'claude:messages'
      AND (
        LOWER(BTRIM(pak.auth_type)) = 'oauth'
        OR (
          LOWER(BTRIM(pak.auth_type)) = 'bearer'
          AND pak.auth_config IS NOT NULL
          AND BTRIM(pak.auth_config) <> ''
        )
      )
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'grok'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($4) IN ('openai:chat', 'openai:responses', 'claude:messages', 'openai:image')
    )
    OR (
      LOWER(BTRIM(p.provider_type)) IN ('gemini_cli', 'antigravity')
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($4) = 'gemini:generate_content'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'windsurf'
      AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer')
      AND LOWER($4) = 'openai:chat'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'vertex_ai'
      AND (
        (
          LOWER(BTRIM(pak.auth_type)) = 'api_key'
          AND LOWER($4) IN ('gemini:generate_content', 'gemini:embedding')
        )
        OR (
          LOWER(BTRIM(pak.auth_type)) IN ('service_account', 'vertex_ai')
          AND LOWER($4) IN ('claude:messages', 'gemini:generate_content', 'gemini:embedding')
        )
      )
    )
    OR (
      LOWER(BTRIM(p.provider_type)) NOT IN (
        'chatgpt_web',
        'claude_code',
        'codex',
        'gemini_cli',
        'grok',
        'vertex_ai',
        'antigravity',
        'kiro',
        'windsurf'
      )
      AND LOWER(BTRIM(pak.auth_type)) <> 'oauth'
    )
  )
),
pool_rows AS (
  SELECT DISTINCT ON (provider_id, endpoint_id, model_id)
    *
  FROM candidate_rows
  WHERE provider_pool_enabled
  ORDER BY
    provider_id ASC,
    endpoint_id ASC,
    model_id ASC,
    key_internal_priority ASC,
    key_id ASC
),
selected_rows AS (
  SELECT * FROM candidate_rows WHERE NOT provider_pool_enabled
  UNION ALL
  SELECT * FROM pool_rows
)
SELECT
  provider_id,
  provider_name,
  provider_type,
  provider_priority,
  provider_is_active,
  endpoint_id,
  endpoint_api_format,
  endpoint_api_family,
  endpoint_kind,
  endpoint_is_active,
  key_id,
  key_name,
  key_auth_type,
  key_is_active,
  key_api_formats,
  key_allowed_models,
  key_capabilities,
  key_internal_priority,
  key_global_priority_by_format,
  model_id,
  global_model_id,
  global_model_name,
  global_model_mappings,
  global_model_supports_streaming,
  model_provider_model_name,
  model_provider_model_mappings,
  model_supports_streaming,
  model_is_active,
  model_is_available
FROM selected_rows
ORDER BY
  provider_priority ASC,
  key_internal_priority ASC,
  provider_id ASC,
  endpoint_id ASC,
  key_id ASC,
  model_id ASC
"#;

const LIST_POOL_KEYS_FOR_GROUP_SQL: &str = r#"
SELECT
  p.id AS provider_id,
  p.name AS provider_name,
  p.provider_type AS provider_type,
  p.provider_priority AS provider_priority,
  p.is_active AS provider_is_active,
  pe.id AS endpoint_id,
  pe.api_format AS endpoint_api_format,
  pe.api_family AS endpoint_api_family,
  pe.endpoint_kind AS endpoint_kind,
  pe.is_active AS endpoint_is_active,
  pak.id AS key_id,
  pak.name AS key_name,
  pak.auth_type AS key_auth_type,
  pak.is_active AS key_is_active,
  pak.api_formats AS key_api_formats,
  pak.allowed_models AS key_allowed_models,
  pak.capabilities AS key_capabilities,
  pak.internal_priority AS key_internal_priority,
  pak.global_priority_by_format AS key_global_priority_by_format,
  m.id AS model_id,
  m.global_model_id AS global_model_id,
  gm.name AS global_model_name,
  CASE
    WHEN gm.config IS NOT NULL THEN gm.config -> 'model_mappings'
    ELSE NULL
  END AS global_model_mappings,
  CASE
    WHEN gm.config IS NOT NULL AND gm.config ? 'streaming'
      THEN (gm.config ->> 'streaming')::BOOLEAN
    ELSE NULL
  END AS global_model_supports_streaming,
  m.provider_model_name AS model_provider_model_name,
  m.provider_model_mappings AS model_provider_model_mappings,
  m.supports_streaming AS model_supports_streaming,
  m.is_active AS model_is_active,
  m.is_available AS model_is_available
FROM providers p
INNER JOIN provider_endpoints pe
  ON pe.provider_id = p.id
INNER JOIN provider_api_keys pak
  ON pak.provider_id = p.id
INNER JOIN models m
  ON m.provider_id = p.id
INNER JOIN global_models gm
  ON gm.id = m.global_model_id
WHERE p.is_active = TRUE
  AND pe.is_active = TRUE
  AND pak.is_active IS TRUE
  AND m.is_active = TRUE
  AND m.is_available = TRUE
  AND gm.is_active = TRUE
  AND LOWER(pe.api_format) = LOWER($1)
  AND p.id = $2
  AND pe.id = $3
  AND m.id = $4
  AND (
    pak.api_formats IS NULL
    OR EXISTS (
      SELECT 1
      FROM json_array_elements_text(pak.api_formats) AS fmt(value)
      WHERE LOWER(BTRIM(fmt.value)) = ANY($5::text[])
    )
  )
  AND (
    (
      LOWER(BTRIM(p.provider_type)) = 'codex'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($6) IN ('openai:responses', 'openai:responses:compact', 'openai:image')
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'chatgpt_web'
      AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'bearer')
      AND LOWER($6) = 'openai:image'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'claude_code'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($6) = 'claude:messages'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'kiro'
      AND LOWER($6) = 'claude:messages'
      AND (
        LOWER(BTRIM(pak.auth_type)) = 'oauth'
        OR (
          LOWER(BTRIM(pak.auth_type)) = 'bearer'
          AND pak.auth_config IS NOT NULL
          AND BTRIM(pak.auth_config) <> ''
        )
      )
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'grok'
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($6) IN ('openai:chat', 'openai:responses', 'claude:messages', 'openai:image')
    )
    OR (
      LOWER(BTRIM(p.provider_type)) IN ('gemini_cli', 'antigravity')
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
      AND LOWER($6) = 'gemini:generate_content'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'windsurf'
      AND LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer')
      AND LOWER($6) = 'openai:chat'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'vertex_ai'
      AND (
        (
          LOWER(BTRIM(pak.auth_type)) = 'api_key'
          AND LOWER($6) IN ('gemini:generate_content', 'gemini:embedding')
        )
        OR (
          LOWER(BTRIM(pak.auth_type)) IN ('service_account', 'vertex_ai')
          AND LOWER($6) IN ('claude:messages', 'gemini:generate_content', 'gemini:embedding')
        )
      )
    )
    OR (
      LOWER(BTRIM(p.provider_type)) NOT IN (
        'chatgpt_web',
        'claude_code',
        'codex',
        'gemini_cli',
        'grok',
        'vertex_ai',
        'antigravity',
        'kiro',
        'windsurf'
      )
      AND LOWER(BTRIM(pak.auth_type)) <> 'oauth'
    )
  )
ORDER BY
  pak.internal_priority ASC,
  pak.id ASC
LIMIT $7
OFFSET $8
"#;

fn pool_key_candidate_order_by_sql(order: &StoredPoolKeyCandidateOrder) -> &'static str {
    match order {
        StoredPoolKeyCandidateOrder::InternalPriority => {
            "ORDER BY\n  pak.internal_priority ASC,\n  pak.id ASC"
        }
        StoredPoolKeyCandidateOrder::Lru => {
            "ORDER BY\n  pak.last_used_at ASC NULLS FIRST,\n  pak.internal_priority ASC,\n  pak.id ASC"
        }
        StoredPoolKeyCandidateOrder::CacheAffinity => {
            "ORDER BY\n  pak.last_used_at DESC NULLS LAST,\n  pak.internal_priority ASC,\n  pak.id ASC"
        }
        StoredPoolKeyCandidateOrder::SingleAccount => {
            "ORDER BY\n  pak.internal_priority ASC,\n  pak.last_used_at DESC NULLS LAST,\n  pak.id ASC"
        }
        StoredPoolKeyCandidateOrder::LoadBalance { .. } => {
            "ORDER BY\n  md5($9 || ':' || pak.id) ASC,\n  pak.id ASC"
        }
    }
}

fn pool_key_candidate_selection_sql(order: &StoredPoolKeyCandidateOrder) -> String {
    let default_order =
        "ORDER BY\n  pak.internal_priority ASC,\n  pak.id ASC\nLIMIT $7\nOFFSET $8\n";
    let replacement = format!(
        "{}\nLIMIT $7\nOFFSET $8\n",
        pool_key_candidate_order_by_sql(order)
    );
    LIST_POOL_KEYS_FOR_GROUP_SQL.replace(default_order, &replacement)
}

fn pool_key_candidate_selection_by_key_ids_sql() -> String {
    let default_order =
        "ORDER BY\n  pak.internal_priority ASC,\n  pak.id ASC\nLIMIT $7\nOFFSET $8\n";
    let replacement = "AND pak.id = ANY($7::text[])\nORDER BY\n  array_position($7::text[], pak.id) ASC,\n  pak.id ASC\n";
    LIST_POOL_KEYS_FOR_GROUP_SQL.replace(default_order, replacement)
}

#[derive(Debug, Clone)]
pub struct SqlxMinimalCandidateSelectionReadRepository {
    pool: PgPool,
}

impl SqlxMinimalCandidateSelectionReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn collect_query_rows<T, S>(
        mut rows: S,
        map_row: fn(&sqlx::postgres::PgRow) -> Result<T, DataLayerError>,
    ) -> Result<Vec<T>, DataLayerError>
    where
        S: TryStream<Ok = sqlx::postgres::PgRow, Error = sqlx::Error> + Unpin,
    {
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_row(&row)?);
        }
        Ok(items)
    }

    pub async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let mut rows = Vec::new();
        let canonical_api_format = normalize_api_format(api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let sql_match_aliases = sql_match_aliases(&storage_aliases);
        for api_format in storage_aliases {
            rows.extend(
                Self::collect_query_rows(
                    sqlx::query(LIST_FOR_EXACT_API_FORMAT_SQL)
                        .bind(api_format)
                        .bind(sql_match_aliases.clone())
                        .bind(canonical_api_format.clone())
                        .fetch(&self.pool),
                    map_candidate_selection_row,
                )
                .await?,
            );
        }
        Ok(dedupe_candidate_selection_rows(rows))
    }

    pub async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let mut rows = Vec::new();
        let canonical_api_format = normalize_api_format(api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let sql_match_aliases = sql_match_aliases(&storage_aliases);
        for api_format in storage_aliases {
            rows.extend(
                Self::collect_query_rows(
                    sqlx::query(LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL)
                        .bind(api_format)
                        .bind(global_model_name)
                        .bind(sql_match_aliases.clone())
                        .bind(canonical_api_format.clone())
                        .fetch(&self.pool),
                    map_candidate_selection_row,
                )
                .await?,
            );
        }
        Ok(dedupe_candidate_selection_rows(rows))
    }

    pub async fn list_for_exact_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let mut rows = Vec::new();
        let canonical_api_format = normalize_api_format(api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let sql_match_aliases = sql_match_aliases(&storage_aliases);
        let sql = requested_model_selection_sql();
        for api_format in storage_aliases {
            rows.extend(
                Self::collect_query_rows(
                    sqlx::query(sql.as_str())
                        .bind(api_format)
                        .bind(requested_model_name)
                        .bind(sql_match_aliases.clone())
                        .bind(canonical_api_format.clone())
                        .fetch(&self.pool),
                    map_candidate_selection_row,
                )
                .await?,
            );
        }
        Ok(dedupe_candidate_selection_rows(rows))
    }

    pub async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let mut rows = Vec::new();
        let canonical_api_format = normalize_api_format(&query.api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let sql_match_aliases = sql_match_aliases(&storage_aliases);
        let limit = i64::from(query.limit.max(1));
        let offset = i64::from(query.offset);
        let sql = requested_model_selection_page_sql();
        for api_format in storage_aliases {
            rows.extend(
                Self::collect_query_rows(
                    sqlx::query(sql.as_str())
                        .bind(api_format)
                        .bind(query.requested_model_name.as_str())
                        .bind(sql_match_aliases.clone())
                        .bind(canonical_api_format.clone())
                        .bind(limit)
                        .bind(offset)
                        .fetch(&self.pool),
                    map_candidate_selection_row,
                )
                .await?,
            );
        }
        Ok(dedupe_candidate_selection_rows(rows))
    }

    pub async fn list_pool_key_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let mut rows = Vec::new();
        let canonical_api_format = normalize_api_format(&query.api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let sql_match_aliases = sql_match_aliases(&storage_aliases);
        let limit = i64::from(query.limit.max(1));
        let offset = i64::from(query.offset);
        let sql = pool_key_candidate_selection_sql(&query.order);
        for api_format in storage_aliases {
            let mut query_builder = sqlx::query(sql.as_str())
                .bind(api_format)
                .bind(query.provider_id.as_str())
                .bind(query.endpoint_id.as_str())
                .bind(query.model_id.as_str())
                .bind(sql_match_aliases.clone())
                .bind(canonical_api_format.clone())
                .bind(limit)
                .bind(offset);
            if let StoredPoolKeyCandidateOrder::LoadBalance { seed } = &query.order {
                query_builder = query_builder.bind(seed.as_str());
            }
            rows.extend(
                Self::collect_query_rows(
                    query_builder.fetch(&self.pool),
                    map_candidate_selection_row,
                )
                .await?,
            );
        }
        Ok(dedupe_candidate_selection_rows(rows))
    }

    pub async fn list_pool_key_rows_for_group_key_ids(
        &self,
        query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        if query.key_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut rows = Vec::new();
        let canonical_api_format = normalize_api_format(&query.api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let sql_match_aliases = sql_match_aliases(&storage_aliases);
        let sql = pool_key_candidate_selection_by_key_ids_sql();
        for api_format in storage_aliases {
            rows.extend(
                Self::collect_query_rows(
                    sqlx::query(sql.as_str())
                        .bind(api_format)
                        .bind(query.provider_id.as_str())
                        .bind(query.endpoint_id.as_str())
                        .bind(query.model_id.as_str())
                        .bind(sql_match_aliases.clone())
                        .bind(canonical_api_format.clone())
                        .bind(query.key_ids.clone())
                        .fetch(&self.pool),
                    map_candidate_selection_row,
                )
                .await?,
            );
        }
        let key_order = query
            .key_ids
            .iter()
            .enumerate()
            .map(|(index, key_id)| (key_id.as_str(), index))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut rows = dedupe_candidate_selection_rows(rows);
        rows.sort_by(|left, right| {
            key_order
                .get(left.key_id.as_str())
                .cmp(&key_order.get(right.key_id.as_str()))
                .then(left.key_id.cmp(&right.key_id))
        });
        Ok(rows)
    }
}

fn requested_model_selection_sql() -> String {
    LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL
        .replace(
            "AND gm.name = $2",
            r#"AND (
    (
      gm.name = $2
      AND (
        m.provider_model_mappings IS NULL
        OR jsonb_typeof(m.provider_model_mappings) <> 'array'
        OR EXISTS (
          SELECT 1
          FROM jsonb_array_elements(
            CASE
              WHEN jsonb_typeof(m.provider_model_mappings) = 'array'
                THEN m.provider_model_mappings
              ELSE '[]'::jsonb
            END
          ) AS mapping(value)
          WHERE (
            mapping.value -> 'api_formats' IS NULL
            OR jsonb_typeof(mapping.value -> 'api_formats') <> 'array'
            OR EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(mapping.value -> 'api_formats') AS fmt(value)
              WHERE LOWER(BTRIM(fmt.value)) = ANY($3::text[])
            )
          )
          AND (
            mapping.value -> 'endpoint_ids' IS NULL
            OR jsonb_typeof(mapping.value -> 'endpoint_ids') <> 'array'
            OR EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(mapping.value -> 'endpoint_ids') AS endpoint(value)
              WHERE endpoint.value = pe.id
            )
          )
        )
        OR NOT EXISTS (
          SELECT 1
          FROM jsonb_array_elements(
            CASE
              WHEN jsonb_typeof(m.provider_model_mappings) = 'array'
                THEN m.provider_model_mappings
              ELSE '[]'::jsonb
            END
          ) AS mapping(value)
          WHERE mapping.value ->> 'name' = m.provider_model_name
        )
      )
    )
    OR (
      m.provider_model_name = $2
      AND (
        m.provider_model_mappings IS NULL
        OR jsonb_typeof(m.provider_model_mappings) <> 'array'
        OR NOT EXISTS (
          SELECT 1
          FROM jsonb_array_elements(
            CASE
              WHEN jsonb_typeof(m.provider_model_mappings) = 'array'
                THEN m.provider_model_mappings
              ELSE '[]'::jsonb
            END
          ) AS mapping(value)
          WHERE mapping.value ->> 'name' = m.provider_model_name
        )
        OR EXISTS (
          SELECT 1
          FROM jsonb_array_elements(
            CASE
              WHEN jsonb_typeof(m.provider_model_mappings) = 'array'
                THEN m.provider_model_mappings
              ELSE '[]'::jsonb
            END
          ) AS mapping(value)
          WHERE mapping.value ->> 'name' = m.provider_model_name
            AND (
              mapping.value -> 'api_formats' IS NULL
              OR jsonb_typeof(mapping.value -> 'api_formats') <> 'array'
              OR EXISTS (
                SELECT 1
                FROM jsonb_array_elements_text(mapping.value -> 'api_formats') AS fmt(value)
                WHERE LOWER(BTRIM(fmt.value)) = ANY($3::text[])
              )
            )
            AND (
              mapping.value -> 'endpoint_ids' IS NULL
              OR jsonb_typeof(mapping.value -> 'endpoint_ids') <> 'array'
              OR EXISTS (
                SELECT 1
                FROM jsonb_array_elements_text(mapping.value -> 'endpoint_ids') AS endpoint(value)
                WHERE endpoint.value = pe.id
              )
            )
        )
      )
    )
    OR (
      jsonb_typeof(m.provider_model_mappings) = 'array'
      AND EXISTS (
        SELECT 1
        FROM jsonb_array_elements(
          CASE
            WHEN jsonb_typeof(m.provider_model_mappings) = 'array'
              THEN m.provider_model_mappings
            ELSE '[]'::jsonb
          END
        ) AS mapping(value)
        WHERE mapping.value ->> 'name' = $2
          AND (
            mapping.value -> 'api_formats' IS NULL
            OR jsonb_typeof(mapping.value -> 'api_formats') <> 'array'
            OR EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(mapping.value -> 'api_formats') AS fmt(value)
              WHERE LOWER(BTRIM(fmt.value)) = ANY($3::text[])
            )
          )
          AND (
            mapping.value -> 'endpoint_ids' IS NULL
            OR jsonb_typeof(mapping.value -> 'endpoint_ids') <> 'array'
            OR EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(mapping.value -> 'endpoint_ids') AS endpoint(value)
              WHERE endpoint.value = pe.id
            )
          )
      )
    )
  )"#,
        )
        .replace(
            "ORDER BY\n  provider_priority ASC,",
            "ORDER BY\n  global_model_name ASC,\n  provider_priority ASC,",
        )
}

fn requested_model_selection_page_sql() -> String {
    format!("{}\nLIMIT $5\nOFFSET $6", requested_model_selection_sql())
}

fn api_format_aliases(api_format: &str) -> Vec<String> {
    aether_ai_formats::api_format_storage_aliases(api_format)
}

fn normalize_api_format(api_format: &str) -> String {
    aether_ai_formats::normalize_api_format_alias(api_format)
}

fn sql_match_aliases(api_formats: &[String]) -> Vec<String> {
    api_formats
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect()
}

fn dedupe_candidate_selection_rows(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    let mut seen = BTreeSet::new();
    rows.into_iter()
        .filter(|row| {
            seen.insert((
                row.endpoint_id.clone(),
                row.key_id.clone(),
                row.model_id.clone(),
            ))
        })
        .collect()
}

#[async_trait]
impl MinimalCandidateSelectionReadRepository for SqlxMinimalCandidateSelectionReadRepository {
    async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Self::list_for_exact_api_format(self, api_format).await
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Self::list_for_exact_api_format_and_global_model(self, api_format, global_model_name).await
    }

    async fn list_for_exact_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Self::list_for_exact_api_format_and_requested_model(self, api_format, requested_model_name)
            .await
    }

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Self::list_for_exact_api_format_and_requested_model_page(self, query).await
    }

    async fn list_pool_key_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Self::list_pool_key_rows_for_group(self, query).await
    }

    async fn list_pool_key_rows_for_group_key_ids(
        &self,
        query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Self::list_pool_key_rows_for_group_key_ids(self, query).await
    }
}

fn map_candidate_selection_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredMinimalCandidateSelectionRow, DataLayerError> {
    Ok(StoredMinimalCandidateSelectionRow {
        provider_id: row.try_get("provider_id").map_postgres_err()?,
        provider_name: row.try_get("provider_name").map_postgres_err()?,
        provider_type: row.try_get("provider_type").map_postgres_err()?,
        provider_priority: row.try_get("provider_priority").map_postgres_err()?,
        provider_is_active: row.try_get("provider_is_active").map_postgres_err()?,
        endpoint_id: row.try_get("endpoint_id").map_postgres_err()?,
        endpoint_api_format: row.try_get("endpoint_api_format").map_postgres_err()?,
        endpoint_api_family: row.try_get("endpoint_api_family").map_postgres_err()?,
        endpoint_kind: row.try_get("endpoint_kind").map_postgres_err()?,
        endpoint_is_active: row.try_get("endpoint_is_active").map_postgres_err()?,
        key_id: row.try_get("key_id").map_postgres_err()?,
        key_name: row.try_get("key_name").map_postgres_err()?,
        key_auth_type: row.try_get("key_auth_type").map_postgres_err()?,
        key_is_active: row.try_get("key_is_active").map_postgres_err()?,
        key_api_formats: parse_string_list(
            row.try_get("key_api_formats").map_postgres_err()?,
            "provider_api_keys.api_formats",
        )?,
        key_allowed_models: parse_string_list(
            row.try_get("key_allowed_models").map_postgres_err()?,
            "provider_api_keys.allowed_models",
        )?,
        key_capabilities: row.try_get("key_capabilities").map_postgres_err()?,
        key_internal_priority: row.try_get("key_internal_priority").map_postgres_err()?,
        key_global_priority_by_format: row
            .try_get("key_global_priority_by_format")
            .map_postgres_err()?,
        model_id: row.try_get("model_id").map_postgres_err()?,
        global_model_id: row.try_get("global_model_id").map_postgres_err()?,
        global_model_name: row.try_get("global_model_name").map_postgres_err()?,
        global_model_mappings: parse_string_list(
            row.try_get("global_model_mappings").map_postgres_err()?,
            "global_models.config.model_mappings",
        )?,
        global_model_supports_streaming: row
            .try_get("global_model_supports_streaming")
            .map_postgres_err()?,
        model_provider_model_name: row
            .try_get("model_provider_model_name")
            .map_postgres_err()?,
        model_provider_model_mappings: parse_provider_model_mappings(
            row.try_get("model_provider_model_mappings")
                .map_postgres_err()?,
        )?,
        model_supports_streaming: row.try_get("model_supports_streaming").map_postgres_err()?,
        model_is_active: row.try_get("model_is_active").map_postgres_err()?,
        model_is_available: row.try_get("model_is_available").map_postgres_err()?,
    })
}

fn parse_string_list(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<Vec<String>>, DataLayerError> {
    let Some(value) = value else {
        return Ok(None);
    };
    parse_string_list_value(&value, field_name)
}

fn parse_string_list_value(
    value: &serde_json::Value,
    field_name: &str,
) -> Result<Option<Vec<String>>, DataLayerError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Array(array) => parse_string_list_array(array, field_name).map(Some),
        serde_json::Value::String(raw) => parse_embedded_string_list(raw, field_name),
        _ => Err(DataLayerError::UnexpectedValue(format!(
            "{field_name} is not a JSON array"
        ))),
    }
}

fn parse_embedded_string_list(
    raw: &str,
    field_name: &str,
) -> Result<Option<Vec<String>>, DataLayerError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
        return Ok(None);
    }

    if let Ok(decoded) = serde_json::from_str::<serde_json::Value>(raw) {
        return parse_string_list_value(&decoded, field_name);
    }

    Ok(Some(vec![raw.to_string()]))
}

fn parse_string_list_array(
    array: &[serde_json::Value],
    field_name: &str,
) -> Result<Vec<String>, DataLayerError> {
    let mut items = Vec::with_capacity(array.len());
    for item in array {
        let Some(item) = item.as_str() else {
            return Err(DataLayerError::UnexpectedValue(format!(
                "{field_name} contains a non-string item"
            )));
        };
        let item = item.trim();
        if !item.is_empty() {
            items.push(item.to_string());
        }
    }
    Ok(items)
}

fn parse_provider_model_mappings(
    value: Option<serde_json::Value>,
) -> Result<Option<Vec<StoredProviderModelMapping>>, DataLayerError> {
    let Some(value) = value else {
        return Ok(None);
    };
    parse_provider_model_mappings_value(&value)
}

fn parse_provider_model_mappings_value(
    value: &serde_json::Value,
) -> Result<Option<Vec<StoredProviderModelMapping>>, DataLayerError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Array(array) => parse_provider_model_mappings_array(array),
        serde_json::Value::Object(object) => {
            parse_provider_model_mapping_object(object).map(|mapping| Some(vec![mapping]))
        }
        serde_json::Value::String(raw) => parse_embedded_provider_model_mappings(raw),
        _ => Err(DataLayerError::UnexpectedValue(
            "models.provider_model_mappings is not a JSON array".to_string(),
        )),
    }
}

fn parse_embedded_provider_model_mappings(
    raw: &str,
) -> Result<Option<Vec<StoredProviderModelMapping>>, DataLayerError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
        return Ok(None);
    }

    if let Ok(decoded) = serde_json::from_str::<serde_json::Value>(raw) {
        return parse_provider_model_mappings_value(&decoded);
    }

    Ok(Some(vec![StoredProviderModelMapping {
        name: raw.to_string(),
        priority: 1,
        api_formats: None,
        endpoint_ids: None,
    }]))
}

fn parse_provider_model_mappings_array(
    array: &[serde_json::Value],
) -> Result<Option<Vec<StoredProviderModelMapping>>, DataLayerError> {
    let mut mappings = Vec::with_capacity(array.len());
    for raw in array {
        match raw {
            serde_json::Value::Object(object) => {
                if let Some(mapping) = parse_provider_model_mapping_object_lenient(object)? {
                    mappings.push(mapping);
                }
            }
            serde_json::Value::String(raw) => {
                let raw = raw.trim();
                if !raw.is_empty() {
                    mappings.push(StoredProviderModelMapping {
                        name: raw.to_string(),
                        priority: 1,
                        api_formats: None,
                        endpoint_ids: None,
                    });
                }
            }
            serde_json::Value::Null => {}
            _ => {}
        }
    }

    if mappings.is_empty() {
        Ok(None)
    } else {
        Ok(Some(mappings))
    }
}

fn parse_provider_model_mapping_object(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<StoredProviderModelMapping, DataLayerError> {
    parse_provider_model_mapping_object_lenient(object)?.ok_or_else(|| {
        DataLayerError::UnexpectedValue(
            "models.provider_model_mappings item is missing a valid name".to_string(),
        )
    })
}

fn parse_provider_model_mapping_object_lenient(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<StoredProviderModelMapping>, DataLayerError> {
    let Some(name) = object
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    let priority = object
        .get("priority")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(1)
        .max(1);
    let api_formats = parse_string_list(
        object.get("api_formats").cloned(),
        "models.provider_model_mappings.api_formats",
    )?
    .map(|formats| {
        formats
            .into_iter()
            .map(|value| aether_ai_formats::normalize_api_format_alias(&value))
            .collect()
    });
    let endpoint_ids = parse_string_list(
        object.get("endpoint_ids").cloned(),
        "models.provider_model_mappings.endpoint_ids",
    )?;

    Ok(Some(StoredProviderModelMapping {
        name: name.to_string(),
        priority: i32::try_from(priority).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "invalid models.provider_model_mappings.priority: {priority}"
            ))
        })?,
        api_formats,
        endpoint_ids,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        parse_provider_model_mappings, parse_string_list, pool_key_candidate_selection_sql,
        requested_model_selection_page_sql, requested_model_selection_sql,
        SqlxMinimalCandidateSelectionReadRepository,
        LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL, LIST_FOR_EXACT_API_FORMAT_SQL,
        LIST_POOL_KEYS_FOR_GROUP_SQL,
    };
    use crate::driver::postgres::{PostgresPoolConfig, PostgresPoolFactory};
    use crate::repository::candidate_selection::{
        StoredPoolKeyCandidateOrder, StoredProviderModelMapping,
    };

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        let pool = factory.connect_lazy().expect("pool should build");
        let repository = SqlxMinimalCandidateSelectionReadRepository::new(pool);
        let _ = repository.pool();
    }

    #[test]
    fn requested_model_selection_sql_filters_before_row_materialization() {
        let sql = requested_model_selection_sql();

        assert!(sql.contains("m.provider_model_name = $2"));
        assert!(sql.contains("jsonb_array_elements("));
        assert!(!sql.contains("json_typeof(m.provider_model_mappings)"));
        assert!(!sql.contains("json_array_elements_text(gm.config -> 'model_mappings')"));
        assert!(sql.contains("ORDER BY\n  global_model_name ASC,"));
        assert!(!sql.contains("AND gm.name = $2\n  AND"));
    }

    #[test]
    fn exact_candidate_selection_sql_limits_pool_key_expansion() {
        for sql in [
            LIST_FOR_EXACT_API_FORMAT_SQL,
            LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL,
        ] {
            assert!(sql.contains("INNER JOIN LATERAL"));
            assert!(sql.contains("FROM provider_api_keys pak"));
            assert!(sql.contains("AND pak.is_active IS TRUE"));
            assert!(!sql.contains("AND pak.is_active = TRUE"));
            assert!(sql.contains(
                "LIMIT CASE WHEN (p.config -> 'pool_advanced') IS NOT NULL THEN 1 ELSE 2147483647 END"
            ));
        }
        assert!(!LIST_POOL_KEYS_FOR_GROUP_SQL.contains("INNER JOIN LATERAL"));
        assert!(LIST_POOL_KEYS_FOR_GROUP_SQL.contains("AND pak.is_active IS TRUE"));
        assert!(!LIST_POOL_KEYS_FOR_GROUP_SQL.contains("AND pak.is_active = TRUE"));
    }

    #[test]
    fn candidate_selection_sql_allows_chatgpt_web_image_auth() {
        let requested_model_sql = requested_model_selection_sql();
        for sql in [
            LIST_FOR_EXACT_API_FORMAT_SQL,
            LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL,
            LIST_POOL_KEYS_FOR_GROUP_SQL,
            requested_model_sql.as_str(),
        ] {
            assert!(sql.contains("LOWER(BTRIM(p.provider_type)) = 'chatgpt_web'"));
            assert!(sql.contains("LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'bearer')"));
            assert!(sql.contains("'chatgpt_web',"));
        }
    }

    #[test]
    fn candidate_selection_sql_allows_grok_oauth_chat_auth() {
        let requested_model_sql = requested_model_selection_sql();
        for sql in [
            LIST_FOR_EXACT_API_FORMAT_SQL,
            LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL,
            LIST_POOL_KEYS_FOR_GROUP_SQL,
            requested_model_sql.as_str(),
        ] {
            assert!(sql.contains("LOWER(BTRIM(p.provider_type)) = 'grok'"));
            assert!(sql.contains("LOWER(BTRIM(pak.auth_type)) = 'oauth'"));
            assert!(sql
                .contains("'openai:chat', 'openai:responses', 'claude:messages', 'openai:image'"));
            assert!(sql.contains("'grok',"));
        }
    }

    #[test]
    fn candidate_selection_sql_allows_windsurf_openai_chat_managed_keys() {
        let requested_model_sql = requested_model_selection_sql();
        for sql in [
            LIST_FOR_EXACT_API_FORMAT_SQL,
            LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL,
            LIST_POOL_KEYS_FOR_GROUP_SQL,
            requested_model_sql.as_str(),
        ] {
            assert!(sql.contains("LOWER(BTRIM(p.provider_type)) = 'windsurf'"));
            assert!(
                sql.contains("LOWER($3) = 'openai:chat'")
                    || sql.contains("LOWER($4) = 'openai:chat'")
                    || sql.contains("LOWER($6) = 'openai:chat'")
            );
            assert!(sql.contains("LOWER(BTRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer')"));
            assert!(sql.contains("'windsurf'"));
        }
    }

    #[test]
    fn candidate_selection_sql_allows_vertex_embedding_auth() {
        let requested_model_sql = requested_model_selection_sql();
        for sql in [
            LIST_FOR_EXACT_API_FORMAT_SQL,
            LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL,
            LIST_POOL_KEYS_FOR_GROUP_SQL,
            requested_model_sql.as_str(),
        ] {
            assert!(sql.contains("LOWER(BTRIM(p.provider_type)) = 'vertex_ai'"));
            assert!(sql.contains("gemini:embedding"));
            assert!(sql.contains("gemini:generate_content"));
            assert!(sql.contains("claude:messages"));
        }
    }

    #[test]
    fn requested_model_selection_page_sql_adds_limit_and_offset() {
        let sql = requested_model_selection_page_sql();

        assert!(sql.ends_with("LIMIT $5\nOFFSET $6"));
    }

    #[test]
    fn pool_key_selection_sql_applies_query_order() {
        let load_balance_sql =
            pool_key_candidate_selection_sql(&StoredPoolKeyCandidateOrder::LoadBalance {
                seed: "seed".to_string(),
            });
        assert!(load_balance_sql.contains("md5($9 || ':' || pak.id) ASC"));
        assert!(load_balance_sql.ends_with("LIMIT $7\nOFFSET $8\n"));

        let lru_sql = pool_key_candidate_selection_sql(&StoredPoolKeyCandidateOrder::Lru);
        assert!(lru_sql.contains("pak.last_used_at ASC NULLS FIRST"));

        let cache_affinity_sql =
            pool_key_candidate_selection_sql(&StoredPoolKeyCandidateOrder::CacheAffinity);
        assert!(cache_affinity_sql.contains("pak.last_used_at DESC NULLS LAST"));
    }

    #[test]
    fn parse_string_list_accepts_stringified_array() {
        let parsed = parse_string_list(
            Some(json!("[\"gpt-5.2\", \"gpt-5\"]")),
            "provider_api_keys.allowed_models",
        )
        .expect("stringified array should parse");

        assert_eq!(
            parsed,
            Some(vec!["gpt-5.2".to_string(), "gpt-5".to_string()])
        );
    }

    #[test]
    fn parse_string_list_accepts_single_string() {
        let parsed = parse_string_list(Some(json!("gpt-5.2")), "provider_api_keys.allowed_models")
            .expect("single string should parse");

        assert_eq!(parsed, Some(vec!["gpt-5.2".to_string()]));
    }

    #[test]
    fn parse_provider_model_mappings_accepts_stringified_array() {
        let parsed = parse_provider_model_mappings(Some(json!(
            "[{\"name\":\"gpt-5.2\",\"priority\":2,\"api_formats\":[\"openai:chat\"]}]"
        )))
        .expect("stringified provider_model_mappings should parse");

        assert_eq!(
            parsed,
            Some(vec![StoredProviderModelMapping {
                name: "gpt-5.2".to_string(),
                priority: 2,
                api_formats: Some(vec!["openai:chat".to_string()]),
                endpoint_ids: None,
            }])
        );
    }

    #[test]
    fn parse_provider_model_mappings_accepts_single_string_alias() {
        let parsed = parse_provider_model_mappings(Some(json!("gpt-5.2")))
            .expect("single-string provider_model_mappings should parse");

        assert_eq!(
            parsed,
            Some(vec![StoredProviderModelMapping {
                name: "gpt-5.2".to_string(),
                priority: 1,
                api_formats: None,
                endpoint_ids: None,
            }])
        );
    }

    #[test]
    fn parse_provider_model_mappings_skips_invalid_array_items() {
        let parsed = parse_provider_model_mappings(Some(json!([
            {"name": "gpt-5.2", "priority": 1},
            {"priority": 2},
            3,
            null,
            "gpt-5.2-mini"
        ])))
        .expect("mixed provider_model_mappings should parse");

        assert_eq!(
            parsed,
            Some(vec![
                StoredProviderModelMapping {
                    name: "gpt-5.2".to_string(),
                    priority: 1,
                    api_formats: None,
                    endpoint_ids: None,
                },
                StoredProviderModelMapping {
                    name: "gpt-5.2-mini".to_string(),
                    priority: 1,
                    api_formats: None,
                    endpoint_ids: None,
                }
            ])
        );
    }
}
