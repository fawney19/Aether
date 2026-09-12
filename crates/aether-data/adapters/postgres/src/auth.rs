use async_trait::async_trait;
use futures_util::{stream::TryStream, TryStreamExt};
use sqlx::{postgres::PgRow, PgPool, Row};

use aether_data_contracts::repository::auth::{
    AuthApiKeyExportSummary, AuthApiKeyLookupKey, AuthApiKeyReadRepository,
    AuthApiKeyWriteRepository, CompareAndSwapAuthApiKeyCiphertext, CreateStandaloneApiKeyRecord,
    CreateUserApiKeyRecord, StandaloneApiKeyExportListQuery, StoredAuthApiKeyExportRecord,
    StoredAuthApiKeySnapshot, UpdateStandaloneApiKeyBasicRecord, UpdateUserApiKeyBasicRecord,
};
use aether_data_contracts::DataLayerError;

use crate::error::{postgres_error, SqlxResultExt};

const FIND_BY_KEY_HASH_SQL: &str = r#"
SELECT
  users.id AS user_id,
  users.username,
  users.email,
  users.role::text AS user_role,
  users.auth_source::text AS user_auth_source,
  users.is_active AS user_is_active,
  users.is_deleted AS user_is_deleted,
  users.rate_limit AS user_rate_limit,
  users.allowed_providers AS user_allowed_providers,
  users.allowed_api_formats AS user_allowed_api_formats,
  users.allowed_models AS user_allowed_models,
  api_keys.id AS api_key_id,
  api_keys.name AS api_key_name,
  api_keys.is_active AS api_key_is_active,
  api_keys.is_locked AS api_key_is_locked,
  api_keys.is_standalone AS api_key_is_standalone,
  api_keys.rate_limit AS api_key_rate_limit,
  api_keys.concurrent_limit AS api_key_concurrent_limit,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS api_key_expires_at_unix_secs,
  api_keys.allowed_providers AS api_key_allowed_providers,
  api_keys.allowed_api_formats AS api_key_allowed_api_formats,
  api_keys.allowed_models AS api_key_allowed_models,
  api_keys.ip_rules AS api_key_ip_rules
FROM api_keys
JOIN users ON users.id = api_keys.user_id
WHERE api_keys.key_hash = $1
LIMIT 1
"#;

const FIND_BY_API_KEY_ID_SQL: &str = r#"
SELECT
  users.id AS user_id,
  users.username,
  users.email,
  users.role::text AS user_role,
  users.auth_source::text AS user_auth_source,
  users.is_active AS user_is_active,
  users.is_deleted AS user_is_deleted,
  users.rate_limit AS user_rate_limit,
  users.allowed_providers AS user_allowed_providers,
  users.allowed_api_formats AS user_allowed_api_formats,
  users.allowed_models AS user_allowed_models,
  api_keys.id AS api_key_id,
  api_keys.name AS api_key_name,
  api_keys.is_active AS api_key_is_active,
  api_keys.is_locked AS api_key_is_locked,
  api_keys.is_standalone AS api_key_is_standalone,
  api_keys.rate_limit AS api_key_rate_limit,
  api_keys.concurrent_limit AS api_key_concurrent_limit,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS api_key_expires_at_unix_secs,
  api_keys.allowed_providers AS api_key_allowed_providers,
  api_keys.allowed_api_formats AS api_key_allowed_api_formats,
  api_keys.allowed_models AS api_key_allowed_models,
  api_keys.ip_rules AS api_key_ip_rules
FROM api_keys
JOIN users ON users.id = api_keys.user_id
WHERE api_keys.id = $1
LIMIT 1
"#;

const FIND_BY_USER_API_KEY_IDS_SQL: &str = r#"
SELECT
  users.id AS user_id,
  users.username,
  users.email,
  users.role::text AS user_role,
  users.auth_source::text AS user_auth_source,
  users.is_active AS user_is_active,
  users.is_deleted AS user_is_deleted,
  users.rate_limit AS user_rate_limit,
  users.allowed_providers AS user_allowed_providers,
  users.allowed_api_formats AS user_allowed_api_formats,
  users.allowed_models AS user_allowed_models,
  api_keys.id AS api_key_id,
  api_keys.name AS api_key_name,
  api_keys.is_active AS api_key_is_active,
  api_keys.is_locked AS api_key_is_locked,
  api_keys.is_standalone AS api_key_is_standalone,
  api_keys.rate_limit AS api_key_rate_limit,
  api_keys.concurrent_limit AS api_key_concurrent_limit,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS api_key_expires_at_unix_secs,
  api_keys.allowed_providers AS api_key_allowed_providers,
  api_keys.allowed_api_formats AS api_key_allowed_api_formats,
  api_keys.allowed_models AS api_key_allowed_models,
  api_keys.ip_rules AS api_key_ip_rules
FROM api_keys
JOIN users ON users.id = api_keys.user_id
WHERE api_keys.id = $1 AND users.id = $2
LIMIT 1
"#;

const LIST_BY_API_KEY_IDS_SQL: &str = r#"
SELECT
  users.id AS user_id,
  users.username,
  users.email,
  users.role::text AS user_role,
  users.auth_source::text AS user_auth_source,
  users.is_active AS user_is_active,
  users.is_deleted AS user_is_deleted,
  users.rate_limit AS user_rate_limit,
  users.allowed_providers AS user_allowed_providers,
  users.allowed_api_formats AS user_allowed_api_formats,
  users.allowed_models AS user_allowed_models,
  api_keys.id AS api_key_id,
  api_keys.name AS api_key_name,
  api_keys.is_active AS api_key_is_active,
  api_keys.is_locked AS api_key_is_locked,
  api_keys.is_standalone AS api_key_is_standalone,
  api_keys.rate_limit AS api_key_rate_limit,
  api_keys.concurrent_limit AS api_key_concurrent_limit,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS api_key_expires_at_unix_secs,
  api_keys.allowed_providers AS api_key_allowed_providers,
  api_keys.allowed_api_formats AS api_key_allowed_api_formats,
  api_keys.allowed_models AS api_key_allowed_models,
  api_keys.ip_rules AS api_key_ip_rules
FROM api_keys
JOIN users ON users.id = api_keys.user_id
WHERE api_keys.id = ANY($1::TEXT[])
ORDER BY api_keys.id ASC
"#;

const LIST_EXPORT_BY_USER_IDS_SQL: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.ip_rules,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(api_keys.total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM api_keys.last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.updated_at) AS BIGINT) AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
WHERE api_keys.user_id = ANY($1::TEXT[])
  AND api_keys.is_standalone = FALSE
ORDER BY api_keys.user_id ASC, api_keys.id ASC
"#;

const LIST_EXPORT_BY_API_KEY_IDS_SQL: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.ip_rules,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(api_keys.total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM api_keys.last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.updated_at) AS BIGINT) AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
WHERE api_keys.id = ANY($1::TEXT[])
ORDER BY api_keys.id ASC
"#;

const RESTORE_API_KEY_SELECT_SQL: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.ip_rules,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(api_keys.total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM api_keys.last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.updated_at) AS BIGINT) AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
WHERE api_keys.id = $1
LIMIT 1
FOR UPDATE
"#;

const LIST_EXPORT_BY_NAME_SEARCH_SQL: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.ip_rules,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(api_keys.total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM api_keys.last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.updated_at) AS BIGINT) AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
WHERE LOWER(COALESCE(api_keys.name, '')) LIKE $1
ORDER BY api_keys.id ASC
"#;

const LIST_EXPORT_STANDALONE_SQL: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.ip_rules,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(api_keys.total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM api_keys.last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.updated_at) AS BIGINT) AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
WHERE api_keys.is_standalone = TRUE
ORDER BY api_keys.id ASC
"#;

const LIST_EXPORT_STANDALONE_PAGE_SQL: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.ip_rules,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(api_keys.total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM api_keys.last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.updated_at) AS BIGINT) AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
WHERE api_keys.is_standalone = TRUE
  AND ($1::BOOLEAN IS NULL OR api_keys.is_active = $1)
ORDER BY api_keys.id ASC
OFFSET $2
LIMIT $3
"#;

const COUNT_EXPORT_STANDALONE_SQL: &str = r#"
SELECT COUNT(*)::BIGINT AS total
FROM api_keys
WHERE api_keys.is_standalone = TRUE
  AND ($1::BOOLEAN IS NULL OR api_keys.is_active = $1)
"#;

const SUMMARIZE_EXPORT_BY_USER_IDS_SQL: &str = r#"
SELECT
  COUNT(*)::BIGINT AS total,
  COUNT(*) FILTER (
    WHERE is_active = TRUE
      AND (expires_at IS NULL OR expires_at >= TO_TIMESTAMP($2::double precision))
  )::BIGINT AS active
FROM api_keys
WHERE user_id = ANY($1::TEXT[])
  AND is_standalone = FALSE
"#;

const SUMMARIZE_EXPORT_NON_STANDALONE_SQL: &str = r#"
SELECT
  COUNT(*)::BIGINT AS total,
  COUNT(*) FILTER (
    WHERE is_active = TRUE
      AND (expires_at IS NULL OR expires_at >= TO_TIMESTAMP($1::double precision))
  )::BIGINT AS active
FROM api_keys
WHERE is_standalone = FALSE
"#;

const SUMMARIZE_EXPORT_STANDALONE_SQL: &str = r#"
SELECT
  COUNT(*)::BIGINT AS total,
  COUNT(*) FILTER (
    WHERE is_active = TRUE
      AND (expires_at IS NULL OR expires_at >= TO_TIMESTAMP($1::double precision))
  )::BIGINT AS active
FROM api_keys
WHERE is_standalone = TRUE
"#;

const FIND_EXPORT_STANDALONE_BY_ID_SQL: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.ip_rules,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  CAST(EXTRACT(EPOCH FROM api_keys.expires_at) AS BIGINT) AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(api_keys.total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM api_keys.last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM api_keys.updated_at) AS BIGINT) AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
WHERE api_keys.is_standalone = TRUE
  AND api_keys.id = $1
LIMIT 1
"#;

const TOUCH_LAST_USED_AT_SQL: &str = r#"
UPDATE api_keys
SET last_used_at = NOW()
WHERE id = $1
"#;

const CREATE_USER_API_KEY_SQL: &str = r#"
INSERT INTO api_keys (
  id,
  user_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  expires_at,
  auto_delete_on_expiry,
  is_locked,
  is_standalone,
  total_requests,
  total_tokens,
  total_cost_usd,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  $14,
  $15,
  $16,
  FALSE,
  FALSE,
  $17,
  $18,
  $19,
  NOW(),
  NOW()
)
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const CREATE_STANDALONE_API_KEY_SQL: &str = r#"
INSERT INTO api_keys (
  id,
  user_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  expires_at,
  auto_delete_on_expiry,
  is_locked,
  is_standalone,
  total_requests,
  total_tokens,
  total_cost_usd,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  NULL,
  $13,
  $14,
  $15,
  FALSE,
  TRUE,
  $16,
  $17,
  $18,
  NOW(),
  NOW()
)
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const UPDATE_USER_API_KEY_BASIC_SQL: &str = r#"
UPDATE api_keys
SET
  key_encrypted = CASE WHEN $3 THEN $4 ELSE key_encrypted END,
  name = CASE WHEN $5 THEN $6 ELSE name END,
  rate_limit = CASE WHEN $7 THEN $8 ELSE rate_limit END,
  concurrent_limit = CASE WHEN $9 THEN $10 ELSE concurrent_limit END,
  ip_rules = CASE WHEN $11 THEN $12::jsonb ELSE ip_rules END,
  feature_settings = CASE WHEN $13 THEN $14::jsonb ELSE feature_settings END,
  updated_at = NOW()
WHERE user_id = $1
  AND id = $2
  AND is_standalone = FALSE
  AND ($15 = FALSE OR is_locked = FALSE)
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const UPDATE_STANDALONE_API_KEY_BASIC_SQL: &str = r#"
UPDATE api_keys
SET
  key_encrypted = CASE WHEN $2 THEN $3 ELSE key_encrypted END,
  name = CASE WHEN $4 THEN $5 ELSE name END,
  force_capabilities = CASE WHEN $6 THEN $7::json ELSE force_capabilities END,
  rate_limit = CASE WHEN $8 THEN $9 ELSE rate_limit END,
  concurrent_limit = CASE WHEN $10 THEN $11 ELSE concurrent_limit END,
  allowed_providers = CASE WHEN $12 THEN $13::json ELSE allowed_providers END,
  allowed_api_formats = CASE WHEN $14 THEN $15::json ELSE allowed_api_formats END,
  allowed_models = CASE WHEN $16 THEN $17::json ELSE allowed_models END,
  ip_rules = CASE WHEN $18 THEN $19::jsonb ELSE ip_rules END,
  expires_at = CASE WHEN $20 THEN $21::timestamptz ELSE expires_at END,
  auto_delete_on_expiry = CASE WHEN $22 THEN $23 ELSE auto_delete_on_expiry END,
  updated_at = NOW()
WHERE id = $1
  AND is_standalone = TRUE
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const SET_USER_API_KEY_ACTIVE_SQL: &str = r#"
UPDATE api_keys
SET
  is_active = $3,
  updated_at = NOW()
WHERE user_id = $1
  AND id = $2
  AND is_standalone = FALSE
  AND ($4 = FALSE OR is_locked = FALSE)
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const SET_STANDALONE_API_KEY_ACTIVE_SQL: &str = r#"
UPDATE api_keys
SET
  is_active = $2,
  updated_at = NOW()
WHERE id = $1
  AND is_standalone = TRUE
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const SET_API_KEY_USAGE_TOTALS_SQL: &str = r#"
UPDATE api_keys
SET
  total_requests = $2,
  total_tokens = $3,
  total_cost_usd = $4,
  updated_at = NOW()
WHERE id = $1
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const SET_USER_API_KEY_LOCKED_SQL: &str = r#"
UPDATE api_keys
SET
  is_locked = $3,
  updated_at = NOW()
WHERE user_id = $1
  AND id = $2
  AND is_standalone = FALSE
"#;

const SET_USER_API_KEY_ALLOWED_PROVIDERS_SQL: &str = r#"
UPDATE api_keys
SET
  allowed_providers = $3,
  updated_at = NOW()
WHERE user_id = $1
  AND id = $2
  AND is_standalone = FALSE
  AND ($4 = FALSE OR is_locked = FALSE)
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const SET_USER_API_KEY_FORCE_CAPABILITIES_SQL: &str = r#"
UPDATE api_keys
SET
  force_capabilities = $3,
  updated_at = NOW()
WHERE user_id = $1
  AND id = $2
  AND is_standalone = FALSE
  AND ($4 = FALSE OR is_locked = FALSE)
RETURNING
  user_id,
  id AS api_key_id,
  key_hash,
  key_encrypted,
  name,
  allowed_providers,
  allowed_api_formats,
  allowed_models,
  ip_rules,
  rate_limit,
  concurrent_limit,
  force_capabilities,
  feature_settings,
  is_active,
  CAST(EXTRACT(EPOCH FROM expires_at) AS BIGINT) AS expires_at_unix_secs,
  auto_delete_on_expiry,
  total_requests,
  COALESCE(total_tokens, 0)::BIGINT AS total_tokens,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) AS last_used_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs,
  is_standalone
"#;

const SET_USER_API_KEY_FEATURE_SETTINGS_SQL: &str = r#"
UPDATE api_keys
SET
  feature_settings = $3,
  updated_at = NOW()
WHERE user_id = $1
  AND id = $2
  AND is_standalone = FALSE
  AND ($4 = FALSE OR is_locked = FALSE)
"#;

const SET_STANDALONE_API_KEY_FEATURE_SETTINGS_SQL: &str = r#"
UPDATE api_keys
SET
  feature_settings = $2,
  updated_at = NOW()
WHERE id = $1
  AND is_standalone = TRUE
"#;

const POSTGRES_ANONYMIZE_API_KEY_HISTORY_SQL: &[&str] = &[
    "UPDATE request_candidates SET api_key_name = NULL WHERE api_key_id = $1",
    "UPDATE video_tasks SET api_key_name = NULL WHERE api_key_id = $1",
    "UPDATE usage SET api_key_name = NULL WHERE api_key_id = $1",
    "UPDATE stats_daily_api_key SET api_key_name = NULL WHERE api_key_id = $1",
    "UPDATE audit_logs SET description = 'deleted API key event', ip_address = NULL, user_agent = NULL, event_metadata = NULL, error_message = NULL WHERE api_key_id = $1",
    "UPDATE wallet_transactions SET description = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE api_key_id = $1)",
    "UPDATE payment_callbacks SET payload = NULL, error_message = NULL WHERE EXISTS (SELECT 1 FROM payment_orders AS history_order JOIN wallets AS history_wallet ON history_wallet.id = history_order.wallet_id WHERE history_wallet.api_key_id = $1 AND (history_order.id = payment_callbacks.payment_order_id OR (payment_callbacks.order_no IS NOT NULL AND history_order.order_no = payment_callbacks.order_no)))",
    "UPDATE payment_orders SET gateway_response = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE api_key_id = $1)",
    "UPDATE refund_requests SET reason = NULL, payout_reference = NULL, payout_proof = NULL, failure_reason = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE api_key_id = $1)",
];

const POSTGRES_DELETE_API_KEY_DEPENDENTS_SQL: &[&str] =
    &["DELETE FROM api_key_provider_mappings WHERE api_key_id = $1"];

#[derive(Debug, Clone)]
pub struct SqlxAuthApiKeySnapshotReadRepository {
    pool: PgPool,
}

impl SqlxAuthApiKeySnapshotReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn collect_query_rows<T, S>(
        mut rows: S,
        map_row: fn(&PgRow) -> Result<T, DataLayerError>,
    ) -> Result<Vec<T>, DataLayerError>
    where
        S: TryStream<Ok = PgRow, Error = sqlx::Error> + Unpin,
    {
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_row(&row)?);
        }
        Ok(items)
    }

    pub async fn find_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        let row = match key {
            AuthApiKeyLookupKey::KeyHash(key_hash) => sqlx::query(FIND_BY_KEY_HASH_SQL)
                .bind(key_hash)
                .fetch_optional(&self.pool)
                .await
                .map_postgres_err()?,
            AuthApiKeyLookupKey::ApiKeyId(api_key_id) => sqlx::query(FIND_BY_API_KEY_ID_SQL)
                .bind(api_key_id)
                .fetch_optional(&self.pool)
                .await
                .map_postgres_err()?,
            AuthApiKeyLookupKey::UserApiKeyIds {
                user_id,
                api_key_id,
            } => sqlx::query(FIND_BY_USER_API_KEY_IDS_SQL)
                .bind(api_key_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await
                .map_postgres_err()?,
        };

        row.as_ref().map(map_auth_api_key_snapshot_row).transpose()
    }

    pub async fn list_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }

        Self::collect_query_rows(
            sqlx::query(LIST_BY_API_KEY_IDS_SQL)
                .bind(api_key_ids)
                .fetch(&self.pool),
            map_auth_api_key_snapshot_row,
        )
        .await
    }

    pub async fn list_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        Self::collect_query_rows(
            sqlx::query(LIST_EXPORT_BY_USER_IDS_SQL)
                .bind(user_ids)
                .fetch(&self.pool),
            map_auth_api_key_export_row,
        )
        .await
    }

    pub async fn list_export_api_keys_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }

        Self::collect_query_rows(
            sqlx::query(LIST_EXPORT_BY_API_KEY_IDS_SQL)
                .bind(api_key_ids)
                .fetch(&self.pool),
            map_auth_api_key_export_row,
        )
        .await
    }

    pub async fn list_export_api_keys_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let name_search = name_search.trim();
        if name_search.is_empty() {
            return Ok(Vec::new());
        }

        Self::collect_query_rows(
            sqlx::query(LIST_EXPORT_BY_NAME_SEARCH_SQL)
                .bind(format!("%{}%", name_search.to_ascii_lowercase()))
                .fetch(&self.pool),
            map_auth_api_key_export_row,
        )
        .await
    }

    pub async fn summarize_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(AuthApiKeyExportSummary::default());
        }
        let now_unix_secs =
            datetime_from_unix_secs(now_unix_secs, "api_keys.summary_now")?.timestamp() as f64;

        let row = sqlx::query(SUMMARIZE_EXPORT_BY_USER_IDS_SQL)
            .bind(user_ids)
            .bind(now_unix_secs)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(AuthApiKeyExportSummary {
            total: row_get::<i64>(&row, "total")?.max(0) as u64,
            active: row_get::<i64>(&row, "active")?.max(0) as u64,
        })
    }

    pub async fn summarize_export_non_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        let now_unix_secs =
            datetime_from_unix_secs(now_unix_secs, "api_keys.summary_now")?.timestamp() as f64;
        let row = sqlx::query(SUMMARIZE_EXPORT_NON_STANDALONE_SQL)
            .bind(now_unix_secs)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(AuthApiKeyExportSummary {
            total: row_get::<i64>(&row, "total")?.max(0) as u64,
            active: row_get::<i64>(&row, "active")?.max(0) as u64,
        })
    }

    pub async fn list_export_standalone_api_keys(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Self::collect_query_rows(
            sqlx::query(LIST_EXPORT_STANDALONE_SQL).fetch(&self.pool),
            map_auth_api_key_export_row,
        )
        .await
    }

    pub async fn list_export_standalone_api_keys_page(
        &self,
        query: &StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let limit = i64::try_from(query.limit)
            .map_err(|_| DataLayerError::InvalidInput("limit is too large".to_string()))?;
        let skip = i64::try_from(query.skip)
            .map_err(|_| DataLayerError::InvalidInput("skip is too large".to_string()))?;
        Self::collect_query_rows(
            sqlx::query(LIST_EXPORT_STANDALONE_PAGE_SQL)
                .bind(query.is_active)
                .bind(skip)
                .bind(limit)
                .fetch(&self.pool),
            map_auth_api_key_export_row,
        )
        .await
    }

    pub async fn count_export_standalone_api_keys(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, DataLayerError> {
        let row = sqlx::query(COUNT_EXPORT_STANDALONE_SQL)
            .bind(is_active)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row_get::<i64>(&row, "total")?.max(0) as u64)
    }

    pub async fn summarize_export_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        let now_unix_secs =
            datetime_from_unix_secs(now_unix_secs, "api_keys.summary_now")?.timestamp() as f64;
        let row = sqlx::query(SUMMARIZE_EXPORT_STANDALONE_SQL)
            .bind(now_unix_secs)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(AuthApiKeyExportSummary {
            total: row_get::<i64>(&row, "total")?.max(0) as u64,
            active: row_get::<i64>(&row, "active")?.max(0) as u64,
        })
    }

    pub async fn find_export_standalone_api_key_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let row = sqlx::query(FIND_EXPORT_STANDALONE_BY_ID_SQL)
            .bind(api_key_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }
}

#[async_trait]
impl AuthApiKeyReadRepository for SqlxAuthApiKeySnapshotReadRepository {
    async fn find_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        Self::find_api_key_snapshot(self, key).await
    }

    async fn list_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        Self::list_api_key_snapshots_by_ids(self, api_key_ids).await
    }

    async fn list_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Self::list_export_api_keys_by_user_ids(self, user_ids).await
    }

    async fn list_export_api_keys_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Self::list_export_api_keys_by_ids(self, api_key_ids).await
    }

    async fn list_export_api_keys_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Self::list_export_api_keys_by_name_search(self, name_search).await
    }

    async fn list_export_standalone_api_keys_page(
        &self,
        query: &StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Self::list_export_standalone_api_keys_page(self, query).await
    }

    async fn count_export_standalone_api_keys(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, DataLayerError> {
        Self::count_export_standalone_api_keys(self, is_active).await
    }

    async fn summarize_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        Self::summarize_export_api_keys_by_user_ids(self, user_ids, now_unix_secs).await
    }

    async fn summarize_export_non_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        Self::summarize_export_non_standalone_api_keys(self, now_unix_secs).await
    }

    async fn summarize_export_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        Self::summarize_export_standalone_api_keys(self, now_unix_secs).await
    }

    async fn find_export_standalone_api_key_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Self::find_export_standalone_api_key_by_id(self, api_key_id).await
    }

    async fn list_export_standalone_api_keys(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Self::list_export_standalone_api_keys(self).await
    }
}

#[async_trait]
impl AuthApiKeyWriteRepository for SqlxAuthApiKeySnapshotReadRepository {
    async fn touch_last_used_at(&self, api_key_id: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query(TOUCH_LAST_USED_AT_SQL)
            .bind(api_key_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn create_user_api_key(
        &self,
        record: CreateUserApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let allowed_providers = record
            .allowed_providers
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_api_formats = record
            .allowed_api_formats
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_models = record
            .allowed_models
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let ip_rules = record
            .ip_rules
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let expires_at = record
            .expires_at_unix_secs
            .map(|value| datetime_from_unix_secs(value, "api_keys.expires_at"))
            .transpose()?;
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let owner_exists: Option<String> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id = $1 AND is_deleted IS FALSE FOR UPDATE",
        )
        .bind(&record.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if owner_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
        let row = sqlx::query(CREATE_USER_API_KEY_SQL)
            .bind(record.api_key_id)
            .bind(record.user_id)
            .bind(record.key_hash)
            .bind(record.key_encrypted)
            .bind(record.name)
            .bind(allowed_providers)
            .bind(allowed_api_formats)
            .bind(allowed_models)
            .bind(ip_rules)
            .bind(record.rate_limit)
            .bind(record.concurrent_limit)
            .bind(record.force_capabilities)
            .bind(record.feature_settings)
            .bind(record.is_active)
            .bind(expires_at)
            .bind(record.auto_delete_on_expiry)
            .bind(i64_from_u64(
                record.total_requests,
                "api_keys.total_requests",
            )?)
            .bind(i64_from_u64(record.total_tokens, "api_keys.total_tokens")?)
            .bind(record.total_cost_usd)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
        let record = row.as_ref().map(map_auth_api_key_export_row).transpose()?;
        tx.commit().await.map_err(postgres_error)?;
        Ok(record)
    }

    async fn create_standalone_api_key(
        &self,
        record: CreateStandaloneApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let allowed_providers = record
            .allowed_providers
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_api_formats = record
            .allowed_api_formats
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_models = record
            .allowed_models
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let ip_rules = record
            .ip_rules
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let expires_at = record
            .expires_at_unix_secs
            .map(|value| datetime_from_unix_secs(value, "api_keys.expires_at"))
            .transpose()?;
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let owner_exists: Option<String> = sqlx::query_scalar(
            "SELECT id FROM users WHERE id = $1 AND is_deleted IS FALSE FOR UPDATE",
        )
        .bind(&record.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if owner_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
        let row = sqlx::query(CREATE_STANDALONE_API_KEY_SQL)
            .bind(record.api_key_id)
            .bind(record.user_id)
            .bind(record.key_hash)
            .bind(record.key_encrypted)
            .bind(record.name)
            .bind(allowed_providers)
            .bind(allowed_api_formats)
            .bind(allowed_models)
            .bind(ip_rules)
            .bind(record.rate_limit)
            .bind(record.concurrent_limit)
            .bind(record.force_capabilities)
            .bind(record.is_active)
            .bind(expires_at)
            .bind(record.auto_delete_on_expiry)
            .bind(i64_from_u64(
                record.total_requests,
                "api_keys.total_requests",
            )?)
            .bind(i64_from_u64(record.total_tokens, "api_keys.total_tokens")?)
            .bind(record.total_cost_usd)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
        let record = row.as_ref().map(map_auth_api_key_export_row).transpose()?;
        tx.commit().await.map_err(postgres_error)?;
        Ok(record)
    }

    async fn update_user_api_key_basic(
        &self,
        record: UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let ip_rules = record
            .ip_rules
            .clone()
            .flatten()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let feature_settings = record.feature_settings.clone().flatten();
        let row = sqlx::query(UPDATE_USER_API_KEY_BASIC_SQL)
            .bind(record.user_id)
            .bind(record.api_key_id)
            .bind(record.key_encrypted_present)
            .bind(record.key_encrypted)
            .bind(record.name_present)
            .bind(record.name)
            .bind(record.rate_limit_present)
            .bind(record.rate_limit)
            .bind(record.concurrent_limit_present)
            .bind(record.concurrent_limit)
            .bind(record.ip_rules.is_some())
            .bind(ip_rules)
            .bind(record.feature_settings.is_some())
            .bind(feature_settings)
            .bind(false)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn compare_and_swap_api_key_ciphertext(
        &self,
        mutation: &CompareAndSwapAuthApiKeyCiphertext,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE api_keys
SET key_encrypted = $1
WHERE id = $2
  AND user_id = $3
  AND key_hash = $4
  AND is_standalone = $5
  AND key_encrypted = $6
"#,
        )
        .bind(&mutation.key_encrypted)
        .bind(&mutation.api_key_id)
        .bind(&mutation.user_id)
        .bind(&mutation.key_hash)
        .bind(mutation.is_standalone)
        .bind(&mutation.expected_key_encrypted)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(result.rows_affected() == 1)
    }

    async fn update_user_api_key_basic_if_unlocked(
        &self,
        record: UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let ip_rules = record
            .ip_rules
            .clone()
            .flatten()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let feature_settings = record.feature_settings.clone().flatten();
        let row = sqlx::query(UPDATE_USER_API_KEY_BASIC_SQL)
            .bind(record.user_id)
            .bind(record.api_key_id)
            .bind(record.key_encrypted_present)
            .bind(record.key_encrypted)
            .bind(record.name_present)
            .bind(record.name)
            .bind(record.rate_limit_present)
            .bind(record.rate_limit)
            .bind(record.concurrent_limit_present)
            .bind(record.concurrent_limit)
            .bind(record.ip_rules.is_some())
            .bind(ip_rules)
            .bind(record.feature_settings.is_some())
            .bind(feature_settings)
            .bind(true)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn update_standalone_api_key_basic(
        &self,
        record: UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let allowed_providers = record
            .allowed_providers
            .clone()
            .flatten()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_api_formats = record
            .allowed_api_formats
            .clone()
            .flatten()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_models = record
            .allowed_models
            .clone()
            .flatten()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let ip_rules = record
            .ip_rules
            .clone()
            .flatten()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let expires_at = record
            .expires_at_unix_secs
            .map(|value| datetime_from_unix_secs(value, "api_keys.expires_at"))
            .transpose()?;
        let row = sqlx::query(UPDATE_STANDALONE_API_KEY_BASIC_SQL)
            .bind(record.api_key_id)
            .bind(record.key_encrypted_present)
            .bind(record.key_encrypted)
            .bind(record.name_present)
            .bind(record.name)
            .bind(record.force_capabilities.is_some())
            .bind(record.force_capabilities.clone().flatten())
            .bind(record.rate_limit_present)
            .bind(record.rate_limit)
            .bind(record.concurrent_limit_present)
            .bind(record.concurrent_limit)
            .bind(record.allowed_providers.is_some())
            .bind(allowed_providers)
            .bind(record.allowed_api_formats.is_some())
            .bind(allowed_api_formats)
            .bind(record.allowed_models.is_some())
            .bind(allowed_models)
            .bind(record.ip_rules.is_some())
            .bind(ip_rules)
            .bind(record.expires_at_present)
            .bind(expires_at)
            .bind(record.auto_delete_on_expiry_present)
            .bind(record.auto_delete_on_expiry)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn restore_api_key_if_matches(
        &self,
        expected: &StoredAuthApiKeyExportRecord,
        restored: &StoredAuthApiKeyExportRecord,
    ) -> Result<bool, DataLayerError> {
        if restored.api_key_id != expected.api_key_id
            || restored.user_id != expected.user_id
            || restored.key_hash != expected.key_hash
            || restored.is_standalone != expected.is_standalone
        {
            return Ok(false);
        }

        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let row = sqlx::query(RESTORE_API_KEY_SELECT_SQL)
            .bind(&expected.api_key_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
        let Some(row) = row else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        let current = map_auth_api_key_export_row(&row)?;
        if current != *expected {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }

        let expires_at = restored
            .expires_at_unix_secs
            .map(|value| datetime_from_unix_secs(value, "api_keys.expires_at"))
            .transpose()?;
        let last_used_at = restored
            .last_used_at_unix_secs
            .map(|value| datetime_from_unix_secs(value, "api_keys.last_used_at"))
            .transpose()?;
        let allowed_providers = restored
            .allowed_providers
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_api_formats = restored
            .allowed_api_formats
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_models = restored
            .allowed_models
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let ip_rules = restored
            .ip_rules
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;

        let result = sqlx::query(
            r#"
UPDATE api_keys
SET key_encrypted = $1,
    name = $2,
    allowed_providers = $3::json,
    allowed_api_formats = $4::json,
    allowed_models = $5::json,
    ip_rules = $6::jsonb,
    rate_limit = $7,
    concurrent_limit = $8,
    force_capabilities = $9::json,
    feature_settings = $10::jsonb,
    is_active = $11,
    expires_at = $12,
    auto_delete_on_expiry = $13,
    total_requests = $14,
    total_tokens = $15,
    total_cost_usd = $16,
    last_used_at = $17,
    updated_at = NOW()
WHERE id = $18
  AND user_id = $19
  AND key_hash = $20
  AND is_standalone = $21
"#,
        )
        .bind(&restored.key_encrypted)
        .bind(&restored.name)
        .bind(allowed_providers)
        .bind(allowed_api_formats)
        .bind(allowed_models)
        .bind(ip_rules)
        .bind(restored.rate_limit)
        .bind(restored.concurrent_limit)
        .bind(&restored.force_capabilities)
        .bind(&restored.feature_settings)
        .bind(restored.is_active)
        .bind(expires_at)
        .bind(restored.auto_delete_on_expiry)
        .bind(i64_from_u64(
            restored.total_requests,
            "api_keys.total_requests",
        )?)
        .bind(i64_from_u64(
            restored.total_tokens,
            "api_keys.total_tokens",
        )?)
        .bind(restored.total_cost_usd)
        .bind(last_used_at)
        .bind(&restored.api_key_id)
        .bind(&restored.user_id)
        .bind(&restored.key_hash)
        .bind(restored.is_standalone)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        if result.rows_affected() != 1 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let row = sqlx::query(SET_USER_API_KEY_ACTIVE_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(is_active)
            .bind(false)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn set_user_api_key_active_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let row = sqlx::query(SET_USER_API_KEY_ACTIVE_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(is_active)
            .bind(true)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let row = sqlx::query(SET_STANDALONE_API_KEY_ACTIVE_SQL)
            .bind(api_key_id)
            .bind(is_active)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(SET_USER_API_KEY_LOCKED_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(is_locked)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let allowed_providers = allowed_providers
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let row = sqlx::query(SET_USER_API_KEY_ALLOWED_PROVIDERS_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(allowed_providers)
            .bind(false)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn set_user_api_key_allowed_providers_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let allowed_providers = allowed_providers
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let row = sqlx::query(SET_USER_API_KEY_ALLOWED_PROVIDERS_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(allowed_providers)
            .bind(true)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let row = sqlx::query(SET_USER_API_KEY_FORCE_CAPABILITIES_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(force_capabilities)
            .bind(false)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn set_user_api_key_force_capabilities_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let row = sqlx::query(SET_USER_API_KEY_FORCE_CAPABILITIES_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(force_capabilities)
            .bind(true)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn set_user_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let result = sqlx::query(SET_USER_API_KEY_FEATURE_SETTINGS_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(feature_settings)
            .bind(false)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        let api_key_ids = [api_key_id.to_string()];
        Ok(self
            .list_export_api_keys_by_ids(&api_key_ids)
            .await?
            .into_iter()
            .find(|record| record.user_id == user_id && !record.is_standalone))
    }

    async fn set_user_api_key_feature_settings_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let result = sqlx::query(SET_USER_API_KEY_FEATURE_SETTINGS_SQL)
            .bind(user_id)
            .bind(api_key_id)
            .bind(feature_settings)
            .bind(true)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        let api_key_ids = [api_key_id.to_string()];
        Ok(self
            .list_export_api_keys_by_ids(&api_key_ids)
            .await?
            .into_iter()
            .find(|record| record.user_id == user_id && !record.is_standalone))
    }

    async fn set_api_key_usage_totals(
        &self,
        api_key_id: &str,
        total_requests: u64,
        total_tokens: u64,
        total_cost_usd: f64,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        if !total_cost_usd.is_finite() {
            return Err(DataLayerError::InvalidInput(
                "api_keys.total_cost_usd is not finite".to_string(),
            ));
        }
        let row = sqlx::query(SET_API_KEY_USAGE_TOTALS_SQL)
            .bind(api_key_id)
            .bind(i64_from_u64(total_requests, "api_keys.total_requests")?)
            .bind(i64_from_u64(total_tokens, "api_keys.total_tokens")?)
            .bind(total_cost_usd)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_auth_api_key_export_row).transpose()
    }

    async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        self.delete_api_key(api_key_id, Some(user_id), false, false)
            .await
    }

    async fn delete_user_api_key_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        self.delete_api_key(api_key_id, Some(user_id), false, true)
            .await
    }

    async fn set_standalone_api_key_feature_settings(
        &self,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let result = sqlx::query(SET_STANDALONE_API_KEY_FEATURE_SETTINGS_SQL)
            .bind(api_key_id)
            .bind(feature_settings)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        let api_key_ids = [api_key_id.to_string()];
        Ok(self
            .list_export_api_keys_by_ids(&api_key_ids)
            .await?
            .into_iter()
            .find(|record| record.is_standalone))
    }

    async fn delete_standalone_api_key(&self, api_key_id: &str) -> Result<bool, DataLayerError> {
        self.delete_api_key(api_key_id, None, true, false).await
    }
}

impl SqlxAuthApiKeySnapshotReadRepository {
    async fn delete_api_key(
        &self,
        api_key_id: &str,
        user_id: Option<&str>,
        is_standalone: bool,
        require_unlocked: bool,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let matching_api_key = if let Some(user_id) = user_id {
            if require_unlocked {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM api_keys WHERE id = $1 AND user_id = $2 AND is_standalone IS FALSE AND is_locked IS FALSE FOR UPDATE",
                )
                .bind(api_key_id)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?
            } else {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM api_keys WHERE id = $1 AND user_id = $2 AND is_standalone IS FALSE FOR UPDATE",
                )
                .bind(api_key_id)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?
            }
        } else {
            sqlx::query_scalar::<_, String>(
                "SELECT id FROM api_keys WHERE id = $1 AND is_standalone IS TRUE FOR UPDATE",
            )
            .bind(api_key_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?
        };
        if matching_api_key.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }

        sqlx::query(
            "UPDATE wallets SET status = 'disabled', updated_at = NOW() WHERE api_key_id = $1 AND status <> 'disabled'",
        )
        .bind(api_key_id)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        for sql in POSTGRES_ANONYMIZE_API_KEY_HISTORY_SQL {
            sqlx::query(sql)
                .bind(api_key_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        for sql in POSTGRES_DELETE_API_KEY_DEPENDENTS_SQL {
            sqlx::query(sql)
                .bind(api_key_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        let result = sqlx::query("DELETE FROM api_keys WHERE id = $1 AND is_standalone = $2")
            .bind(api_key_id)
            .bind(is_standalone)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        if result.rows_affected() != 1 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        tx.commit().await.map_err(postgres_error)?;
        Ok(true)
    }
}

fn i64_from_u64(value: u64, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field_name} exceeds i64: {value}")))
}

fn datetime_from_unix_secs(
    value: u64,
    field_name: &str,
) -> Result<chrono::DateTime<chrono::Utc>, DataLayerError> {
    let unix_secs = i64_from_u64(value, field_name)?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_secs, 0).ok_or_else(|| {
        DataLayerError::InvalidInput(format!(
            "{field_name} is outside the supported timestamp range: {value}"
        ))
    })
}

fn row_get<T>(row: &sqlx::postgres::PgRow, column: &str) -> Result<T, DataLayerError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_postgres_err()
}

fn map_auth_api_key_snapshot_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredAuthApiKeySnapshot, DataLayerError> {
    let snapshot = StoredAuthApiKeySnapshot::new(
        row_get(row, "user_id")?,
        row_get(row, "username")?,
        row_get(row, "email")?,
        row_get(row, "user_role")?,
        row_get(row, "user_auth_source")?,
        row_get(row, "user_is_active")?,
        row_get(row, "user_is_deleted")?,
        row_get(row, "user_allowed_providers")?,
        row_get(row, "user_allowed_api_formats")?,
        row_get(row, "user_allowed_models")?,
        row_get(row, "api_key_id")?,
        row_get(row, "api_key_name")?,
        row_get(row, "api_key_is_active")?,
        row_get(row, "api_key_is_locked")?,
        row_get(row, "api_key_is_standalone")?,
        row_get(row, "api_key_rate_limit")?,
        row_get(row, "api_key_concurrent_limit")?,
        row_get(row, "api_key_expires_at_unix_secs")?,
        row_get(row, "api_key_allowed_providers")?,
        row_get(row, "api_key_allowed_api_formats")?,
        row_get(row, "api_key_allowed_models")?,
    )?
    .with_api_key_ip_rules(row_get(row, "api_key_ip_rules")?)?;
    Ok(snapshot.with_user_rate_limit(row_get(row, "user_rate_limit")?))
}

fn map_auth_api_key_export_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredAuthApiKeyExportRecord, DataLayerError> {
    let feature_settings = row_get(row, "feature_settings")?;
    StoredAuthApiKeyExportRecord::new(
        row_get(row, "user_id")?,
        row_get(row, "api_key_id")?,
        row_get(row, "key_hash")?,
        row_get(row, "key_encrypted")?,
        row_get(row, "name")?,
        row_get(row, "allowed_providers")?,
        row_get(row, "allowed_api_formats")?,
        row_get(row, "allowed_models")?,
        row_get(row, "rate_limit")?,
        row_get(row, "concurrent_limit")?,
        row_get(row, "force_capabilities")?,
        row_get(row, "is_active")?,
        row_get(row, "expires_at_unix_secs")?,
        row_get(row, "auto_delete_on_expiry")?,
        row_get::<i64>(row, "total_requests")?,
        row_get::<i64>(row, "total_tokens")?,
        row_get(row, "total_cost_usd")?,
        row_get(row, "is_standalone")?,
    )
    .and_then(|record| record.with_ip_rules(row_get(row, "ip_rules")?))
    .map(|record| record.with_feature_settings(feature_settings))
    .and_then(|record| {
        record.with_activity_timestamps(
            row_get(row, "last_used_at_unix_secs")?,
            row_get(row, "created_at_unix_secs")?,
            row_get(row, "updated_at_unix_secs")?,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        datetime_from_unix_secs, i64_from_u64, DataLayerError,
        SqlxAuthApiKeySnapshotReadRepository, CREATE_STANDALONE_API_KEY_SQL,
        CREATE_USER_API_KEY_SQL, POSTGRES_ANONYMIZE_API_KEY_HISTORY_SQL,
        POSTGRES_DELETE_API_KEY_DEPENDENTS_SQL, UPDATE_STANDALONE_API_KEY_BASIC_SQL,
        UPDATE_USER_API_KEY_BASIC_SQL,
    };
    use crate::{PostgresPoolConfig, PostgresPoolFactory};

    #[test]
    fn checked_u64_conversions_reject_counter_and_timestamp_overflow() {
        assert_eq!(
            i64_from_u64(i64::MAX as u64, "api_keys.total_requests")
                .expect("i64 maximum should fit"),
            i64::MAX
        );
        assert!(matches!(
            i64_from_u64(u64::MAX, "api_keys.total_requests"),
            Err(DataLayerError::InvalidInput(_))
        ));
        assert!(matches!(
            datetime_from_unix_secs(i64::MAX as u64, "api_keys.expires_at"),
            Err(DataLayerError::InvalidInput(_))
        ));
    }

    #[test]
    fn create_api_key_sql_orders_expiry_before_standalone_flags() {
        assert!(CREATE_USER_API_KEY_SQL
            .contains("expires_at,\n  auto_delete_on_expiry,\n  is_locked,\n  is_standalone,"));
        assert!(CREATE_USER_API_KEY_SQL
            .contains("$13,\n  $14,\n  $15,\n  $16,\n  FALSE,\n  FALSE,\n  $17,"));
        assert!(CREATE_STANDALONE_API_KEY_SQL
            .contains("expires_at,\n  auto_delete_on_expiry,\n  is_locked,\n  is_standalone,"));
        assert!(CREATE_STANDALONE_API_KEY_SQL
            .contains("$13,\n  $14,\n  $15,\n  FALSE,\n  TRUE,\n  $16,"));
    }

    #[test]
    fn api_key_delete_sql_preserves_ids_and_removes_private_snapshots() {
        for table in [
            "request_candidates",
            "video_tasks",
            "usage",
            "stats_daily_api_key",
        ] {
            assert!(POSTGRES_ANONYMIZE_API_KEY_HISTORY_SQL.iter().any(|sql| {
                sql.starts_with(&format!("UPDATE {table} "))
                    && sql.contains("SET api_key_name = NULL")
                    && sql.ends_with("WHERE api_key_id = $1")
            }));
        }
        assert!(POSTGRES_ANONYMIZE_API_KEY_HISTORY_SQL
            .iter()
            .any(|sql| sql
                .starts_with("UPDATE audit_logs SET description = 'deleted API key event'")));
        assert!(POSTGRES_ANONYMIZE_API_KEY_HISTORY_SQL.iter().any(|sql| sql
            .starts_with("UPDATE payment_callbacks SET payload = NULL, error_message = NULL")));
        assert_eq!(
            POSTGRES_DELETE_API_KEY_DEPENDENTS_SQL,
            &["DELETE FROM api_key_provider_mappings WHERE api_key_id = $1"]
        );
    }

    #[test]
    fn update_standalone_api_key_basic_sql_casts_json_case_values() {
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL
            .contains("key_encrypted = CASE WHEN $2 THEN $3 ELSE key_encrypted END"));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL
            .contains("concurrent_limit = CASE WHEN $10 THEN $11 ELSE concurrent_limit END"));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL.contains(
            "allowed_providers = CASE WHEN $12 THEN $13::json ELSE allowed_providers END"
        ));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL.contains(
            "allowed_api_formats = CASE WHEN $14 THEN $15::json ELSE allowed_api_formats END"
        ));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL
            .contains("allowed_models = CASE WHEN $16 THEN $17::json ELSE allowed_models END"));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL
            .contains("ip_rules = CASE WHEN $18 THEN $19::jsonb ELSE ip_rules END"));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL
            .contains("rate_limit = CASE WHEN $8 THEN $9 ELSE rate_limit END"));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL
            .contains("expires_at = CASE WHEN $20 THEN $21::timestamptz ELSE expires_at END"));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL.contains(
            "auto_delete_on_expiry = CASE WHEN $22 THEN $23 ELSE auto_delete_on_expiry END"
        ));
        assert!(UPDATE_STANDALONE_API_KEY_BASIC_SQL.contains(
            "force_capabilities = CASE WHEN $6 THEN $7::json ELSE force_capabilities END"
        ));
    }

    #[test]
    fn update_user_api_key_basic_sql_casts_json_patches_and_fences_locked_keys() {
        assert!(UPDATE_USER_API_KEY_BASIC_SQL
            .contains("key_encrypted = CASE WHEN $3 THEN $4 ELSE key_encrypted END"));
        assert!(UPDATE_USER_API_KEY_BASIC_SQL
            .contains("ip_rules = CASE WHEN $11 THEN $12::jsonb ELSE ip_rules END"));
        assert!(UPDATE_USER_API_KEY_BASIC_SQL.contains(
            "feature_settings = CASE WHEN $13 THEN $14::jsonb ELSE feature_settings END"
        ));
        assert!(UPDATE_USER_API_KEY_BASIC_SQL.contains("AND ($15 = FALSE OR is_locked = FALSE)"));
    }

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
        let repository = SqlxAuthApiKeySnapshotReadRepository::new(pool);
        let _ = repository.pool();
    }
}
