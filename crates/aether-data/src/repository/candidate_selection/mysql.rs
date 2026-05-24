use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use super::{
    MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateOrder, StoredPoolKeyCandidateRowsByKeyIdsQuery,
    StoredPoolKeyCandidateRowsQuery, StoredProviderModelMapping,
    StoredRequestedModelCandidateRowsQuery,
};
use crate::driver::mysql::MysqlPool;
use crate::error::SqlResultExt;
use crate::DataLayerError;

const CANDIDATE_SELECTION_COLUMNS: &str = r#"
SELECT
  p.id AS provider_id,
  p.name AS provider_name,
  p.provider_type AS provider_type,
  p.provider_priority AS provider_priority,
  p.is_active AS provider_is_active,
  p.config AS provider_config,
  pe.id AS endpoint_id,
  COALESCE(pe.api_format, '') AS endpoint_api_format,
  pe.api_family AS endpoint_api_family,
  pe.endpoint_kind AS endpoint_kind,
  pe.is_active AS endpoint_is_active,
  pak.id AS key_id,
  pak.name AS key_name,
  pak.auth_type AS key_auth_type,
  pak.auth_config AS key_auth_config,
  pak.is_active AS key_is_active,
  pak.api_formats AS key_api_formats,
  pak.allowed_models AS key_allowed_models,
  pak.capabilities AS key_capabilities,
  pak.internal_priority AS key_internal_priority,
  pak.global_priority_by_format AS key_global_priority_by_format,
  pak.last_used_at AS key_last_used_at_unix_secs,
  m.id AS model_id,
  m.global_model_id AS global_model_id,
  gm.name AS global_model_name,
  gm.config AS global_model_config,
  m.provider_model_name AS model_provider_model_name,
  m.provider_model_mappings AS model_provider_model_mappings,
  m.supports_streaming AS model_supports_streaming,
  m.is_active AS model_is_active,
  m.is_available AS model_is_available
FROM providers p
INNER JOIN provider_endpoints pe ON pe.provider_id = p.id
INNER JOIN provider_api_keys pak ON pak.provider_id = p.id
INNER JOIN models m ON m.provider_id = p.id
INNER JOIN global_models gm ON gm.id = m.global_model_id
WHERE p.is_active = 1
  AND pe.is_active = 1
  AND pak.is_active = 1
  AND m.is_active = 1
  AND m.is_available = 1
  AND gm.is_active = 1
"#;

#[derive(Debug, Clone)]
pub struct MysqlMinimalCandidateSelectionReadRepository {
    pool: MysqlPool,
}

#[derive(Debug, Clone)]
struct CandidateSelectionRow {
    row: StoredMinimalCandidateSelectionRow,
    provider_pool_enabled: bool,
    key_auth_config: Option<String>,
    key_last_used_at_unix_secs: Option<u64>,
}

impl MysqlMinimalCandidateSelectionReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn load_rows_for_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<CandidateSelectionRow>, DataLayerError> {
        let canonical_api_format = normalize_api_format(api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let match_aliases = sql_match_aliases(&storage_aliases);

        let mut builder = QueryBuilder::<MySql>::new(CANDIDATE_SELECTION_COLUMNS);
        builder.push(" AND LOWER(pe.api_format) IN (");
        {
            let mut separated = builder.separated(", ");
            for alias in &match_aliases {
                separated.push_bind(alias);
            }
        }
        builder.push(")");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let mut items = rows
            .iter()
            .map(map_candidate_selection_row)
            .collect::<Result<Vec<_>, _>>()?;
        items.retain(|item| {
            api_format_matches(&item.row.endpoint_api_format, &canonical_api_format)
                && item.row.key_supports_api_format(&canonical_api_format)
                && key_auth_channel_matches(item, &canonical_api_format)
        });
        Ok(items)
    }

    async fn selected_rows_for_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let rows = self.load_rows_for_api_format(api_format).await?;
        Ok(sort_rows(select_pool_rows(rows), true))
    }
}

#[async_trait]
impl MinimalCandidateSelectionReadRepository for MysqlMinimalCandidateSelectionReadRepository {
    async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.selected_rows_for_api_format(api_format).await
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Ok(sort_rows(
            self.selected_rows_for_api_format(api_format)
                .await?
                .into_iter()
                .filter(|row| row.global_model_name == global_model_name)
                .collect(),
            false,
        ))
    }

    async fn list_for_exact_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.list_for_exact_api_format_and_requested_model_page(
            &StoredRequestedModelCandidateRowsQuery {
                api_format: api_format.to_string(),
                requested_model_name: requested_model_name.to_string(),
                offset: 0,
                limit: u32::MAX,
            },
        )
        .await
    }

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let rows = self
            .selected_rows_for_api_format(&query.api_format)
            .await?
            .into_iter()
            .filter(|row| {
                row_matches_requested_model(row, &query.requested_model_name, &query.api_format)
            })
            .collect::<Vec<_>>();
        Ok(sort_rows(rows, true)
            .into_iter()
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .collect())
    }

    async fn list_pool_key_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let rows = self
            .load_rows_for_api_format(&query.api_format)
            .await?
            .into_iter()
            .filter(|row| {
                row.row.provider_id == query.provider_id
                    && row.row.endpoint_id == query.endpoint_id
                    && row.row.model_id == query.model_id
            })
            .collect::<Vec<_>>();
        let mut rows = sort_pool_key_rows(rows, &query.order);
        Ok(rows
            .drain(..)
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .map(|item| item.row)
            .collect())
    }

    async fn list_pool_key_rows_for_group_key_ids(
        &self,
        query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        if query.key_ids.is_empty() {
            return Ok(Vec::new());
        }
        let key_order = query
            .key_ids
            .iter()
            .enumerate()
            .map(|(index, key_id)| (key_id.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let mut rows = self
            .load_rows_for_api_format(&query.api_format)
            .await?
            .into_iter()
            .filter(|row| {
                row.row.provider_id == query.provider_id
                    && row.row.endpoint_id == query.endpoint_id
                    && row.row.model_id == query.model_id
                    && key_order.contains_key(row.row.key_id.as_str())
            })
            .map(|item| item.row)
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            key_order
                .get(left.key_id.as_str())
                .cmp(&key_order.get(right.key_id.as_str()))
                .then(left.key_id.cmp(&right.key_id))
        });
        Ok(dedupe_candidate_selection_rows(rows))
    }
}

fn select_pool_rows(rows: Vec<CandidateSelectionRow>) -> Vec<StoredMinimalCandidateSelectionRow> {
    let mut selected = Vec::new();
    let mut pool_rows =
        BTreeMap::<(String, String, String), StoredMinimalCandidateSelectionRow>::new();
    for item in rows {
        if !item.provider_pool_enabled {
            selected.push(item.row);
            continue;
        }
        let key = (
            item.row.provider_id.clone(),
            item.row.endpoint_id.clone(),
            item.row.model_id.clone(),
        );
        match pool_rows.get(&key) {
            Some(existing)
                if (existing.key_internal_priority, existing.key_id.as_str())
                    <= (item.row.key_internal_priority, item.row.key_id.as_str()) => {}
            _ => {
                pool_rows.insert(key, item.row);
            }
        }
    }
    selected.extend(pool_rows.into_values());
    dedupe_candidate_selection_rows(selected)
}

fn sort_rows(
    mut rows: Vec<StoredMinimalCandidateSelectionRow>,
    include_global_model: bool,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    rows.sort_by(|left, right| {
        if include_global_model {
            let ordering = left.global_model_name.cmp(&right.global_model_name);
            if !ordering.is_eq() {
                return ordering;
            }
        }
        left.provider_priority
            .cmp(&right.provider_priority)
            .then(left.key_internal_priority.cmp(&right.key_internal_priority))
            .then(left.provider_id.cmp(&right.provider_id))
            .then(left.endpoint_id.cmp(&right.endpoint_id))
            .then(left.key_id.cmp(&right.key_id))
            .then(left.model_id.cmp(&right.model_id))
    });
    rows
}

fn sort_pool_key_rows(
    mut rows: Vec<CandidateSelectionRow>,
    order: &StoredPoolKeyCandidateOrder,
) -> Vec<CandidateSelectionRow> {
    rows.sort_by(|left, right| match order {
        StoredPoolKeyCandidateOrder::InternalPriority => compare_pool_key_internal(left, right),
        StoredPoolKeyCandidateOrder::Lru => left
            .key_last_used_at_unix_secs
            .cmp(&right.key_last_used_at_unix_secs)
            .then_with(|| compare_pool_key_internal(left, right)),
        StoredPoolKeyCandidateOrder::CacheAffinity => right
            .key_last_used_at_unix_secs
            .cmp(&left.key_last_used_at_unix_secs)
            .then_with(|| compare_pool_key_internal(left, right)),
        StoredPoolKeyCandidateOrder::SingleAccount => left
            .row
            .key_internal_priority
            .cmp(&right.row.key_internal_priority)
            .then_with(|| {
                right
                    .key_last_used_at_unix_secs
                    .cmp(&left.key_last_used_at_unix_secs)
            })
            .then(left.row.key_id.cmp(&right.row.key_id)),
        StoredPoolKeyCandidateOrder::LoadBalance { seed } => {
            stable_pool_key_hash(seed.as_str(), left.row.key_id.as_str())
                .cmp(&stable_pool_key_hash(
                    seed.as_str(),
                    right.row.key_id.as_str(),
                ))
                .then(left.row.key_id.cmp(&right.row.key_id))
        }
    });
    rows
}

fn compare_pool_key_internal(
    left: &CandidateSelectionRow,
    right: &CandidateSelectionRow,
) -> std::cmp::Ordering {
    left.row
        .key_internal_priority
        .cmp(&right.row.key_internal_priority)
        .then(left.row.key_id.cmp(&right.row.key_id))
}

fn stable_pool_key_hash(seed: &str, key_id: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in seed
        .as_bytes()
        .iter()
        .copied()
        .chain(std::iter::once(b':'))
        .chain(key_id.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn row_matches_requested_model(
    row: &StoredMinimalCandidateSelectionRow,
    requested_model_name: &str,
    api_format: &str,
) -> bool {
    (row_has_available_provider_model(row, api_format)
        && row.global_model_name == requested_model_name)
        || (row_default_provider_model_name_available(row, api_format)
            && row.model_provider_model_name == requested_model_name)
        || row
            .model_provider_model_mappings
            .as_ref()
            .is_some_and(|mappings| {
                mappings.iter().any(|mapping| {
                    mapping_scope_matches(mapping, row, api_format)
                        && mapping.name == requested_model_name
                })
            })
}

fn row_has_available_provider_model(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> bool {
    row_mapping_matches_scope(row, api_format)
        || row_default_provider_model_name_available(row, api_format)
}

fn row_default_provider_model_name_available(
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> bool {
    let Some(mappings) = row.model_provider_model_mappings.as_ref() else {
        return true;
    };
    let mut has_explicit_default_mapping = false;
    for mapping in mappings {
        if mapping.name != row.model_provider_model_name {
            continue;
        }
        has_explicit_default_mapping = true;
        if mapping_scope_matches(mapping, row, api_format) {
            return true;
        }
    }
    !has_explicit_default_mapping
}

fn row_mapping_matches_scope(row: &StoredMinimalCandidateSelectionRow, api_format: &str) -> bool {
    row.model_provider_model_mappings
        .as_ref()
        .is_some_and(|mappings| {
            mappings
                .iter()
                .any(|mapping| mapping_scope_matches(mapping, row, api_format))
        })
}

fn mapping_scope_matches(
    mapping: &super::StoredProviderModelMapping,
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> bool {
    mapping.api_formats.as_ref().is_none_or(|formats| {
        formats
            .iter()
            .any(|value| api_format_matches(value, api_format))
    }) && mapping.endpoint_ids.as_ref().is_none_or(|endpoint_ids| {
        endpoint_ids
            .iter()
            .any(|endpoint_id| endpoint_id == &row.endpoint_id)
    })
}

fn key_auth_channel_matches(row: &CandidateSelectionRow, api_format: &str) -> bool {
    let provider_type = row.row.provider_type.trim().to_ascii_lowercase();
    let auth_type = row.row.key_auth_type.trim().to_ascii_lowercase();
    let api_format = normalize_api_format(api_format);
    match provider_type.as_str() {
        "codex" => {
            auth_type == "oauth"
                && matches!(
                    api_format.as_str(),
                    "openai:responses" | "openai:responses:compact" | "openai:image"
                )
        }
        "chatgpt_web" => {
            matches!(auth_type.as_str(), "oauth" | "bearer") && api_format == "openai:image"
        }
        "claude_code" => auth_type == "oauth" && api_format == "claude:messages",
        "kiro" => {
            api_format == "claude:messages"
                && (auth_type == "oauth"
                    || (auth_type == "bearer"
                        && row
                            .key_auth_config
                            .as_deref()
                            .is_some_and(|value| !value.trim().is_empty())))
        }
        "gemini_cli" | "antigravity" => {
            auth_type == "oauth" && api_format == "gemini:generate_content"
        }
        "grok" => {
            auth_type == "oauth"
                && matches!(
                    api_format.as_str(),
                    "openai:chat" | "openai:responses" | "claude:messages" | "openai:image"
                )
        }
        "windsurf" => {
            matches!(auth_type.as_str(), "oauth" | "api_key" | "bearer")
                && api_format == "openai:chat"
        }
        "vertex_ai" => {
            (auth_type == "api_key"
                && matches!(
                    api_format.as_str(),
                    "gemini:generate_content" | "gemini:embedding"
                ))
                || (matches!(auth_type.as_str(), "service_account" | "vertex_ai")
                    && matches!(
                        api_format.as_str(),
                        "claude:messages" | "gemini:generate_content" | "gemini:embedding"
                    ))
        }
        _ => auth_type != "oauth",
    }
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

fn map_candidate_selection_row(row: &MySqlRow) -> Result<CandidateSelectionRow, DataLayerError> {
    let provider_config = parse_json(row.try_get("provider_config").ok().flatten())?;
    let global_model_config = parse_json(row.try_get("global_model_config").ok().flatten())?;
    let provider_pool_enabled = json_object_field_present(&provider_config, "pool_advanced");
    let global_model_mappings = global_model_config
        .as_ref()
        .and_then(|value| value.get("model_mappings").cloned());
    let global_model_supports_streaming = global_model_config
        .as_ref()
        .and_then(|value| value.get("streaming"))
        .and_then(json_bool);
    Ok(CandidateSelectionRow {
        row: StoredMinimalCandidateSelectionRow {
            provider_id: row.try_get("provider_id").map_sql_err()?,
            provider_name: row.try_get("provider_name").map_sql_err()?,
            provider_type: row.try_get("provider_type").map_sql_err()?,
            provider_priority: row.try_get("provider_priority").map_sql_err()?,
            provider_is_active: row.try_get("provider_is_active").map_sql_err()?,
            endpoint_id: row.try_get("endpoint_id").map_sql_err()?,
            endpoint_api_format: row.try_get("endpoint_api_format").map_sql_err()?,
            endpoint_api_family: row.try_get("endpoint_api_family").map_sql_err()?,
            endpoint_kind: row.try_get("endpoint_kind").map_sql_err()?,
            endpoint_is_active: row.try_get("endpoint_is_active").map_sql_err()?,
            key_id: row.try_get("key_id").map_sql_err()?,
            key_name: row.try_get("key_name").map_sql_err()?,
            key_auth_type: row.try_get("key_auth_type").map_sql_err()?,
            key_is_active: row.try_get("key_is_active").map_sql_err()?,
            key_api_formats: parse_string_list(
                parse_json(row.try_get("key_api_formats").ok().flatten())?,
                "provider_api_keys.api_formats",
            )?,
            key_allowed_models: parse_string_list(
                parse_json(row.try_get("key_allowed_models").ok().flatten())?,
                "provider_api_keys.allowed_models",
            )?,
            key_capabilities: parse_json(row.try_get("key_capabilities").ok().flatten())?,
            key_internal_priority: row.try_get("key_internal_priority").map_sql_err()?,
            key_global_priority_by_format: parse_json(
                row.try_get("key_global_priority_by_format").ok().flatten(),
            )?,
            model_id: row.try_get("model_id").map_sql_err()?,
            global_model_id: row.try_get("global_model_id").map_sql_err()?,
            global_model_name: row.try_get("global_model_name").map_sql_err()?,
            global_model_mappings: parse_string_list(
                global_model_mappings,
                "global_models.config.model_mappings",
            )?,
            global_model_supports_streaming,
            model_provider_model_name: row.try_get("model_provider_model_name").map_sql_err()?,
            model_provider_model_mappings: parse_provider_model_mappings(parse_json(
                row.try_get("model_provider_model_mappings").ok().flatten(),
            )?)?,
            model_supports_streaming: row.try_get("model_supports_streaming").map_sql_err()?,
            model_is_active: row.try_get("model_is_active").map_sql_err()?,
            model_is_available: row.try_get("model_is_available").map_sql_err()?,
        },
        provider_pool_enabled,
        key_auth_config: row.try_get("key_auth_config").map_sql_err()?,
        key_last_used_at_unix_secs: row
            .try_get::<Option<i64>, _>("key_last_used_at_unix_secs")
            .map_sql_err()?
            .and_then(|value| u64::try_from(value).ok()),
    })
}

fn parse_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "candidate selection JSON field is invalid: {err}"
                ))
            })
        })
        .transpose()
}

fn json_object_field_present(value: &Option<serde_json::Value>, field: &str) -> bool {
    value
        .as_ref()
        .and_then(|value| value.get(field))
        .is_some_and(|value| !value.is_null())
}

fn json_bool(value: &serde_json::Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value
            .as_str()
            .and_then(|value| value.trim().parse::<bool>().ok())
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
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Array(array) => parse_provider_model_mappings_array(&array),
        serde_json::Value::Object(object) => parse_provider_model_mapping_object_lenient(&object)
            .map(|mapping| mapping.map(|value| vec![value])),
        serde_json::Value::String(raw) => parse_embedded_provider_model_mappings(&raw),
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
        return parse_provider_model_mappings(Some(decoded));
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
            serde_json::Value::String(raw) if !raw.trim().is_empty() => {
                mappings.push(StoredProviderModelMapping {
                    name: raw.trim().to_string(),
                    priority: 1,
                    api_formats: None,
                    endpoint_ids: None,
                });
            }
            _ => {}
        }
    }

    if mappings.is_empty() {
        Ok(None)
    } else {
        Ok(Some(mappings))
    }
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
            .map(|value| normalize_api_format(&value))
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

fn api_format_aliases(api_format: &str) -> Vec<String> {
    aether_ai_formats::api_format_storage_aliases(api_format)
}

fn normalize_api_format(api_format: &str) -> String {
    aether_ai_formats::normalize_api_format_alias(api_format)
}

fn api_format_matches(left: &str, right: &str) -> bool {
    aether_ai_formats::api_format_alias_matches(left, right)
}

fn sql_match_aliases(api_formats: &[String]) -> Vec<String> {
    api_formats
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::MysqlMinimalCandidateSelectionReadRepository;

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlMinimalCandidateSelectionReadRepository::new(pool);
    }
}
