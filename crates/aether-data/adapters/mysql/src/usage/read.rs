use aether_data_contracts::repository::usage::{
    ProviderApiKeyWindowUsageRequest, StoredRequestUsageAudit, UsageAuditKeywordSearchQuery,
    UsageAuditListQuery, UsageMonitoringErrorCountQuery, UsageMonitoringErrorListQuery,
};
use aether_data_contracts::DataLayerError;
use sqlx::{MySql, QueryBuilder};

use crate::error::SqlResultExt;

use super::{http_capture, map_usage_row, row_u64, to_i64, MysqlUsageStorage, USAGE_COLUMNS};

const EFFECTIVE_PROVIDER_ID_EXPR: &str = r#"CASE
  WHEN usage_routing_snapshots.request_id IS NOT NULL
  THEN usage_routing_snapshots.selected_provider_id
  ELSE `usage`.provider_id
END"#;

const EFFECTIVE_PROVIDER_API_KEY_ID_EXPR: &str = r#"CASE
  WHEN usage_routing_snapshots.request_id IS NOT NULL
  THEN usage_routing_snapshots.selected_provider_api_key_id
  ELSE `usage`.provider_api_key_id
END"#;

const MONITORING_ERROR_PREDICATE: &str = "`usage`.outcome_class = 'service_error'";
const SUCCESS_PERCENTILE_PREDICATE: &str = r#"(
  `usage`.outcome_class = 'success'
  OR (
    `usage`.outcome_class = 'in_flight'
    AND LOWER(TRIM(COALESCE(`usage`.status, ''))) = 'completed'
    AND (`usage`.status_code IS NULL OR `usage`.status_code < 400)
    AND TRIM(COALESCE(`usage`.error_message, '')) = ''
  )
)"#;

/// A SQL-side superset filter used before the runtime applies complex usage analytics.
///
/// Every scan has explicit time bounds; an empty range returns no rows. Optional dimensions
/// further reduce the rows sent to the in-memory analytics implementation without changing its
/// result.
#[derive(Debug, Clone)]
pub struct MysqlUsageReadFilter {
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
    user_id: Option<String>,
    api_key_id: Option<String>,
    provider_name: Option<String>,
    provider_id: Option<String>,
    model: Option<String>,
    api_format: Option<String>,
    endpoint_kind: Option<String>,
    is_stream: Option<bool>,
    has_format_conversion: Option<bool>,
    finalized_only: bool,
    completed_only: bool,
    success_only: bool,
}

impl MysqlUsageReadFilter {
    pub fn new(created_from_unix_secs: u64, created_until_unix_secs: u64) -> Self {
        Self {
            created_from_unix_secs,
            created_until_unix_secs,
            user_id: None,
            api_key_id: None,
            provider_name: None,
            provider_id: None,
            model: None,
            api_format: None,
            endpoint_kind: None,
            is_stream: None,
            has_format_conversion: None,
            finalized_only: false,
            completed_only: false,
            success_only: false,
        }
    }

    pub fn with_user_id(mut self, value: Option<&str>) -> Self {
        self.user_id = value.map(ToOwned::to_owned);
        self
    }

    pub fn with_api_key_id(mut self, value: Option<&str>) -> Self {
        self.api_key_id = value.map(ToOwned::to_owned);
        self
    }

    pub fn with_provider_name(mut self, value: Option<&str>) -> Self {
        self.provider_name = value.map(ToOwned::to_owned);
        self
    }

    pub fn with_provider_id(mut self, value: Option<&str>) -> Self {
        self.provider_id = value.map(ToOwned::to_owned);
        self
    }

    pub fn with_model(mut self, value: Option<&str>) -> Self {
        self.model = value.map(ToOwned::to_owned);
        self
    }

    pub fn with_api_format(mut self, value: Option<&str>) -> Self {
        self.api_format = value.map(ToOwned::to_owned);
        self
    }

    pub fn with_endpoint_kind(mut self, value: Option<&str>) -> Self {
        self.endpoint_kind = value.map(ToOwned::to_owned);
        self
    }

    pub fn with_is_stream(mut self, value: Option<bool>) -> Self {
        self.is_stream = value;
        self
    }

    pub fn with_has_format_conversion(mut self, value: Option<bool>) -> Self {
        self.has_format_conversion = value;
        self
    }

    pub fn finalized_only(mut self) -> Self {
        self.finalized_only = true;
        self
    }

    pub fn completed_only(mut self) -> Self {
        self.completed_only = true;
        self
    }

    /// Restrict the SQL-side read model to canonical successful requests.
    ///
    /// This is intentionally separate from `completed_only`: a completed HTTP 400 is a
    /// terminal user error and must not enter latency percentile samples.
    pub fn success_only(mut self) -> Self {
        self.success_only = true;
        self
    }

    fn is_empty(&self) -> bool {
        self.created_from_unix_secs >= self.created_until_unix_secs
    }
}

impl MysqlUsageStorage {
    pub async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(&format!("{USAGE_COLUMNS} WHERE `usage`.id = ? LIMIT 1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref()
            .map(|row| map_usage_row(row, false))
            .transpose()
    }

    pub async fn list_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        builder.push(" WHERE `usage`.id IN (");
        {
            let mut separated = builder.separated(", ");
            for id in ids {
                separated.push_bind(id.clone());
            }
        }
        builder.push(") ORDER BY created_at_unix_ms DESC, `usage`.id ASC");
        self.fetch_usage_items(builder).await
    }

    pub async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(&format!(
            "{USAGE_COLUMNS} WHERE `usage`.request_id = ? LIMIT 1"
        ))
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        let usage = row
            .as_ref()
            .map(|row| map_usage_row(row, true))
            .transpose()?;
        match usage {
            Some(usage) => http_capture::hydrate_usage_body_refs(&self.pool, usage)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub async fn resolve_body_ref(
        &self,
        body_ref: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        http_capture::resolve_body_ref(&self.pool, body_ref).await
    }

    pub async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        let mut has_where = false;
        push_list_filters(&mut builder, query, &mut has_where)?;
        push_order_limit_offset(&mut builder, query.newest_first, query.limit, query.offset)?;
        self.fetch_usage_items(builder).await
    }

    pub async fn count_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<MySql>::new("SELECT CAST(COUNT(*) AS SIGNED) AS total FROM `usage`");
        let mut has_where = false;
        push_list_filters(&mut builder, query, &mut has_where)?;
        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        row_u64(&row, "total")
    }

    pub async fn list_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        let mut has_where = false;
        push_keyword_filters(&mut builder, query, &mut has_where)?;
        push_order_limit_offset(&mut builder, query.newest_first, query.limit, query.offset)?;
        self.fetch_usage_items(builder).await
    }

    pub async fn count_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<MySql>::new("SELECT CAST(COUNT(*) AS SIGNED) AS total FROM `usage`");
        let mut has_where = false;
        push_keyword_filters(&mut builder, query, &mut has_where)?;
        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        row_u64(&row, "total")
    }

    pub async fn load_usage_records_in_range(
        &self,
        filter: &MysqlUsageReadFilter,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if filter.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = build_range_query(filter)?;
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(|row| map_usage_row(row, false)).collect()
    }

    pub async fn count_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorCountQuery,
    ) -> Result<u64, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(0);
        }
        let row = sqlx::query(&format!(
            r#"
SELECT CAST(COUNT(*) AS SIGNED) AS total
FROM `usage`
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND {MONITORING_ERROR_PREDICATE}
"#
        ))
        .bind(to_i64(
            query.created_from_unix_secs,
            "usage.created_at_unix_ms",
        )?)
        .bind(to_i64(
            query.created_until_unix_secs,
            "usage.created_at_unix_ms",
        )?)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        row_u64(&row, "total")
    }

    pub async fn list_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        builder
            .push(" WHERE `usage`.created_at_unix_ms >= ")
            .push_bind(to_i64(
                query.created_from_unix_secs,
                "usage.created_at_unix_ms",
            )?)
            .push(" AND `usage`.created_at_unix_ms < ")
            .push_bind(to_i64(
                query.created_until_unix_secs,
                "usage.created_at_unix_ms",
            )?)
            .push(" AND ")
            .push(MONITORING_ERROR_PREDICATE)
            .push(" ORDER BY created_at_unix_ms DESC, `usage`.id ASC");
        if let Some(limit) = query.limit {
            builder
                .push(" LIMIT ")
                .push_bind(usize_to_i64(limit, "usage monitoring limit")?);
        }
        self.fetch_usage_items(builder).await
    }

    pub async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        if let Some(user_id) = user_id {
            builder
                .push(" WHERE `usage`.user_id = ")
                .push_bind(user_id.to_string());
        }
        builder
            .push(" ORDER BY created_at_unix_ms DESC, `usage`.id ASC LIMIT ")
            .push_bind(usize_to_i64(limit, "recent usage limit")?);
        self.fetch_usage_items(builder).await
    }

    pub async fn load_usage_records_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        builder.push(" WHERE `usage`.api_key_id IN (");
        push_string_list(&mut builder, api_key_ids);
        builder.push(") ORDER BY `usage`.created_at_unix_ms ASC, `usage`.request_id ASC");
        self.fetch_usage_items(builder).await
    }

    pub async fn load_usage_records_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if provider_api_key_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        builder
            .push(" WHERE (")
            .push(EFFECTIVE_PROVIDER_API_KEY_ID_EXPR)
            .push(") IN (");
        push_string_list(&mut builder, provider_api_key_ids);
        builder.push(") ORDER BY `usage`.created_at_unix_ms ASC, `usage`.request_id ASC");
        self.fetch_usage_items(builder).await
    }

    pub async fn load_usage_records_by_provider_api_key_windows(
        &self,
        requests: &[ProviderApiKeyWindowUsageRequest],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        builder.push(" WHERE ");
        for request in requests {
            let provider_api_key_id = request.provider_api_key_id.trim();
            if provider_api_key_id.is_empty() {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage provider_api_key_id cannot be empty".to_string(),
                ));
            }
            let window_code = request.window_code.trim();
            if window_code.is_empty() {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage window_code cannot be empty".to_string(),
                ));
            }
            if request.start_unix_secs >= request.end_unix_secs {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage range must be non-empty".to_string(),
                ));
            }
        }
        {
            let mut separated = builder.separated(" OR ");
            for request in requests {
                separated
                    .push("((")
                    .push(EFFECTIVE_PROVIDER_API_KEY_ID_EXPR)
                    .push(") = ")
                    .push_bind(request.provider_api_key_id.trim().to_string())
                    .push(" AND `usage`.created_at_unix_ms >= ")
                    .push_bind(to_i64(request.start_unix_secs, "usage.created_at_unix_ms")?)
                    .push(" AND `usage`.created_at_unix_ms < ")
                    .push_bind(to_i64(request.end_unix_secs, "usage.created_at_unix_ms")?)
                    .push(")");
            }
        }
        builder.push(" ORDER BY `usage`.created_at_unix_ms ASC, `usage`.request_id ASC");
        self.fetch_usage_items(builder).await
    }

    pub async fn load_usage_records_for_provider_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        builder
            .push(" WHERE (")
            .push(EFFECTIVE_PROVIDER_ID_EXPR)
            .push(") = ")
            .push_bind(provider_id.to_string())
            .push(" AND `usage`.created_at_unix_ms >= ")
            .push_bind(to_i64(since_unix_secs, "usage.created_at_unix_ms")?)
            .push(" ORDER BY `usage`.created_at_unix_ms ASC, `usage`.request_id ASC");
        self.fetch_usage_items(builder).await
    }

    async fn fetch_usage_items(
        &self,
        mut builder: QueryBuilder<'_, MySql>,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(|row| map_usage_row(row, false)).collect()
    }
}

fn build_range_query(
    filter: &MysqlUsageReadFilter,
) -> Result<QueryBuilder<'static, MySql>, DataLayerError> {
    let mut builder = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
    builder
        .push(" WHERE `usage`.created_at_unix_ms >= ")
        .push_bind(to_i64(
            filter.created_from_unix_secs,
            "usage.created_at_unix_ms",
        )?)
        .push(" AND `usage`.created_at_unix_ms < ")
        .push_bind(to_i64(
            filter.created_until_unix_secs,
            "usage.created_at_unix_ms",
        )?);
    push_optional_text_filter(&mut builder, "`usage`.user_id", filter.user_id.as_deref());
    push_optional_text_filter(
        &mut builder,
        "`usage`.api_key_id",
        filter.api_key_id.as_deref(),
    );
    push_optional_text_filter(
        &mut builder,
        "`usage`.provider_name",
        filter.provider_name.as_deref(),
    );
    if let Some(provider_id) = filter.provider_id.as_deref() {
        builder
            .push(" AND (")
            .push(EFFECTIVE_PROVIDER_ID_EXPR)
            .push(") = ")
            .push_bind(provider_id.to_string());
    }
    push_optional_text_filter(&mut builder, "`usage`.model", filter.model.as_deref());
    push_optional_text_filter(
        &mut builder,
        "`usage`.api_format",
        filter.api_format.as_deref(),
    );
    push_optional_text_filter(
        &mut builder,
        "`usage`.endpoint_kind",
        filter.endpoint_kind.as_deref(),
    );
    if let Some(is_stream) = filter.is_stream {
        builder
            .push(" AND `usage`.is_stream = ")
            .push_bind(is_stream);
    }
    if let Some(has_format_conversion) = filter.has_format_conversion {
        builder
            .push(" AND CASE WHEN usage_routing_snapshots.request_id IS NOT NULL ")
            .push("THEN COALESCE(usage_routing_snapshots.has_format_conversion, FALSE) ")
            .push("ELSE COALESCE(`usage`.has_format_conversion, FALSE) END = ")
            .push_bind(has_format_conversion);
    }
    if filter.finalized_only {
        builder.push(
            " AND `usage`.status NOT IN ('pending', 'streaming') \
AND `usage`.provider_name NOT IN ('unknown', 'pending')",
        );
    }
    if filter.completed_only {
        builder.push(" AND `usage`.status = 'completed'");
    }
    if filter.success_only {
        builder.push(" AND ").push(SUCCESS_PERCENTILE_PREDICATE);
    }
    builder.push(" ORDER BY `usage`.created_at_unix_ms ASC, `usage`.request_id ASC");
    Ok(builder)
}

fn push_list_filters(
    builder: &mut QueryBuilder<'_, MySql>,
    query: &UsageAuditListQuery,
    has_where: &mut bool,
) -> Result<(), DataLayerError> {
    if let Some(value) = query.created_from_unix_secs {
        push_where(builder, has_where);
        builder
            .push("`usage`.created_at_unix_ms >= ")
            .push_bind(to_i64(value, "usage.created_at_unix_ms")?);
    }
    if let Some(value) = query.created_until_unix_secs {
        push_where(builder, has_where);
        builder
            .push("`usage`.created_at_unix_ms < ")
            .push_bind(to_i64(value, "usage.created_at_unix_ms")?);
    }
    for (column, value) in [
        ("`usage`.user_id", query.user_id.as_deref()),
        ("`usage`.provider_name", query.provider_name.as_deref()),
        ("`usage`.model", query.model.as_deref()),
        ("`usage`.api_format", query.api_format.as_deref()),
    ] {
        if let Some(value) = value {
            push_where(builder, has_where);
            builder
                .push(column)
                .push(" = ")
                .push_bind(value.to_string());
        }
    }
    if let Some(client_family) = query
        .client_family
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_where(builder, has_where);
        builder
            .push("LOWER(COALESCE(NULLIF(TRIM(JSON_UNQUOTE(JSON_EXTRACT(`usage`.request_metadata, '$.client_session_affinity.client_family'))), ''), NULLIF(TRIM(JSON_UNQUOTE(JSON_EXTRACT(`usage`.request_metadata, '$.client_family'))), ''))) = ")
            .push_bind(client_family.to_ascii_lowercase());
    }
    if query.exclude_unknown_model_or_provider {
        push_where(builder, has_where);
        builder.push(
            "(LOWER(TRIM(COALESCE(`usage`.model, ''))) NOT IN ('unknown', 'unknow') \
AND LOWER(TRIM(COALESCE(`usage`.provider_name, ''))) NOT IN ('unknown', 'unknow'))",
        );
    }
    if let Some(statuses) = query
        .statuses
        .as_deref()
        .filter(|values| !values.is_empty())
    {
        push_where(builder, has_where);
        builder.push("`usage`.status IN (");
        push_string_list(builder, statuses);
        builder.push(")");
    }
    if !query.exclude_status_codes.is_empty() {
        push_where(builder, has_where);
        builder.push("(`usage`.status_code IS NULL OR `usage`.status_code NOT IN (");
        {
            let mut separated = builder.separated(", ");
            for status_code in &query.exclude_status_codes {
                separated.push_bind(i64::from(*status_code));
            }
        }
        builder.push("))");
    }
    if let Some(is_stream) = query.is_stream {
        push_where(builder, has_where);
        builder.push("`usage`.is_stream = ").push_bind(is_stream);
    }
    if query.error_only {
        push_where(builder, has_where);
        builder.push("`usage`.outcome_class = 'service_error'");
    }
    Ok(())
}

fn push_keyword_filters(
    builder: &mut QueryBuilder<'_, MySql>,
    query: &UsageAuditKeywordSearchQuery,
    has_where: &mut bool,
) -> Result<(), DataLayerError> {
    push_list_filters(
        builder,
        &UsageAuditListQuery {
            created_from_unix_secs: query.created_from_unix_secs,
            created_until_unix_secs: query.created_until_unix_secs,
            user_id: query.user_id.clone(),
            provider_name: query.provider_name.clone(),
            model: query.model.clone(),
            api_format: query.api_format.clone(),
            client_family: query.client_family.clone(),
            exclude_unknown_model_or_provider: query.exclude_unknown_model_or_provider,
            statuses: query.statuses.clone(),
            exclude_status_codes: query.exclude_status_codes.clone(),
            is_stream: query.is_stream,
            error_only: query.error_only,
            limit: None,
            offset: None,
            newest_first: query.newest_first,
        },
        has_where,
    )?;

    for (index, keyword) in query.keywords.iter().enumerate() {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            continue;
        }
        let pattern = format!("%{}%", keyword.to_ascii_lowercase());
        push_where(builder, has_where);
        builder
            .push("(LOWER(COALESCE(`usage`.model, '')) LIKE ")
            .push_bind(pattern.clone())
            .push(" OR LOWER(COALESCE(`usage`.provider_name, '')) LIKE ")
            .push_bind(pattern.clone());
        if query.auth_user_reader_available {
            if let Some(ids) = query
                .matched_user_ids_by_keyword
                .get(index)
                .filter(|ids| !ids.is_empty())
            {
                builder.push(" OR `usage`.user_id IN (");
                push_string_list(builder, ids);
                builder.push(")");
            }
        } else {
            builder
                .push(" OR LOWER(COALESCE(`usage`.username, '')) LIKE ")
                .push_bind(pattern.clone());
        }
        if query.auth_api_key_reader_available {
            if let Some(ids) = query
                .matched_api_key_ids_by_keyword
                .get(index)
                .filter(|ids| !ids.is_empty())
            {
                builder.push(" OR `usage`.api_key_id IN (");
                push_string_list(builder, ids);
                builder.push(")");
            }
        } else {
            builder
                .push(" OR LOWER(COALESCE(`usage`.api_key_name, '')) LIKE ")
                .push_bind(pattern);
        }
        builder.push(")");
    }

    if let Some(username_keyword) = query
        .username_keyword
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_where(builder, has_where);
        if query.auth_user_reader_available {
            if query.matched_user_ids_for_username.is_empty() {
                builder.push("FALSE");
            } else {
                builder.push("`usage`.user_id IN (");
                push_string_list(builder, &query.matched_user_ids_for_username);
                builder.push(")");
            }
        } else {
            builder
                .push("LOWER(COALESCE(`usage`.username, '')) LIKE ")
                .push_bind(format!("%{}%", username_keyword.to_ascii_lowercase()));
        }
    }
    Ok(())
}

fn push_order_limit_offset(
    builder: &mut QueryBuilder<'_, MySql>,
    newest_first: bool,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<(), DataLayerError> {
    if newest_first {
        builder.push(" ORDER BY created_at_unix_ms DESC, `usage`.id ASC");
    } else {
        builder.push(" ORDER BY created_at_unix_ms ASC, `usage`.request_id ASC");
    }
    match (limit, offset) {
        (Some(limit), offset) => {
            builder
                .push(" LIMIT ")
                .push_bind(usize_to_i64(limit, "usage list limit")?);
            if let Some(offset) = offset {
                builder
                    .push(" OFFSET ")
                    .push_bind(usize_to_i64(offset, "usage list offset")?);
            }
        }
        (None, Some(offset)) => {
            builder
                .push(" LIMIT 18446744073709551615 OFFSET ")
                .push_bind(usize_to_i64(offset, "usage list offset")?);
        }
        (None, None) => {}
    }
    Ok(())
}

fn push_where(builder: &mut QueryBuilder<'_, MySql>, has_where: &mut bool) {
    builder.push(if *has_where { " AND " } else { " WHERE " });
    *has_where = true;
}

fn push_optional_text_filter(
    builder: &mut QueryBuilder<'_, MySql>,
    column: &'static str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        builder
            .push(" AND ")
            .push(column)
            .push(" = ")
            .push_bind(value.to_string());
    }
}

fn push_string_list(builder: &mut QueryBuilder<'_, MySql>, values: &[String]) {
    let mut separated = builder.separated(", ");
    for value in values {
        separated.push_bind(value.clone());
    }
}

fn usize_to_i64(value: usize, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| DataLayerError::InvalidInput(format!("{field} overflow")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_reads_always_have_both_time_bounds() {
        let filter = MysqlUsageReadFilter::new(100, 200)
            .with_user_id(Some("user-1"))
            .finalized_only();
        let query = build_range_query(&filter).expect("range query should build");
        let sql = query.sql();
        assert!(sql.contains("created_at_unix_ms >= ?"));
        assert!(sql.contains("created_at_unix_ms < ?"));
        assert!(sql.contains("`usage`.user_id = ?"));
        assert!(sql.contains("status NOT IN ('pending', 'streaming')"));
    }

    #[test]
    fn success_only_keeps_legacy_completed_non_400_rows_but_excludes_400() {
        let filter = MysqlUsageReadFilter::new(100, 200).success_only();
        let query = build_range_query(&filter).expect("range query should build");
        let sql = query.sql();
        assert!(sql.contains("outcome_class = 'success'"));
        assert!(sql.contains("outcome_class = 'in_flight'"));
        assert!(sql.contains("status_code IS NULL OR `usage`.status_code < 400"));
        assert!(sql.contains("error_message"));
    }

    #[test]
    fn audit_reads_keep_pagination_in_mysql() {
        let mut query = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        push_order_limit_offset(&mut query, true, Some(25), Some(50))
            .expect("pagination should build");
        assert!(query.sql().contains("LIMIT ? OFFSET ?"));
    }

    #[test]
    fn offset_only_uses_mysql_unbounded_limit_syntax() {
        let mut query = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        push_order_limit_offset(&mut query, false, None, Some(50))
            .expect("pagination should build");
        assert!(query.sql().contains("LIMIT 18446744073709551615 OFFSET ?"));
    }

    #[test]
    fn usage_projection_and_legacy_keyword_search_keep_snapshot_names() {
        assert!(USAGE_COLUMNS.contains("`usage`.username"));
        assert!(USAGE_COLUMNS.contains("`usage`.api_key_name"));

        let mut query = QueryBuilder::<MySql>::new(USAGE_COLUMNS);
        let mut has_where = false;
        push_keyword_filters(
            &mut query,
            &UsageAuditKeywordSearchQuery {
                keywords: vec!["legacy".to_string()],
                ..UsageAuditKeywordSearchQuery::default()
            },
            &mut has_where,
        )
        .expect("keyword query should build");
        assert!(query
            .sql()
            .contains("LOWER(COALESCE(`usage`.username, '')) LIKE ?"));
        assert!(query
            .sql()
            .contains("LOWER(COALESCE(`usage`.api_key_name, '')) LIKE ?"));
    }
}
