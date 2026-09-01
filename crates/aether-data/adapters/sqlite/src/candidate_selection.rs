use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use aether_data_contracts::repository::candidate_selection::{
    provider_model_mapping_api_format_covers, MinimalCandidateSelectionReadRepository,
    StoredApiFormatCandidateRowsQuery, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateOrder, StoredPoolKeyCandidateRowsByKeyIdsQuery,
    StoredPoolKeyCandidateRowsQuery, StoredProviderModelMapping,
    StoredRequestedModelCandidateRowsQuery,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::SqlitePool;

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
  m.is_available AS model_is_available,
  CASE
    WHEN json_valid(p.config) THEN
      CASE
        WHEN json_type(p.config, '$.pool_advanced') IS NOT NULL THEN 1
        ELSE 0
      END
    ELSE 0
  END AS provider_pool_enabled
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
pub struct SqliteMinimalCandidateSelectionReadRepository {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
struct CandidateSelectionRow {
    row: StoredMinimalCandidateSelectionRow,
    key_auth_config: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum SelectedRowsOrder {
    WithGlobalModel,
    WithoutGlobalModel,
}

#[derive(Debug, Clone, Copy)]
enum SelectedRowsFilter<'a> {
    None,
    GlobalModel(&'a str),
    RequestedModel(&'a str),
}

#[derive(Debug, Clone, Copy)]
struct SqlPage {
    limit: i64,
    offset: i64,
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

#[derive(Debug)]
struct RequestedModelRawPage {
    rows: Vec<CandidateSelectionRow>,
    raw_len: u32,
}

impl SqliteMinimalCandidateSelectionReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn selected_rows_for_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.load_selected_rows_for_api_format(
            api_format,
            SelectedRowsFilter::None,
            SelectedRowsOrder::WithGlobalModel,
            None,
        )
        .await
    }

    async fn load_selected_rows_for_api_format(
        &self,
        api_format: &str,
        filter: SelectedRowsFilter<'_>,
        order: SelectedRowsOrder,
        page: Option<SqlPage>,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let canonical_api_format = normalize_api_format(api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let match_aliases =
            sql_match_aliases(&api_format_permission_aliases(&canonical_api_format));
        let mut rows = Vec::new();

        for storage_api_format in storage_aliases {
            let mut builder = QueryBuilder::<Sqlite>::new("WITH candidate_rows AS (");
            builder.push(CANDIDATE_SELECTION_COLUMNS);
            push_candidate_sql_filters(&mut builder, &storage_api_format, &match_aliases);
            match filter {
                SelectedRowsFilter::None => {}
                SelectedRowsFilter::GlobalModel(global_model_name) => {
                    builder.push(" AND gm.name = ");
                    builder.push_bind(global_model_name);
                }
                SelectedRowsFilter::RequestedModel(requested_model_name) => {
                    push_requested_model_sql_filter(
                        &mut builder,
                        requested_model_name,
                        &match_aliases,
                    );
                }
            }
            push_selected_rows_query_tail(&mut builder, order, page);

            let query_rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
            let mut items = query_rows
                .iter()
                .map(map_candidate_selection_row)
                .collect::<Result<Vec<_>, _>>()?;
            items.retain(|item| {
                api_format_matches(&item.row.endpoint_api_format, &canonical_api_format)
                    && item.row.key_supports_api_format(&canonical_api_format)
                    && key_auth_channel_matches(item, &canonical_api_format)
            });
            rows.extend(items.into_iter().map(|item| item.row));
        }

        let rows = match filter {
            SelectedRowsFilter::RequestedModel(requested_model_name) => rows
                .into_iter()
                .filter(|row| {
                    row_matches_requested_model(row, requested_model_name, &canonical_api_format)
                })
                .collect(),
            _ => rows,
        };
        Ok(dedupe_candidate_selection_rows(rows))
    }

    async fn load_requested_model_raw_page(
        &self,
        api_format: &str,
        requested_model_name: &str,
        page: SqlPage,
    ) -> Result<RequestedModelRawPage, DataLayerError> {
        let canonical_api_format = normalize_api_format(api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let match_aliases =
            sql_match_aliases(&api_format_permission_aliases(&canonical_api_format));
        let mut builder = QueryBuilder::<Sqlite>::new("WITH candidate_rows AS (");
        builder.push(CANDIDATE_SELECTION_COLUMNS);
        push_candidate_sql_filters_for_aliases(
            &mut builder,
            &storage_aliases,
            &match_aliases,
            &canonical_api_format,
        );
        push_requested_model_sql_filter(&mut builder, requested_model_name, &match_aliases);
        push_selected_rows_query_tail(&mut builder, SelectedRowsOrder::WithGlobalModel, Some(page));

        let query_rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let raw_len = u32::try_from(query_rows.len()).unwrap_or(u32::MAX);
        let mut rows = query_rows
            .iter()
            .map(map_candidate_selection_row)
            .collect::<Result<Vec<_>, _>>()?;
        rows.retain(|item| {
            api_format_matches(&item.row.endpoint_api_format, &canonical_api_format)
                && item.row.key_supports_api_format(&canonical_api_format)
                && key_auth_channel_matches(item, &canonical_api_format)
        });

        Ok(RequestedModelRawPage { rows, raw_len })
    }
}

#[async_trait]
impl MinimalCandidateSelectionReadRepository for SqliteMinimalCandidateSelectionReadRepository {
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
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        let fetch_limit = query.offset.saturating_add(query.limit);
        let mut rows = self
            .load_selected_rows_for_api_format(
                &query.api_format,
                SelectedRowsFilter::None,
                SelectedRowsOrder::WithGlobalModel,
                Some(SqlPage {
                    limit: i64::from(fetch_limit),
                    offset: 0,
                }),
            )
            .await?;
        sort_candidate_selection_rows(&mut rows, true);
        Ok(rows
            .into_iter()
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .collect())
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.load_selected_rows_for_api_format(
            api_format,
            SelectedRowsFilter::GlobalModel(global_model_name),
            SelectedRowsOrder::WithoutGlobalModel,
            None,
        )
        .await
    }

    async fn list_for_exact_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.load_selected_rows_for_api_format(
            api_format,
            SelectedRowsFilter::RequestedModel(requested_model_name),
            SelectedRowsOrder::WithGlobalModel,
            None,
        )
        .await
    }

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        if query.limit == 0 {
            return Ok(Vec::new());
        }

        let mut exact_page = ExactPageAccumulator::new(query.offset, query.limit);
        let mut raw_offset = 0_u32;
        while !exact_page.is_full() && raw_offset < REQUESTED_MODEL_RAW_SCAN_LIMIT {
            let raw_limit = REQUESTED_MODEL_RAW_PAGE_SIZE
                .min(REQUESTED_MODEL_RAW_SCAN_LIMIT.saturating_sub(raw_offset));
            let raw_page = self
                .load_requested_model_raw_page(
                    &query.api_format,
                    &query.requested_model_name,
                    SqlPage {
                        limit: i64::from(raw_limit),
                        offset: i64::from(raw_offset),
                    },
                )
                .await?;
            exact_page.push_matching(raw_page.rows, |item| {
                row_matches_requested_model(
                    &item.row,
                    &query.requested_model_name,
                    &query.api_format,
                )
            });
            raw_offset = raw_offset.saturating_add(raw_page.raw_len);
            if raw_page.raw_len < raw_limit || raw_page.raw_len == 0 {
                break;
            }
        }

        let rows = exact_page
            .into_page()
            .into_iter()
            .map(|item| item.row)
            .collect();
        Ok(dedupe_candidate_selection_rows(rows))
    }

    async fn list_pool_key_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        let canonical_api_format = normalize_api_format(&query.api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let match_aliases =
            sql_match_aliases(&api_format_permission_aliases(&canonical_api_format));
        let mut rows = Vec::<CandidateSelectionRow>::new();

        for storage_api_format in storage_aliases {
            let mut builder = QueryBuilder::<Sqlite>::new(CANDIDATE_SELECTION_COLUMNS);
            push_candidate_sql_filters(&mut builder, &storage_api_format, &match_aliases);
            builder.push(" AND p.id = ");
            builder.push_bind(&query.provider_id);
            builder.push(" AND pe.id = ");
            builder.push_bind(&query.endpoint_id);
            builder.push(" AND m.id = ");
            builder.push_bind(&query.model_id);
            push_pool_key_order(&mut builder, &query.order);
            builder.push(" LIMIT ");
            builder.push_bind(i64::from(query.limit));
            builder.push(" OFFSET ");
            builder.push_bind(i64::from(query.offset));

            let query_rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
            let mut items = query_rows
                .iter()
                .map(map_candidate_selection_row)
                .collect::<Result<Vec<_>, _>>()?;
            items.retain(|item| {
                api_format_matches(&item.row.endpoint_api_format, &canonical_api_format)
                    && item.row.key_supports_api_format(&canonical_api_format)
                    && key_auth_channel_matches(item, &canonical_api_format)
            });
            rows.extend(items);
        }

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
        let canonical_api_format = normalize_api_format(&query.api_format);
        let storage_aliases = api_format_aliases(&canonical_api_format);
        let match_aliases =
            sql_match_aliases(&api_format_permission_aliases(&canonical_api_format));
        let mut rows = Vec::new();

        for storage_api_format in storage_aliases {
            let mut builder = QueryBuilder::<Sqlite>::new(CANDIDATE_SELECTION_COLUMNS);
            push_candidate_sql_filters(&mut builder, &storage_api_format, &match_aliases);
            builder.push(" AND p.id = ");
            builder.push_bind(&query.provider_id);
            builder.push(" AND pe.id = ");
            builder.push_bind(&query.endpoint_id);
            builder.push(" AND m.id = ");
            builder.push_bind(&query.model_id);
            builder.push(" AND pak.id IN (");
            {
                let mut separated = builder.separated(", ");
                for key_id in &query.key_ids {
                    separated.push_bind(key_id);
                }
            }
            builder.push(")");
            builder.push(" ORDER BY CASE pak.id");
            for (index, key_id) in query.key_ids.iter().enumerate() {
                builder.push(" WHEN ");
                builder.push_bind(key_id);
                builder.push(" THEN ");
                builder.push_bind(i64::try_from(index).map_err(|_| {
                    DataLayerError::UnexpectedValue("key id order index overflowed".to_string())
                })?);
            }
            builder.push(" ELSE ");
            builder.push_bind(i64::try_from(query.key_ids.len()).map_err(|_| {
                DataLayerError::UnexpectedValue("key id order length overflowed".to_string())
            })?);
            builder.push(" END ASC, pak.id ASC");

            let query_rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
            let mut items = query_rows
                .iter()
                .map(map_candidate_selection_row)
                .collect::<Result<Vec<_>, _>>()?;
            items.retain(|item| {
                api_format_matches(&item.row.endpoint_api_format, &canonical_api_format)
                    && item.row.key_supports_api_format(&canonical_api_format)
                    && key_auth_channel_matches(item, &canonical_api_format)
            });
            rows.extend(items.into_iter().map(|item| item.row));
        }

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

fn push_candidate_sql_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    storage_api_format: &str,
    match_aliases: &[String],
) {
    builder.push(" AND LOWER(COALESCE(pe.api_format, '')) = ");
    builder.push_bind(storage_api_format.trim().to_ascii_lowercase());
    push_key_api_format_sql_filter(builder, match_aliases);
    push_key_auth_channel_sql_filter(builder, storage_api_format);
}

fn push_candidate_sql_filters_for_aliases(
    builder: &mut QueryBuilder<'_, Sqlite>,
    storage_api_formats: &[String],
    match_aliases: &[String],
    requested_api_format: &str,
) {
    builder.push(" AND LOWER(COALESCE(pe.api_format, '')) IN (");
    push_bind_list(builder, storage_api_formats);
    builder.push(")");
    push_key_api_format_sql_filter(builder, match_aliases);
    push_key_auth_channel_sql_filter(builder, requested_api_format);
}

fn push_key_api_format_sql_filter(
    builder: &mut QueryBuilder<'_, Sqlite>,
    match_aliases: &[String],
) {
    builder.push(
        r#"
  AND (
    pak.api_formats IS NULL
    OR TRIM(pak.api_formats) = ''
    OR CASE
      WHEN json_valid(pak.api_formats) THEN
      (
        (
          json_type(pak.api_formats) = 'array'
          AND EXISTS (
            SELECT 1
            FROM json_each(pak.api_formats) AS fmt
            WHERE LOWER(TRIM(CAST(fmt.value AS TEXT))) IN (
"#,
    );
    push_bind_list(builder, match_aliases);
    builder.push(
        r#"
            )
          )
        )
        OR (
          json_type(pak.api_formats) = 'text'
          AND LOWER(TRIM(CAST(json_extract(pak.api_formats, '$') AS TEXT))) IN (
"#,
    );
    push_bind_list(builder, match_aliases);
    builder.push(
        r#"
          )
        )
        OR (
          json_type(pak.api_formats) = 'text'
          AND EXISTS (
            SELECT 1
            FROM json_each(
              CASE
                WHEN json_valid(CAST(json_extract(pak.api_formats, '$') AS TEXT))
                  THEN CAST(json_extract(pak.api_formats, '$') AS TEXT)
                ELSE '[]'
              END
            ) AS fmt
            WHERE LOWER(TRIM(CAST(fmt.value AS TEXT))) IN (
"#,
    );
    push_bind_list(builder, match_aliases);
    builder.push(
        r#"
            )
          )
        )
      )
      ELSE 0
    END
    OR LOWER(TRIM(pak.api_formats)) IN (
"#,
    );
    push_bind_list(builder, match_aliases);
    builder.push(
        r#"
    )
  )
"#,
    );
}

fn push_key_auth_channel_sql_filter(
    builder: &mut QueryBuilder<'_, Sqlite>,
    storage_api_format: &str,
) {
    let api_format = normalize_api_format(storage_api_format);
    builder.push(
        r#"
  AND (
    (
      LOWER(TRIM(p.provider_type)) = 'codex'
      AND LOWER(TRIM(pak.auth_type)) = 'oauth'
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" IN ('openai:responses', 'openai:responses:compact', 'openai:search', 'openai:image', 'codex:live')
    )
    OR (
      LOWER(TRIM(p.provider_type)) = 'chatgpt_web'
      AND LOWER(TRIM(pak.auth_type)) IN ('oauth', 'bearer')
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" = 'openai:image'
    )
    OR (
      LOWER(TRIM(p.provider_type)) = 'claude_code'
      AND LOWER(TRIM(pak.auth_type)) = 'oauth'
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" = 'claude:messages'
    )
    OR (
      LOWER(TRIM(p.provider_type)) = 'kiro'
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" = 'claude:messages'
      AND (
        LOWER(TRIM(pak.auth_type)) = 'oauth'
        OR (
          LOWER(TRIM(pak.auth_type)) = 'bearer'
          AND pak.auth_config IS NOT NULL
          AND TRIM(pak.auth_config) <> ''
        )
      )
    )
    OR (
      LOWER(TRIM(p.provider_type)) IN ('gemini_cli', 'antigravity')
      AND LOWER(TRIM(pak.auth_type)) = 'oauth'
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" = 'gemini:generate_content'
    )
    OR (
      LOWER(TRIM(p.provider_type)) = 'grok'
      AND LOWER(TRIM(pak.auth_type)) = 'oauth'
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" IN ('openai:chat', 'openai:responses', 'claude:messages', 'openai:image')
    )
    OR (
      LOWER(TRIM(p.provider_type)) = 'windsurf'
      AND LOWER(TRIM(pak.auth_type)) IN ('oauth', 'api_key', 'bearer')
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" = 'openai:chat'
    )
    OR (
      LOWER(TRIM(p.provider_type)) = 'vertex_ai'
      AND LOWER(TRIM(pak.auth_type)) IN ('api_key', 'service_account', 'vertex_ai')
      AND "#,
    );
    builder.push_bind(api_format.clone());
    builder.push(
        r#" IN ('gemini:generate_content', 'gemini:embedding')
    )
    OR (
      LOWER(TRIM(p.provider_type)) NOT IN (
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
      AND LOWER(TRIM(pak.auth_type)) <> 'oauth'
    )
  )
"#,
    );
}

fn push_requested_model_sql_filter(
    builder: &mut QueryBuilder<'_, Sqlite>,
    requested_model_name: &str,
    _match_aliases: &[String],
) {
    builder.push(
        r#"
  AND (
    gm.name = "#,
    );
    builder.push_bind(requested_model_name.to_string());
    builder.push(
        r#"
    OR m.provider_model_name = "#,
    );
    builder.push_bind(requested_model_name.to_string());
    builder.push(
        r#"
    OR (
      m.provider_model_mappings IS NOT NULL
      AND m.provider_model_mappings LIKE "#,
    );
    builder.push_bind(format!(
        "%{}%",
        requested_model_name
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    ));
    builder.push(
        r#"
      ESCAPE '\'
    )
  )
"#,
    );
}

fn push_selected_rows_query_tail(
    builder: &mut QueryBuilder<'_, Sqlite>,
    order: SelectedRowsOrder,
    page: Option<SqlPage>,
) {
    builder.push(
        r#"
),
pool_rows AS (
  SELECT candidate.*
  FROM candidate_rows candidate
  WHERE candidate.provider_pool_enabled = 1
    AND NOT EXISTS (
      SELECT 1
      FROM candidate_rows other
      WHERE other.provider_pool_enabled = 1
        AND other.provider_id = candidate.provider_id
        AND other.endpoint_id = candidate.endpoint_id
        AND other.model_id = candidate.model_id
        AND (
          other.key_internal_priority < candidate.key_internal_priority
          OR (
            other.key_internal_priority = candidate.key_internal_priority
            AND other.key_id < candidate.key_id
          )
        )
    )
),
selected_rows AS (
  SELECT * FROM candidate_rows WHERE provider_pool_enabled = 0
  UNION ALL
  SELECT * FROM pool_rows
)
SELECT * FROM selected_rows
"#,
    );
    push_selected_rows_order(builder, order);
    if let Some(page) = page {
        builder.push(" LIMIT ");
        builder.push_bind(page.limit);
        builder.push(" OFFSET ");
        builder.push_bind(page.offset);
    }
}

fn push_selected_rows_order(builder: &mut QueryBuilder<'_, Sqlite>, order: SelectedRowsOrder) {
    builder.push(" ORDER BY ");
    if matches!(order, SelectedRowsOrder::WithGlobalModel) {
        builder.push("global_model_name ASC, ");
    }
    builder.push(
        "provider_priority ASC, key_internal_priority ASC, provider_id ASC, endpoint_id ASC, key_id ASC, model_id ASC",
    );
}

fn push_pool_key_order(
    builder: &mut QueryBuilder<'_, Sqlite>,
    order: &StoredPoolKeyCandidateOrder,
) {
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
            push_seeded_pool_key_order(builder, seed);
        }
    }
}

fn push_seeded_pool_key_order(builder: &mut QueryBuilder<'_, Sqlite>, seed: &str) {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    const PLACEHOLDERS: &[u8; 16] = b"ghijklmnopqrstuv";

    let mut digits_by_rank = HEX_DIGITS.map(char::from);
    digits_by_rank.sort_by(|left, right| {
        stable_pool_key_hash(seed, &left.to_string())
            .cmp(&stable_pool_key_hash(seed, &right.to_string()))
            .then(left.cmp(right))
    });
    let mut rank_by_digit = ['0'; 16];
    for (rank, digit) in digits_by_rank.into_iter().enumerate() {
        let digit_index = digit
            .to_digit(16)
            .expect("seeded pool-key rank input must be hexadecimal")
            as usize;
        rank_by_digit[digit_index] = char::from(HEX_DIGITS[rank]);
    }

    // SQLite has no built-in hash; placeholders avoid cascading replacements
    // while remapping every key-id nibble to a seed-derived rank.
    builder.push(" ORDER BY ");
    for _ in 0..(HEX_DIGITS.len() + PLACEHOLDERS.len()) {
        builder.push("replace(");
    }
    builder.push("lower(hex(pak.id))");
    for (digit, placeholder) in HEX_DIGITS.iter().zip(PLACEHOLDERS) {
        builder.push(format!(
            ", '{}', '{}')",
            char::from(*digit),
            char::from(*placeholder)
        ));
    }
    for (placeholder, rank) in PLACEHOLDERS.iter().zip(rank_by_digit) {
        builder.push(format!(", '{}', ", char::from(*placeholder)));
        builder.push_bind(rank.to_string());
        builder.push(")");
    }
    builder.push(" ASC, pak.id ASC");
}

fn push_bind_list(builder: &mut QueryBuilder<'_, Sqlite>, values: &[String]) {
    let mut separated = builder.separated(", ");
    for value in values {
        separated.push_bind(value.clone());
    }
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

fn sort_candidate_selection_rows(
    rows: &mut [StoredMinimalCandidateSelectionRow],
    include_global_model: bool,
) {
    rows.sort_by(|left, right| {
        let global_model_order = if include_global_model {
            left.global_model_name.cmp(&right.global_model_name)
        } else {
            std::cmp::Ordering::Equal
        };
        global_model_order
            .then(left.provider_priority.cmp(&right.provider_priority))
            .then(left.key_internal_priority.cmp(&right.key_internal_priority))
            .then(left.provider_id.cmp(&right.provider_id))
            .then(left.endpoint_id.cmp(&right.endpoint_id))
            .then(left.key_id.cmp(&right.key_id))
            .then(left.model_id.cmp(&right.model_id))
    });
}

fn map_candidate_selection_row(row: &SqliteRow) -> Result<CandidateSelectionRow, DataLayerError> {
    let _provider_config = parse_json(row.try_get("provider_config").ok().flatten())?;
    let global_model_config = parse_json(row.try_get("global_model_config").ok().flatten())?;
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
            provider_pool_enabled: row
                .try_get::<i64, _>("provider_pool_enabled")
                .map_sql_err()?
                != 0,
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
        provider_model_mapping_api_format_covers, push_key_auth_channel_sql_filter,
        push_pool_key_order, vertex_key_auth_channel_matches, ExactPageAccumulator,
        SqliteMinimalCandidateSelectionReadRepository, REQUESTED_MODEL_RAW_SCAN_LIMIT,
    };
    use crate::run_migrations;
    use aether_data_contracts::repository::candidate_selection::{
        MinimalCandidateSelectionReadRepository, StoredPoolKeyCandidateOrder,
        StoredPoolKeyCandidateRowsQuery, StoredRequestedModelCandidateRowsQuery,
    };

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

        let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT 1 WHERE 1 = 1");
        push_key_auth_channel_sql_filter(&mut builder, "claude:messages");
        let sql = builder.sql();
        let vertex_clause = sql
            .split_once("LOWER(TRIM(p.provider_type)) = 'vertex_ai'")
            .and_then(|(_, suffix)| suffix.split_once("LOWER(TRIM(p.provider_type)) NOT IN"))
            .map(|(clause, _)| clause)
            .expect("Vertex auth clause should exist");
        assert!(!vertex_clause.contains("claude:messages"));
        assert!(vertex_clause.contains("gemini:generate_content"));
        assert!(vertex_clause.contains("gemini:embedding"));
    }

    #[test]
    fn codex_auth_sql_allows_live_for_oauth_keys() {
        let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT 1 WHERE 1 = 1");
        push_key_auth_channel_sql_filter(&mut builder, "codex:live");
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
    fn sqlite_mapping_scope_keeps_legacy_responses_compatibility_codex_only() {
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
    fn load_balance_pool_key_order_is_seeded_and_pageable_in_sql() {
        let sql_for_seed = |seed: &str| {
            let mut builder =
                sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT pak.id FROM provider_api_keys pak");
            push_pool_key_order(
                &mut builder,
                &StoredPoolKeyCandidateOrder::LoadBalance {
                    seed: seed.to_string(),
                },
            );
            builder.push(" LIMIT ");
            builder.push_bind(64_i64);
            builder.push(" OFFSET ");
            builder.push_bind(128_i64);
            builder.sql().to_string()
        };

        let first_seed_sql = sql_for_seed("seed-a");
        let second_seed_sql = sql_for_seed("seed-b");

        assert!(first_seed_sql.contains("lower(hex(pak.id))"));
        assert!(first_seed_sql.contains("ASC, pak.id ASC LIMIT ? OFFSET ?"));
        assert_eq!(first_seed_sql, second_seed_sql);
        assert_eq!(first_seed_sql.matches('?').count(), 18);
    }

    #[tokio::test]
    async fn sqlite_repository_reads_candidate_selection_rows() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_candidate_selection(&pool).await;

        let repository = SqliteMinimalCandidateSelectionReadRepository::new(pool);
        let rows = repository
            .list_for_exact_api_format("openai:chat")
            .await
            .expect("candidate rows should load");
        assert_eq!(
            rows.iter()
                .map(|row| row.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-windsurf-oauth", "key-1"]
        );
        assert_eq!(
            rows[1].global_model_mappings,
            Some(vec!["alias-global".to_string()])
        );
        assert_eq!(rows[1].global_model_supports_streaming, Some(true));
        assert_eq!(
            rows[1]
                .model_provider_model_mappings
                .as_ref()
                .and_then(|mappings| mappings.first())
                .and_then(|mapping| mapping.operations.as_ref()),
            Some(&vec!["compact".to_string()])
        );

        let requested = repository
            .list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:chat".to_string(),
                    requested_model_name: "alias-provider".to_string(),
                    offset: 0,
                    limit: 10,
                },
            )
            .await
            .expect("requested model rows should load");
        assert_eq!(requested.len(), 1);

        let windsurf_requested = repository
            .list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:chat".to_string(),
                    requested_model_name: "claude-opus-4-7".to_string(),
                    offset: 0,
                    limit: 10,
                },
            )
            .await
            .expect("windsurf oauth rows should load");
        assert_eq!(
            windsurf_requested
                .iter()
                .map(|row| row.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-windsurf-oauth"]
        );

        let pool_keys = repository
            .list_pool_key_rows_for_group(&StoredPoolKeyCandidateRowsQuery {
                api_format: "openai:chat".to_string(),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                model_id: "model-1".to_string(),
                selected_provider_model_name: "provider-model".to_string(),
                order: StoredPoolKeyCandidateOrder::InternalPriority,
                offset: 1,
                limit: 1,
            })
            .await
            .expect("pool keys should load");
        assert_eq!(pool_keys.len(), 1);
        assert_eq!(pool_keys[0].key_id, "key-2");

        let image_rows = repository
            .list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:image".to_string(),
                    requested_model_name: "gpt-image-2".to_string(),
                    offset: 0,
                    limit: 10,
                },
            )
            .await
            .expect("chatgpt web image rows should load");
        assert_eq!(
            image_rows
                .iter()
                .map(|row| row.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-chatgpt-web-oauth", "key-chatgpt-web-bearer"]
        );

        let search_rows = repository
            .list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:search".to_string(),
                    requested_model_name: "gpt-5.6-sol".to_string(),
                    offset: 0,
                    limit: 10,
                },
            )
            .await
            .expect("Codex Search rows should load through Responses permissions");
        assert_eq!(search_rows.len(), 1);
        assert_eq!(search_rows[0].key_id, "key-codex-search");
        assert_eq!(search_rows[0].endpoint_api_format, "openai:search");
    }

    #[tokio::test]
    async fn sqlite_requested_model_page_crosses_coarse_false_positive_windows() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_requested_model_pagination(&pool).await;

        let repository = SqliteMinimalCandidateSelectionReadRepository::new(pool);
        let rows = repository
            .list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:chat".to_string(),
                    requested_model_name: "sqlite-page-target".to_string(),
                    offset: 1,
                    limit: 1,
                },
            )
            .await
            .expect("requested model page should cross the coarse-only window");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model_id, "model-pagination-exact-1");
    }

    #[tokio::test]
    async fn sqlite_load_balance_pool_key_pages_use_stable_seeded_order() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_candidate_selection(&pool).await;

        let repository = SqliteMinimalCandidateSelectionReadRepository::new(pool);
        let load_page = |seed: &str, offset, limit| StoredPoolKeyCandidateRowsQuery {
            api_format: "openai:chat".to_string(),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            model_id: "model-1".to_string(),
            selected_provider_model_name: "provider-model".to_string(),
            order: StoredPoolKeyCandidateOrder::LoadBalance {
                seed: seed.to_string(),
            },
            offset,
            limit,
        };

        let seed_a_first = repository
            .list_pool_key_rows_for_group(&load_page("seed-a", 0, 1))
            .await
            .expect("first load-balance page should load");
        let seed_a_second = repository
            .list_pool_key_rows_for_group(&load_page("seed-a", 1, 1))
            .await
            .expect("second load-balance page should load");
        let seed_a_replay = repository
            .list_pool_key_rows_for_group(&load_page("seed-a", 0, 2))
            .await
            .expect("replayed load-balance window should load");
        let seed_b = repository
            .list_pool_key_rows_for_group(&load_page("seed-b", 0, 2))
            .await
            .expect("alternate load-balance seed should load");

        let seed_a_pages = seed_a_first
            .iter()
            .chain(&seed_a_second)
            .map(|row| row.key_id.as_str())
            .collect::<Vec<_>>();
        let seed_a_replay = seed_a_replay
            .iter()
            .map(|row| row.key_id.as_str())
            .collect::<Vec<_>>();
        let seed_b = seed_b
            .iter()
            .map(|row| row.key_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(seed_a_pages, vec!["key-1", "key-2"]);
        assert_eq!(seed_a_pages, seed_a_replay);
        assert_eq!(seed_b, vec!["key-2", "key-1"]);
    }

    async fn seed_candidate_selection(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO providers (
  id, name, provider_type, provider_priority, config, is_active, created_at, updated_at
)
VALUES ('provider-1', 'Provider One', 'custom', 10, '{"pool_advanced":{}}', 1, 1, 1);

INSERT INTO provider_endpoints (
  id, provider_id, name, base_url, api_format, is_active, created_at, updated_at
)
VALUES ('endpoint-1', 'provider-1', 'Endpoint One', 'https://example.test', 'openai:chat', 1, 1, 1);

INSERT INTO provider_api_keys (
  id, provider_id, name, auth_type, api_formats, internal_priority, is_active, created_at, updated_at
)
VALUES
  ('key-1', 'provider-1', 'Key One', 'api_key', '["openai:chat"]', 10, 1, 1, 1),
  ('key-2', 'provider-1', 'Key Two', 'api_key', '["openai:chat"]', 20, 1, 1, 1);

INSERT INTO providers (
  id, name, provider_type, provider_priority, is_active, created_at, updated_at
)
VALUES ('provider-chatgpt-web', 'ChatGPT Web', 'chatgpt_web', 20, 1, 1, 1);

INSERT INTO providers (
  id, name, provider_type, provider_priority, is_active, created_at, updated_at
)
VALUES ('provider-windsurf', 'Windsurf', 'windsurf', 15, 1, 1, 1);

INSERT INTO providers (
  id, name, provider_type, provider_priority, is_active, created_at, updated_at
)
VALUES ('provider-codex-search', 'Codex Search', 'codex', 12, 1, 1, 1);

INSERT INTO provider_endpoints (
  id, provider_id, name, base_url, api_format, is_active, created_at, updated_at
)
VALUES (
  'endpoint-chatgpt-web', 'provider-chatgpt-web', 'ChatGPT Web Image',
  'https://chatgpt.com', 'openai:image', 1, 1, 1
);

INSERT INTO provider_endpoints (
  id, provider_id, name, base_url, api_format, is_active, created_at, updated_at
)
VALUES (
  'endpoint-windsurf', 'provider-windsurf', 'Windsurf Chat',
  'https://server.codeium.com', 'openai:chat', 1, 1, 1
);

INSERT INTO provider_endpoints (
  id, provider_id, name, base_url, api_format, is_active, created_at, updated_at
)
VALUES (
  'endpoint-codex-search', 'provider-codex-search', 'Codex Search',
  'https://chatgpt.com/backend-api/codex', 'openai:search', 1, 1, 1
);

INSERT INTO provider_api_keys (
  id, provider_id, name, auth_type, api_formats, internal_priority, is_active, created_at, updated_at
)
VALUES
  ('key-chatgpt-web-oauth', 'provider-chatgpt-web', 'OAuth', 'oauth', '["openai:image"]', 10, 1, 1, 1),
  ('key-chatgpt-web-bearer', 'provider-chatgpt-web', 'Bearer', 'bearer', '["openai:image"]', 20, 1, 1, 1),
  ('key-chatgpt-web-api-key', 'provider-chatgpt-web', 'API Key', 'api_key', '["openai:image"]', 30, 1, 1, 1);

INSERT INTO provider_api_keys (
  id, provider_id, name, auth_type, api_formats, internal_priority, is_active, created_at, updated_at
)
VALUES (
  'key-windsurf-oauth', 'provider-windsurf', 'OAuth', 'oauth', '["openai:chat"]', 10, 1, 1, 1
);

INSERT INTO provider_api_keys (
  id, provider_id, name, auth_type, api_formats, internal_priority, is_active, created_at, updated_at
)
VALUES (
  'key-codex-search', 'provider-codex-search', 'OAuth', 'oauth',
  '["openai:responses"]', 10, 1, 1, 1
);

INSERT INTO global_models (
  id, name, config, is_active, created_at, updated_at
)
VALUES
  ('global-1', 'gpt-5', '{"model_mappings":["alias-global"],"streaming":true}', 1, 1, 1),
  ('global-image-1', 'gpt-image-2', NULL, 1, 1, 1),
  ('global-windsurf-1', 'claude-opus-4-7', '{"streaming":true}', 1, 1, 1),
  ('global-codex-search-1', 'search-global', '{"streaming":false}', 1, 1, 1);

INSERT INTO models (
  id, provider_id, global_model_id, provider_model_name, provider_model_mappings,
  supports_streaming, is_active, is_available, created_at, updated_at
)
VALUES (
  'model-1', 'provider-1', 'global-1', 'provider-model',
  '[{"name":"alias-provider","api_formats":["openai:chat"],"operations":["COMPACT"],"priority":1}]',
  1, 1, 1, 1, 1
),
(
  'model-chatgpt-web-image', 'provider-chatgpt-web', 'global-image-1', 'gpt-image-2',
  NULL, 1, 1, 1, 1, 1
),
(
  'model-windsurf-opus', 'provider-windsurf', 'global-windsurf-1', 'claude-opus-4-7',
  NULL, NULL, 1, 1, 1, 1
),
(
  'model-codex-search', 'provider-codex-search', 'global-codex-search-1', 'search-upstream',
  '[{"name":"gpt-5.6-sol","api_formats":["openai:responses"],"priority":1}]',
  0, 1, 1, 1, 1
);
"#,
        )
        .execute(pool)
        .await
        .expect("candidate selection rows should seed");
    }

    async fn seed_requested_model_pagination(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO providers (
  id, name, provider_type, provider_priority, is_active, created_at, updated_at
)
VALUES (
  'provider-pagination', 'Pagination Provider', 'custom', 10, 1, 1, 1
);

INSERT INTO provider_endpoints (
  id, provider_id, name, base_url, api_format, is_active, created_at, updated_at
)
VALUES (
  'endpoint-pagination', 'provider-pagination', 'Pagination Endpoint',
  'https://example.test', 'openai:chat', 1, 1, 1
);

INSERT INTO provider_api_keys (
  id, provider_id, name, auth_type, api_formats, internal_priority,
  is_active, created_at, updated_at
)
VALUES (
  'key-pagination', 'provider-pagination', 'Pagination Key', 'api_key',
  '["openai:chat"]', 10, 1, 1, 1
);

WITH RECURSIVE sequence(value) AS (
  SELECT 0
  UNION ALL
  SELECT value + 1 FROM sequence WHERE value < 255
)
INSERT INTO global_models (
  id, name, is_active, created_at, updated_at
)
SELECT
  printf('global-pagination-false-%03d', value),
  printf('a-pagination-false-%03d', value),
  1, 1, 1
FROM sequence;

INSERT INTO global_models (
  id, name, is_active, created_at, updated_at
)
VALUES
  ('global-pagination-exact-0', 'z-pagination-exact-0', 1, 1, 1),
  ('global-pagination-exact-1', 'z-pagination-exact-1', 1, 1, 1);

WITH RECURSIVE sequence(value) AS (
  SELECT 0
  UNION ALL
  SELECT value + 1 FROM sequence WHERE value < 255
)
INSERT INTO models (
  id, provider_id, global_model_id, provider_model_name, provider_model_mappings,
  is_active, is_available, created_at, updated_at
)
SELECT
  printf('model-pagination-false-%03d', value),
  'provider-pagination',
  printf('global-pagination-false-%03d', value),
  'upstream-false',
  '[{"name":"sqlite-page-target-noise","api_formats":["openai:chat"],"priority":1}]',
  1, 1, 1, 1
FROM sequence;

INSERT INTO models (
  id, provider_id, global_model_id, provider_model_name, provider_model_mappings,
  is_active, is_available, created_at, updated_at
)
VALUES
  (
    'model-pagination-exact-0', 'provider-pagination', 'global-pagination-exact-0',
    'upstream-exact-0',
    '[{"name":"sqlite-page-target","api_formats":["openai:chat"],"priority":1}]',
    1, 1, 1, 1
  ),
  (
    'model-pagination-exact-1', 'provider-pagination', 'global-pagination-exact-1',
    'upstream-exact-1',
    '[{"name":"sqlite-page-target","api_formats":["openai:chat"],"priority":1}]',
    1, 1, 1, 1
  );
"#,
        )
        .execute(pool)
        .await
        .expect("requested model pagination rows should seed");
    }
}
