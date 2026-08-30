use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use aether_data_contracts::repository::candidate_selection::{
    provider_model_mapping_api_format_covers, MinimalCandidateSelectionReadRepository,
    StoredApiFormatCandidateRowsQuery, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateOrder, StoredPoolKeyCandidateRowsByKeyIdsQuery,
    StoredPoolKeyCandidateRowsQuery, StoredProviderModelMapping,
    StoredRequestedModelCandidateRowsQuery,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::MysqlPool;

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

const REQUESTED_MODEL_RAW_PAGE_SIZE: u32 = 256;
const REQUESTED_MODEL_RAW_SCAN_LIMIT: u32 = 2048;

#[derive(Debug, Clone)]
pub struct MysqlMinimalCandidateSelectionReadRepository {
    pool: MysqlPool,
}

#[derive(Debug, Clone)]
struct CandidateSelectionRow {
    row: StoredMinimalCandidateSelectionRow,
    provider_pool_enabled: bool,
    key_auth_config: Option<String>,
}

#[derive(Debug)]
struct ExactPageAccumulator<T> {
    rows: Vec<T>,
    offset: usize,
    limit: usize,
    target_len: usize,
}

impl<T> ExactPageAccumulator<T> {
    fn new(offset: u32, limit: u32) -> Self {
        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        Self {
            rows: Vec::new(),
            offset,
            limit,
            target_len: offset.saturating_add(limit),
        }
    }

    fn is_full(&self) -> bool {
        self.rows.len() >= self.target_len
    }

    fn push_matching<I, F>(&mut self, rows: I, mut predicate: F)
    where
        I: IntoIterator<Item = T>,
        F: FnMut(&T) -> bool,
    {
        let remaining = self.target_len.saturating_sub(self.rows.len());
        self.rows.extend(
            rows.into_iter()
                .filter(|row| predicate(row))
                .take(remaining),
        );
    }

    fn into_page(self) -> Vec<T> {
        self.rows
            .into_iter()
            .skip(self.offset)
            .take(self.limit)
            .collect()
    }
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

    async fn load_selected_rows_for_api_format_page(
        &self,
        api_format: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CandidateSelectionRow>, DataLayerError> {
        let mut builder = api_format_page_query(api_format, limit, offset);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_candidate_selection_row).collect()
    }

    async fn selected_rows_for_api_format_page(
        &self,
        query: &StoredApiFormatCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        let target_len = query.offset.saturating_add(query.limit);
        let mut raw_offset = 0_u32;
        let mut selected = Vec::new();
        while selected.len() < target_len as usize && raw_offset < REQUESTED_MODEL_RAW_SCAN_LIMIT {
            let raw_limit =
                target_len.min(REQUESTED_MODEL_RAW_SCAN_LIMIT.saturating_sub(raw_offset));
            let rows = self
                .load_selected_rows_for_api_format_page(&query.api_format, raw_limit, raw_offset)
                .await?;
            let raw_len = rows.len() as u32;
            selected.extend(rows.into_iter().filter(|item| {
                api_format_matches(&item.row.endpoint_api_format, &query.api_format)
                    && item.row.key_supports_api_format(&query.api_format)
                    && key_auth_channel_matches(item, &query.api_format)
            }));
            if raw_len < raw_limit || raw_len == 0 {
                break;
            }
            let next_offset = raw_offset.saturating_add(raw_len);
            if next_offset == raw_offset {
                break;
            }
            raw_offset = next_offset;
        }
        let rows = selected.into_iter().map(|item| item.row).collect();
        Ok(sort_rows(dedupe_candidate_selection_rows(rows), true)
            .into_iter()
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .collect())
    }
}

fn api_format_page_query(
    api_format: &str,
    limit: u32,
    offset: u32,
) -> QueryBuilder<'static, MySql> {
    let mut builder = QueryBuilder::<MySql>::new("WITH candidate_rows AS (");
    builder.push(CANDIDATE_SELECTION_COLUMNS);
    push_candidate_api_format_filters(&mut builder, api_format);
    push_selected_pool_rows(&mut builder);
    builder.push(
        r#"
ORDER BY
  global_model_name ASC,
  provider_priority ASC,
  key_internal_priority ASC,
  provider_id ASC,
  endpoint_id ASC,
  key_id ASC,
  model_id ASC
LIMIT "#,
    );
    builder.push_bind(i64::from(limit));
    builder.push(" OFFSET ");
    builder.push_bind(i64::from(offset));
    builder
}

fn requested_model_page_query(
    api_format: &str,
    requested_model_name: &str,
    limit: u32,
    offset: u32,
) -> QueryBuilder<'static, MySql> {
    let mut builder = QueryBuilder::<MySql>::new("WITH candidate_rows AS (");
    builder.push(CANDIDATE_SELECTION_COLUMNS);
    push_candidate_api_format_filters(&mut builder, api_format);
    push_requested_model_sql_filter(&mut builder, requested_model_name);
    push_selected_pool_rows(&mut builder);
    builder.push(
        r#"
ORDER BY
  global_model_name ASC,
  provider_priority ASC,
  key_internal_priority ASC,
  provider_id ASC,
  endpoint_id ASC,
  key_id ASC,
  model_id ASC
LIMIT "#,
    );
    builder.push_bind(i64::from(limit));
    builder.push(" OFFSET ");
    builder.push_bind(i64::from(offset));
    builder
}

fn pool_key_group_query(query: &StoredPoolKeyCandidateRowsQuery) -> QueryBuilder<'static, MySql> {
    let mut builder = QueryBuilder::<MySql>::new(CANDIDATE_SELECTION_COLUMNS);
    push_candidate_api_format_filters(&mut builder, &query.api_format);
    push_pool_key_group_filters(
        &mut builder,
        &query.provider_id,
        &query.endpoint_id,
        &query.model_id,
    );
    push_pool_key_order(&mut builder, &query.order);
    builder.push(" LIMIT ");
    builder.push_bind(i64::from(query.limit));
    builder.push(" OFFSET ");
    builder.push_bind(i64::from(query.offset));
    builder
}

fn pool_key_group_by_key_ids_query(
    query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
) -> QueryBuilder<'static, MySql> {
    let mut builder = QueryBuilder::<MySql>::new(CANDIDATE_SELECTION_COLUMNS);
    push_candidate_api_format_filters(&mut builder, &query.api_format);
    push_pool_key_group_filters(
        &mut builder,
        &query.provider_id,
        &query.endpoint_id,
        &query.model_id,
    );
    builder.push(" AND pak.id IN (");
    {
        let mut separated = builder.separated(", ");
        for key_id in &query.key_ids {
            separated.push_bind(key_id.clone());
        }
    }
    builder.push(") ORDER BY FIELD(pak.id, ");
    {
        let mut separated = builder.separated(", ");
        for key_id in &query.key_ids {
            separated.push_bind(key_id.clone());
        }
    }
    builder.push(") ASC, pak.id ASC");
    builder
}

fn push_candidate_api_format_filters(builder: &mut QueryBuilder<'_, MySql>, api_format: &str) {
    let canonical_api_format = normalize_api_format(api_format);
    let storage_aliases = sql_match_aliases(&api_format_aliases(&canonical_api_format));
    let permission_aliases =
        sql_match_aliases(&api_format_permission_aliases(&canonical_api_format));
    builder.push(" AND LOWER(pe.api_format) IN (");
    {
        let mut separated = builder.separated(", ");
        for alias in storage_aliases {
            separated.push_bind(alias);
        }
    }
    builder.push(") AND (pak.api_formats IS NULL OR TRIM(pak.api_formats) = ''");
    for alias in permission_aliases {
        builder.push(" OR JSON_SEARCH(LOWER(pak.api_formats), 'one', ");
        builder.push_bind(alias);
        builder.push(") IS NOT NULL");
    }
    builder.push(")");
    push_key_auth_channel_filter(builder, &canonical_api_format);
}

fn push_requested_model_sql_filter(
    builder: &mut QueryBuilder<'_, MySql>,
    requested_model_name: &str,
) {
    builder.push(" AND (gm.name = ");
    builder.push_bind(requested_model_name.to_string());
    builder.push(" OR m.provider_model_name = ");
    builder.push_bind(requested_model_name.to_string());
    builder.push(" OR (m.provider_model_mappings IS NOT NULL AND LOCATE(");
    builder.push_bind(requested_model_name.to_string());
    builder.push(", m.provider_model_mappings) > 0))");
}

fn push_selected_pool_rows(builder: &mut QueryBuilder<'_, MySql>) {
    builder.push(
        r#"
),
ranked_rows AS (
  SELECT
    candidate_rows.*,
    CASE
      WHEN JSON_EXTRACT(provider_config, '$.pool_advanced') IS NOT NULL
        AND JSON_TYPE(JSON_EXTRACT(provider_config, '$.pool_advanced')) <> 'NULL'
      THEN 1 ELSE 0
    END AS provider_pool_enabled,
    ROW_NUMBER() OVER (
      PARTITION BY provider_id, endpoint_id, model_id
      ORDER BY key_internal_priority ASC, key_id ASC
    ) AS pool_rank
  FROM candidate_rows
),
selected_rows AS (
  SELECT *
  FROM ranked_rows
  WHERE provider_pool_enabled = 0 OR pool_rank = 1
)
SELECT *
FROM selected_rows"#,
    );
}

fn push_pool_key_group_filters(
    builder: &mut QueryBuilder<'_, MySql>,
    provider_id: &str,
    endpoint_id: &str,
    model_id: &str,
) {
    builder.push(" AND p.id = ");
    builder.push_bind(provider_id.to_string());
    builder.push(" AND pe.id = ");
    builder.push_bind(endpoint_id.to_string());
    builder.push(" AND m.id = ");
    builder.push_bind(model_id.to_string());
}

fn push_pool_key_order(builder: &mut QueryBuilder<'_, MySql>, order: &StoredPoolKeyCandidateOrder) {
    match order {
        StoredPoolKeyCandidateOrder::InternalPriority => {
            builder.push(" ORDER BY pak.internal_priority ASC, pak.id ASC");
        }
        StoredPoolKeyCandidateOrder::Lru => {
            builder.push(
                " ORDER BY pak.last_used_at IS NOT NULL ASC, pak.last_used_at ASC, pak.internal_priority ASC, pak.id ASC",
            );
        }
        StoredPoolKeyCandidateOrder::CacheAffinity => {
            builder.push(
                " ORDER BY pak.last_used_at IS NULL ASC, pak.last_used_at DESC, pak.internal_priority ASC, pak.id ASC",
            );
        }
        StoredPoolKeyCandidateOrder::SingleAccount => {
            builder.push(
                " ORDER BY pak.internal_priority ASC, pak.last_used_at IS NULL ASC, pak.last_used_at DESC, pak.id ASC",
            );
        }
        StoredPoolKeyCandidateOrder::LoadBalance { seed } => {
            builder.push(" ORDER BY MD5(CONCAT(");
            builder.push_bind(seed.clone());
            builder.push(", ':', pak.id)) ASC, pak.id ASC");
        }
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

    async fn list_for_exact_api_format_page(
        &self,
        query: &StoredApiFormatCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.selected_rows_for_api_format_page(query).await
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
        let rows = self
            .selected_rows_for_api_format(api_format)
            .await?
            .into_iter()
            .filter(|row| row_matches_requested_model(row, requested_model_name, api_format))
            .collect::<Vec<_>>();
        Ok(sort_rows(rows, true))
    }

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        // The SQL model predicate is a coarse superset, so fill the exact page across raw windows.
        let mut exact_page = ExactPageAccumulator::new(query.offset, query.limit);
        let mut raw_offset = 0_u32;
        while !exact_page.is_full() && raw_offset < REQUESTED_MODEL_RAW_SCAN_LIMIT {
            let raw_limit = REQUESTED_MODEL_RAW_PAGE_SIZE
                .min(REQUESTED_MODEL_RAW_SCAN_LIMIT.saturating_sub(raw_offset));
            let mut builder = requested_model_page_query(
                &query.api_format,
                &query.requested_model_name,
                raw_limit,
                raw_offset,
            );
            let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
            let raw_len = u32::try_from(rows.len()).unwrap_or(u32::MAX);
            let items = rows
                .iter()
                .map(map_candidate_selection_row)
                .collect::<Result<Vec<_>, _>>()?;
            exact_page.push_matching(items, |item| {
                row_matches_requested_model(
                    &item.row,
                    &query.requested_model_name,
                    &query.api_format,
                )
            });
            raw_offset = raw_offset.saturating_add(raw_len);
            if raw_len < raw_limit || raw_len == 0 {
                break;
            }
        }
        let rows = exact_page
            .into_page()
            .into_iter()
            .map(|item| item.row)
            .collect();
        Ok(sort_rows(dedupe_candidate_selection_rows(rows), true))
    }

    async fn list_pool_key_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        let mut builder = pool_key_group_query(query);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let rows = rows
            .iter()
            .map(map_candidate_selection_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(dedupe_candidate_selection_rows(
            rows.into_iter().map(|item| item.row).collect(),
        ))
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
        let mut builder = pool_key_group_by_key_ids_query(query);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let mut rows = rows
            .iter()
            .map(map_candidate_selection_row)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|item| item.row)
            .collect::<Vec<_>>();
        rows = dedupe_candidate_selection_rows(rows);
        rows.sort_by(|left, right| {
            key_order
                .get(left.key_id.as_str())
                .cmp(&key_order.get(right.key_id.as_str()))
                .then(left.key_id.cmp(&right.key_id))
        });
        Ok(rows)
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
    mapping: &StoredProviderModelMapping,
    row: &StoredMinimalCandidateSelectionRow,
    api_format: &str,
) -> bool {
    mapping.api_formats.as_ref().is_none_or(|formats| {
        formats.iter().any(|value| {
            provider_model_mapping_api_format_covers(&row.provider_type, value, api_format)
        })
    }) && mapping.endpoint_ids.as_ref().is_none_or(|endpoint_ids| {
        endpoint_ids
            .iter()
            .any(|endpoint_id| endpoint_id == &row.endpoint_id)
    })
}

fn push_key_auth_channel_filter(builder: &mut QueryBuilder<'_, MySql>, api_format: &str) {
    builder.push(" AND ((LOWER(TRIM(p.provider_type)) = 'codex'");
    builder.push(" AND LOWER(TRIM(pak.auth_type)) = 'oauth' AND ");
    builder.push_bind(api_format.to_string());
    builder.push(
        " IN ('openai:responses', 'openai:responses:compact', 'openai:search', 'openai:image', 'codex:live'))",
    );

    builder.push(" OR (LOWER(TRIM(p.provider_type)) = 'chatgpt_web'");
    builder.push(" AND LOWER(TRIM(pak.auth_type)) IN ('oauth', 'bearer') AND ");
    builder.push_bind(api_format.to_string());
    builder.push(" = 'openai:image')");

    builder.push(" OR (LOWER(TRIM(p.provider_type)) = 'claude_code'");
    builder.push(" AND LOWER(TRIM(pak.auth_type)) = 'oauth' AND ");
    builder.push_bind(api_format.to_string());
    builder.push(" = 'claude:messages')");

    builder.push(" OR (LOWER(TRIM(p.provider_type)) = 'kiro' AND ");
    builder.push_bind(api_format.to_string());
    builder.push(" = 'claude:messages' AND (LOWER(TRIM(pak.auth_type)) = 'oauth'");
    builder.push(" OR (LOWER(TRIM(pak.auth_type)) = 'bearer'");
    builder.push(" AND pak.auth_config IS NOT NULL AND TRIM(pak.auth_config) <> '')))");

    builder.push(" OR (LOWER(TRIM(p.provider_type)) IN ('gemini_cli', 'antigravity')");
    builder.push(" AND LOWER(TRIM(pak.auth_type)) = 'oauth' AND ");
    builder.push_bind(api_format.to_string());
    builder.push(" = 'gemini:generate_content')");

    builder.push(" OR (LOWER(TRIM(p.provider_type)) = 'grok'");
    builder.push(" AND LOWER(TRIM(pak.auth_type)) = 'oauth' AND ");
    builder.push_bind(api_format.to_string());
    builder.push(" IN ('openai:chat', 'openai:responses', 'claude:messages', 'openai:image'))");

    builder.push(" OR (LOWER(TRIM(p.provider_type)) = 'windsurf'");
    builder.push(" AND LOWER(TRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer') AND ");
    builder.push_bind(api_format.to_string());
    builder.push(" = 'openai:chat')");

    builder.push(" OR (LOWER(TRIM(p.provider_type)) = 'vertex_ai'");
    builder.push(
        " AND LOWER(TRIM(pak.auth_type)) IN ('api_key', 'service_account', 'vertex_ai') AND ",
    );
    builder.push_bind(api_format.to_string());
    builder.push(" IN ('gemini:generate_content', 'gemini:embedding'))");

    builder.push(
        " OR (LOWER(TRIM(p.provider_type)) NOT IN ('chatgpt_web', 'claude_code', 'codex', 'gemini_cli', 'grok', 'vertex_ai', 'antigravity', 'kiro', 'windsurf') AND LOWER(TRIM(pak.auth_type)) <> 'oauth'))",
    );
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
                    "openai:responses"
                        | "openai:responses:compact"
                        | "openai:search"
                        | "openai:image"
                        | "codex:live"
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
        "vertex_ai" => vertex_key_auth_channel_matches(&auth_type, &api_format),
        _ => auth_type != "oauth",
    }
}

fn vertex_key_auth_channel_matches(auth_type: &str, api_format: &str) -> bool {
    matches!(auth_type, "api_key" | "service_account" | "vertex_ai")
        && matches!(api_format, "gemini:generate_content" | "gemini:embedding")
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
            provider_pool_enabled,
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
        operations: None,
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
                    operations: None,
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
    let operations = parse_string_list(
        object.get("operations").cloned(),
        "models.provider_model_mappings.operations",
    )?
    .and_then(normalize_request_operations);

    Ok(Some(StoredProviderModelMapping {
        name: name.to_string(),
        priority: i32::try_from(priority).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "invalid models.provider_model_mappings.priority: {priority}"
            ))
        })?,
        api_formats,
        endpoint_ids,
        operations,
    }))
}

fn normalize_request_operations(values: Vec<String>) -> Option<Vec<String>> {
    let operations = values
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!operations.is_empty()).then_some(operations)
}

fn api_format_aliases(api_format: &str) -> Vec<String> {
    aether_ai_formats::api_format_storage_aliases(api_format)
}

fn api_format_permission_aliases(api_format: &str) -> Vec<String> {
    aether_ai_formats::api_format_permission_storage_aliases(api_format)
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
    use super::{
        api_format_page_query, pool_key_group_by_key_ids_query, pool_key_group_query,
        provider_model_mapping_api_format_covers, push_key_auth_channel_filter,
        requested_model_page_query, vertex_key_auth_channel_matches, ExactPageAccumulator,
        MysqlMinimalCandidateSelectionReadRepository, REQUESTED_MODEL_RAW_SCAN_LIMIT,
    };
    use aether_data_contracts::repository::candidate_selection::{
        StoredPoolKeyCandidateOrder, StoredPoolKeyCandidateRowsByKeyIdsQuery,
        StoredPoolKeyCandidateRowsQuery,
    };

    #[test]
    fn api_format_page_query_uses_portable_sql_pagination_and_stable_order() {
        let query = api_format_page_query("openai:chat", 256, 512);
        let sql = query.sql();

        assert!(sql.contains("ROW_NUMBER() OVER ("));
        assert!(sql.contains("JSON_SEARCH(LOWER(pak.api_formats), 'one', ?"));
        assert!(sql.contains("WHERE provider_pool_enabled = 0 OR pool_rank = 1"));
        assert!(sql.contains(
            "ORDER BY\n  global_model_name ASC,\n  provider_priority ASC,\n  key_internal_priority ASC,"
        ));
        assert!(sql.contains("LIMIT ? OFFSET ?"));
    }

    #[test]
    fn requested_model_page_query_filters_and_pages_before_fetch() {
        let query = requested_model_page_query("openai:chat", "gpt-5", 256, 256);
        let sql = query.sql();

        assert!(sql.contains("LOWER(pe.api_format) IN ("));
        assert!(sql.contains("JSON_SEARCH(LOWER(pak.api_formats), 'one', ?"));
        assert!(sql.contains("LOWER(TRIM(p.provider_type)) = 'codex'"));
        assert!(sql.contains("AND (gm.name = ? OR m.provider_model_name = ?"));
        assert!(sql.contains("LOCATE(?, m.provider_model_mappings) > 0"));
        assert!(sql.contains("ROW_NUMBER() OVER ("));
        assert!(sql.contains("LIMIT ? OFFSET ?"));
    }

    #[test]
    fn codex_auth_sql_allows_live_for_oauth_keys() {
        let mut builder = sqlx::QueryBuilder::<sqlx::MySql>::new("SELECT 1 WHERE 1 = 1");
        push_key_auth_channel_filter(&mut builder, "codex:live");
        let sql = builder.sql();
        let codex_clause = sql
            .split_once("LOWER(TRIM(p.provider_type)) = 'codex'")
            .and_then(|(_, suffix)| {
                suffix.split_once("LOWER(TRIM(p.provider_type)) = 'chatgpt_web'")
            })
            .map(|(clause, _)| clause)
            .expect("Codex auth clause should exist");

        assert!(codex_clause.contains("LOWER(TRIM(pak.auth_type)) = 'oauth'"));
        assert!(codex_clause.contains("'codex:live'"));
    }

    #[test]
    fn mysql_mapping_scope_keeps_legacy_responses_compatibility_codex_only() {
        assert!(provider_model_mapping_api_format_covers(
            "codex",
            "openai:responses",
            "codex:live"
        ));
        assert!(!provider_model_mapping_api_format_covers(
            "openai",
            "openai:responses",
            "codex:live"
        ));
        assert!(!provider_model_mapping_api_format_covers(
            "custom",
            "openai:responses",
            "codex:live"
        ));
        assert!(!provider_model_mapping_api_format_covers(
            "codex",
            "openai:chat",
            "codex:live"
        ));
    }

    #[test]
    fn exact_page_accumulator_continues_after_coarse_false_positives() {
        let mut accumulator = ExactPageAccumulator::new(1, 2);
        accumulator.push_matching(vec![("coarse-1", false), ("coarse-2", false)], |row| row.1);
        assert!(!accumulator.is_full());

        accumulator.push_matching(
            vec![
                ("exact-1", true),
                ("coarse-3", false),
                ("exact-2", true),
                ("exact-3", true),
            ],
            |row| row.1,
        );

        assert!(accumulator.is_full());
        assert_eq!(
            accumulator.into_page(),
            vec![("exact-2", true), ("exact-3", true)]
        );
        assert_eq!(REQUESTED_MODEL_RAW_SCAN_LIMIT, 2048);
    }

    #[test]
    fn pool_group_query_pushes_group_filters_order_and_page_into_sql() {
        let query = pool_key_group_query(&StoredPoolKeyCandidateRowsQuery {
            api_format: "openai:chat".to_string(),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            model_id: "model-1".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            order: StoredPoolKeyCandidateOrder::Lru,
            offset: 64,
            limit: 64,
        });
        let sql = query.sql();

        assert!(sql.contains("LOWER(pe.api_format) IN ("));
        assert!(sql.contains("JSON_SEARCH(LOWER(pak.api_formats), 'one', ?"));
        assert!(sql.contains("LOWER(TRIM(p.provider_type)) = 'codex'"));
        assert!(sql.contains("AND p.id = ? AND pe.id = ? AND m.id = ?"));
        assert!(sql.contains(
            "ORDER BY pak.last_used_at IS NOT NULL ASC, pak.last_used_at ASC, pak.internal_priority ASC, pak.id ASC"
        ));
        assert!(sql.contains("LIMIT ? OFFSET ?"));
    }

    #[test]
    fn load_balance_pool_group_query_uses_seeded_sql_order_before_paging() {
        let query = pool_key_group_query(&StoredPoolKeyCandidateRowsQuery {
            api_format: "openai:chat".to_string(),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            model_id: "model-1".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            order: StoredPoolKeyCandidateOrder::LoadBalance {
                seed: "request-1".to_string(),
            },
            offset: 128,
            limit: 64,
        });
        let sql = query.sql();

        assert!(sql.contains("AND p.id = ? AND pe.id = ? AND m.id = ?"));
        assert!(
            sql.contains("ORDER BY MD5(CONCAT(?, ':', pak.id)) ASC, pak.id ASC LIMIT ? OFFSET ?")
        );
        assert!(!sql.contains("ROW_NUMBER() OVER ("));
    }

    #[test]
    fn pool_group_by_key_ids_query_filters_ids_and_preserves_requested_order() {
        let query = pool_key_group_by_key_ids_query(&StoredPoolKeyCandidateRowsByKeyIdsQuery {
            api_format: "openai:chat".to_string(),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            model_id: "model-1".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            key_ids: vec!["key-2".to_string(), "key-1".to_string()],
        });
        let sql = query.sql();

        assert!(sql.contains("LOWER(pe.api_format) IN ("));
        assert!(sql.contains("JSON_SEARCH(LOWER(pak.api_formats), 'one', ?"));
        assert!(sql.contains("LOWER(TRIM(p.provider_type)) = 'codex'"));
        assert!(sql.contains("AND p.id = ? AND pe.id = ? AND m.id = ?"));
        assert!(sql.contains("AND pak.id IN (?, ?)"));
        assert!(sql.contains("ORDER BY FIELD(pak.id, ?, ?) ASC, pak.id ASC"));
    }

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlMinimalCandidateSelectionReadRepository::new(pool);
    }

    #[test]
    fn vertex_auth_matrix_rejects_retired_claude_format() {
        for auth_type in ["api_key", "service_account", "vertex_ai"] {
            assert!(!vertex_key_auth_channel_matches(
                auth_type,
                "claude:messages"
            ));
            assert!(vertex_key_auth_channel_matches(
                auth_type,
                "gemini:generate_content"
            ));
            assert!(vertex_key_auth_channel_matches(
                auth_type,
                "gemini:embedding"
            ));
        }
    }
}
