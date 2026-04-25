use async_trait::async_trait;
use futures_util::{stream::TryStream, TryStreamExt};
use sqlx::{PgPool, Row};

use super::{
    MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
    StoredProviderModelMapping,
};
use crate::{error::SqlxResultExt, DataLayerError};

const LIST_FOR_EXACT_API_FORMAT_SQL: &str = r#"
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
  AND pak.is_active = TRUE
  AND m.is_active = TRUE
  AND m.is_available = TRUE
  AND gm.is_active = TRUE
  AND LOWER(pe.api_format) = LOWER($1)
  AND (
    pak.api_formats IS NULL
    OR (
      LOWER(BTRIM(p.provider_type)) IN (
        'claude_code',
        'codex',
        'gemini_cli',
        'vertex_ai',
        'antigravity'
      )
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'kiro'
      AND (
        LOWER(BTRIM(pak.auth_type)) = 'oauth'
        OR (
          LOWER(BTRIM(pak.auth_type)) = 'bearer'
          AND pak.auth_config IS NOT NULL
          AND BTRIM(pak.auth_config) <> ''
        )
      )
    )
    OR EXISTS (
      SELECT 1
      FROM json_array_elements_text(pak.api_formats) AS fmt(value)
      WHERE LOWER(fmt.value) = LOWER($1)
    )
  )
ORDER BY
  gm.name ASC,
  p.provider_priority ASC,
  pak.internal_priority ASC,
  p.id ASC,
  pe.id ASC,
  pak.id ASC,
  m.id ASC
"#;

const LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL: &str = r#"
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
  AND pak.is_active = TRUE
  AND m.is_active = TRUE
  AND m.is_available = TRUE
  AND gm.is_active = TRUE
  AND LOWER(pe.api_format) = LOWER($1)
  AND gm.name = $2
  AND (
    pak.api_formats IS NULL
    OR (
      LOWER(BTRIM(p.provider_type)) IN (
        'claude_code',
        'codex',
        'gemini_cli',
        'vertex_ai',
        'antigravity'
      )
      AND LOWER(BTRIM(pak.auth_type)) = 'oauth'
    )
    OR (
      LOWER(BTRIM(p.provider_type)) = 'kiro'
      AND (
        LOWER(BTRIM(pak.auth_type)) = 'oauth'
        OR (
          LOWER(BTRIM(pak.auth_type)) = 'bearer'
          AND pak.auth_config IS NOT NULL
          AND BTRIM(pak.auth_config) <> ''
        )
      )
    )
    OR EXISTS (
      SELECT 1
      FROM json_array_elements_text(pak.api_formats) AS fmt(value)
      WHERE LOWER(fmt.value) = LOWER($1)
    )
  )
ORDER BY
  p.provider_priority ASC,
  pak.internal_priority ASC,
  p.id ASC,
  pe.id ASC,
  pak.id ASC,
  m.id ASC
"#;

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
        Self::collect_query_rows(
            sqlx::query(LIST_FOR_EXACT_API_FORMAT_SQL)
                .bind(api_format)
                .fetch(&self.pool),
            map_candidate_selection_row,
        )
        .await
    }

    pub async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Self::collect_query_rows(
            sqlx::query(LIST_FOR_EXACT_API_FORMAT_AND_GLOBAL_MODEL_SQL)
                .bind(api_format)
                .bind(global_model_name)
                .fetch(&self.pool),
            map_candidate_selection_row,
        )
        .await
    }
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
    )?;

    Ok(Some(StoredProviderModelMapping {
        name: name.to_string(),
        priority: i32::try_from(priority).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "invalid models.provider_model_mappings.priority: {priority}"
            ))
        })?,
        api_formats,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        parse_provider_model_mappings, parse_string_list,
        SqlxMinimalCandidateSelectionReadRepository,
    };
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};
    use crate::repository::candidate_selection::StoredProviderModelMapping;

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
                },
                StoredProviderModelMapping {
                    name: "gpt-5.2-mini".to_string(),
                    priority: 1,
                    api_formats: None,
                }
            ])
        );
    }
}
