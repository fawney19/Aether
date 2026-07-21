use std::collections::{BTreeMap, BTreeSet};

#[cfg(all(feature = "postgres", feature = "sqlite"))]
use futures_util::TryStreamExt;
use serde_json::Value;
use sqlx::Row;
#[cfg(any(feature = "mysql", feature = "sqlite"))]
use sqlx::{Column, TypeInfo, ValueRef};

use crate::error::SqlResultExt;
use crate::{DataLayerError, DatabaseDriver, SqlDatabaseConfig};

#[cfg(feature = "mysql")]
mod mysql;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(all(test, feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod tests;

#[cfg(feature = "mysql")]
pub use mysql::{
    export_mysql_core_jsonl, export_mysql_jsonl, import_mysql_jsonl, import_mysql_plan,
};
#[cfg(feature = "postgres")]
pub use postgres::{
    export_postgres_core_jsonl, export_postgres_jsonl, import_postgres_jsonl, import_postgres_plan,
};
#[cfg(feature = "sqlite")]
pub use sqlite::{
    export_sqlite_core_jsonl, export_sqlite_jsonl, import_sqlite_jsonl, import_sqlite_plan,
};

#[cfg(all(feature = "postgres", feature = "sqlite"))]
use postgres::{
    is_postgres_boolean_column, is_postgres_timestamp_column, load_postgres_import_columns,
};

#[cfg(all(test, feature = "postgres", feature = "mysql", feature = "sqlite"))]
use postgres::normalize_postgres_import_payload;

pub const EXPORT_FORMAT_VERSION: u32 = 1;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExportDomain {
    Users,
    ApiKeys,
    Providers,
    ProviderKeys,
    Endpoints,
    GlobalModels,
    Models,
    AuthModules,
    OAuthProviders,
    UserOAuthLinks,
    UserGroups,
    UserGroupMembers,
    ProxyNodes,
    SystemConfigs,
    Wallets,
    Usage,
    Billing,
}

impl ExportDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Users => "users",
            Self::ApiKeys => "api_keys",
            Self::Providers => "providers",
            Self::ProviderKeys => "provider_keys",
            Self::Endpoints => "endpoints",
            Self::Models => "models",
            Self::GlobalModels => "global_models",
            Self::AuthModules => "auth_modules",
            Self::OAuthProviders => "oauth_providers",
            Self::UserOAuthLinks => "user_oauth_links",
            Self::UserGroups => "user_groups",
            Self::UserGroupMembers => "user_group_members",
            Self::ProxyNodes => "proxy_nodes",
            Self::SystemConfigs => "system_configs",
            Self::Wallets => "wallets",
            Self::Usage => "usage",
            Self::Billing => "billing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DataExportManifest {
    pub format_version: u32,
    pub created_at_unix_secs: u64,
    pub source_driver: Option<DatabaseDriver>,
    pub domains: Vec<ExportDomain>,
}

impl DataExportManifest {
    pub fn new(
        created_at_unix_secs: u64,
        source_driver: Option<DatabaseDriver>,
        domains: Vec<ExportDomain>,
    ) -> Self {
        let mut domains = domains;
        domains.sort();
        domains.dedup();
        Self {
            format_version: EXPORT_FORMAT_VERSION,
            created_at_unix_secs,
            source_driver,
            domains,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
pub enum DataExportRecord {
    Manifest {
        manifest: DataExportManifest,
    },
    Row {
        domain: ExportDomain,
        id: String,
        payload: Value,
    },
}

impl DataExportRecord {
    pub fn manifest(manifest: DataExportManifest) -> Self {
        Self::Manifest { manifest }
    }

    pub fn row(domain: ExportDomain, id: impl Into<String>, payload: Value) -> Self {
        Self::Row {
            domain,
            id: id.into(),
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataImportPlan {
    pub manifest: DataExportManifest,
    pub rows_by_domain: BTreeMap<ExportDomain, Vec<ExportRow>>,
}

impl DataImportPlan {
    pub fn rows(&self, domain: ExportDomain) -> &[ExportRow] {
        self.rows_by_domain
            .get(&domain)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportRow {
    pub id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataCopyOptions {
    pub omit_request_body_details: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(all(feature = "postgres", feature = "sqlite"))]
struct SqliteCopyColumn {
    name: String,
    declared_type: String,
    not_null: bool,
    has_default: bool,
    primary_key_position: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(all(feature = "postgres", feature = "sqlite"))]
enum SqliteCopyAffinity {
    Integer,
    Real,
    Text,
    Blob,
    Numeric,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(all(feature = "postgres", feature = "sqlite"))]
struct SchemaCopyColumn {
    sqlite: SqliteCopyColumn,
    postgres: PostgresImportColumn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(all(feature = "postgres", feature = "sqlite"))]
struct SchemaCopyTable {
    table_name: String,
    columns: Vec<SchemaCopyColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "postgres")]
struct PostgresImportColumn {
    data_type: String,
    udt_name: String,
    is_nullable: bool,
    has_default: bool,
}

#[cfg(feature = "postgres")]
type PostgresImportColumns = BTreeMap<String, PostgresImportColumn>;
#[cfg(any(feature = "mysql", feature = "sqlite"))]
type ImportColumnNames = BTreeSet<String>;

const USAGE_REQUEST_BODY_DETAIL_COLUMNS: &[&str] = &[
    "request_body",
    "response_body",
    "provider_request_body",
    "client_response_body",
    "request_body_compressed",
    "response_body_compressed",
    "provider_request_body_compressed",
    "client_response_body_compressed",
];

#[cfg(all(feature = "postgres", feature = "sqlite"))]
const REQUEST_BODY_DETAIL_TABLES: &[&str] = &["usage_body_blobs", "usage_http_audits"];
#[cfg(all(feature = "postgres", feature = "sqlite"))]
const LIFECYCLE_TABLES: &[&str] = &["_sqlx_migrations", "schema_backfills"];

pub fn encode_jsonl(records: &[DataExportRecord]) -> Result<String, DataLayerError> {
    validate_export_records(records)?;

    let mut output = String::new();
    for record in records {
        let line = serde_json::to_string(record)
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}

pub fn decode_jsonl(input: &str) -> Result<Vec<DataExportRecord>, DataLayerError> {
    let mut records = Vec::new();
    for (line_index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str::<DataExportRecord>(line).map_err(|err| {
            DataLayerError::InvalidInput(format!(
                "invalid export JSONL record on line {}: {err}",
                line_index + 1
            ))
        })?;
        records.push(record);
    }
    validate_export_records(&records)?;
    Ok(records)
}

pub fn build_import_plan(input: &str) -> Result<DataImportPlan, DataLayerError> {
    let records = decode_jsonl(input)?;
    let manifest = match records.first() {
        Some(DataExportRecord::Manifest { manifest }) => manifest.clone(),
        _ => unreachable!("decode_jsonl validates the manifest record"),
    };
    let mut rows_by_domain = BTreeMap::<ExportDomain, Vec<ExportRow>>::new();
    for record in records.into_iter().skip(1) {
        let DataExportRecord::Row {
            domain,
            id,
            payload,
        } = record
        else {
            return Err(DataLayerError::InvalidInput(
                "export manifest must appear only as the first record".to_string(),
            ));
        };
        rows_by_domain
            .entry(domain)
            .or_default()
            .push(ExportRow { id, payload });
    }
    Ok(DataImportPlan {
        manifest,
        rows_by_domain,
    })
}

pub fn validate_export_records(records: &[DataExportRecord]) -> Result<(), DataLayerError> {
    let Some(DataExportRecord::Manifest { manifest }) = records.first() else {
        return Err(DataLayerError::InvalidInput(
            "export JSONL must start with a manifest record".to_string(),
        ));
    };
    if manifest.format_version != EXPORT_FORMAT_VERSION {
        return Err(DataLayerError::InvalidInput(format!(
            "unsupported export format version {}; expected {}",
            manifest.format_version, EXPORT_FORMAT_VERSION
        )));
    }

    let allowed_domains = manifest.domains.iter().copied().collect::<BTreeSet<_>>();
    let mut seen_ids = BTreeSet::<(ExportDomain, String)>::new();
    for (index, record) in records.iter().enumerate().skip(1) {
        match record {
            DataExportRecord::Manifest { .. } => {
                return Err(DataLayerError::InvalidInput(format!(
                    "export manifest appears more than once at record {}",
                    index + 1
                )));
            }
            DataExportRecord::Row {
                domain,
                id,
                payload: _,
            } => {
                if !allowed_domains.contains(domain) {
                    return Err(DataLayerError::InvalidInput(format!(
                        "record {} uses domain '{}' not declared in manifest",
                        index + 1,
                        domain.as_str()
                    )));
                }
                if id.trim().is_empty() {
                    return Err(DataLayerError::InvalidInput(format!(
                        "record {} has an empty id",
                        index + 1
                    )));
                }
                let key = (*domain, id.clone());
                if !seen_ids.insert(key) {
                    return Err(DataLayerError::InvalidInput(format!(
                        "duplicate '{}' export id '{}' at record {}",
                        domain.as_str(),
                        id,
                        index + 1
                    )));
                }
            }
        }
    }
    Ok(())
}

pub fn sqlite_core_export_domains() -> Vec<ExportDomain> {
    vec![
        ExportDomain::Users,
        ExportDomain::ApiKeys,
        ExportDomain::Providers,
        ExportDomain::ProviderKeys,
        ExportDomain::Endpoints,
        ExportDomain::GlobalModels,
        ExportDomain::Models,
        ExportDomain::AuthModules,
        ExportDomain::OAuthProviders,
        ExportDomain::UserOAuthLinks,
        ExportDomain::UserGroups,
        ExportDomain::UserGroupMembers,
        ExportDomain::ProxyNodes,
        ExportDomain::SystemConfigs,
        ExportDomain::Wallets,
        ExportDomain::Usage,
        ExportDomain::Billing,
    ]
}

pub fn mysql_core_export_domains() -> Vec<ExportDomain> {
    sqlite_core_export_domains()
}

pub fn postgres_core_export_domains() -> Vec<ExportDomain> {
    sqlite_core_export_domains()
}

pub async fn export_database_jsonl(
    database: SqlDatabaseConfig,
    domains: Vec<ExportDomain>,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    match database.driver {
        #[cfg(feature = "sqlite")]
        DatabaseDriver::Sqlite => {
            let pool = crate::driver::sqlite::SqlitePoolFactory::new(database)?.connect_lazy()?;
            if domains.is_empty() {
                export_sqlite_core_jsonl(&pool, created_at_unix_secs).await
            } else {
                export_sqlite_jsonl(&pool, domains, created_at_unix_secs).await
            }
        }
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => {
            let pool = crate::driver::mysql::MysqlPoolFactory::new(database)?.connect_lazy()?;
            if domains.is_empty() {
                export_mysql_core_jsonl(&pool, created_at_unix_secs).await
            } else {
                export_mysql_jsonl(&pool, domains, created_at_unix_secs).await
            }
        }
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => {
            let pool =
                crate::driver::postgres::PostgresPoolFactory::new(database.to_postgres_config()?)?
                    .connect_lazy()?;
            if domains.is_empty() {
                export_postgres_core_jsonl(&pool, created_at_unix_secs).await
            } else {
                export_postgres_jsonl(&pool, domains, created_at_unix_secs).await
            }
        }
        #[cfg(not(feature = "sqlite"))]
        DatabaseDriver::Sqlite => Err(DataLayerError::InvalidInput(
            "SQLite driver is not enabled for aether-data".to_string(),
        )),
        #[cfg(not(feature = "mysql"))]
        DatabaseDriver::Mysql => Err(DataLayerError::InvalidInput(
            "MySQL driver is not enabled for aether-data".to_string(),
        )),
        #[cfg(not(feature = "postgres"))]
        DatabaseDriver::Postgres => Err(DataLayerError::InvalidInput(
            "PostgreSQL driver is not enabled for aether-data".to_string(),
        )),
    }
}

pub async fn import_database_jsonl(
    database: SqlDatabaseConfig,
    input: &str,
) -> Result<usize, DataLayerError> {
    match database.driver {
        #[cfg(feature = "sqlite")]
        DatabaseDriver::Sqlite => {
            let pool = crate::driver::sqlite::SqlitePoolFactory::new(database)?.connect_lazy()?;
            import_sqlite_jsonl(&pool, input).await
        }
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => {
            let pool = crate::driver::mysql::MysqlPoolFactory::new(database)?.connect_lazy()?;
            import_mysql_jsonl(&pool, input).await
        }
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => {
            let pool =
                crate::driver::postgres::PostgresPoolFactory::new(database.to_postgres_config()?)?
                    .connect_lazy()?;
            import_postgres_jsonl(&pool, input).await
        }
        #[cfg(not(feature = "sqlite"))]
        DatabaseDriver::Sqlite => Err(DataLayerError::InvalidInput(
            "SQLite driver is not enabled for aether-data".to_string(),
        )),
        #[cfg(not(feature = "mysql"))]
        DatabaseDriver::Mysql => Err(DataLayerError::InvalidInput(
            "MySQL driver is not enabled for aether-data".to_string(),
        )),
        #[cfg(not(feature = "postgres"))]
        DatabaseDriver::Postgres => Err(DataLayerError::InvalidInput(
            "PostgreSQL driver is not enabled for aether-data".to_string(),
        )),
    }
}

pub async fn copy_database_records(
    source: SqlDatabaseConfig,
    target: SqlDatabaseConfig,
    domains: Vec<ExportDomain>,
    created_at_unix_secs: u64,
    options: DataCopyOptions,
) -> Result<usize, DataLayerError> {
    #[cfg(all(feature = "postgres", feature = "sqlite"))]
    if domains.is_empty()
        && source.driver == DatabaseDriver::Postgres
        && target.driver == DatabaseDriver::Sqlite
    {
        return copy_postgres_to_sqlite_from_target_schema(source, target, options).await;
    }

    let mut records =
        decode_jsonl(&export_database_jsonl(source, domains, created_at_unix_secs).await?)?;
    if options.omit_request_body_details {
        omit_request_body_details_from_records(&mut records);
    }
    import_database_jsonl(target, &encode_jsonl(&records)?).await
}

fn omit_request_body_details_from_records(records: &mut [DataExportRecord]) {
    for record in records {
        let DataExportRecord::Row {
            domain: ExportDomain::Usage,
            payload,
            ..
        } = record
        else {
            continue;
        };

        if let Some(object) = payload.as_object_mut() {
            for column_name in USAGE_REQUEST_BODY_DETAIL_COLUMNS {
                object.remove(*column_name);
            }
        }
    }
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn copy_postgres_to_sqlite_from_target_schema(
    source: SqlDatabaseConfig,
    mut target: SqlDatabaseConfig,
    options: DataCopyOptions,
) -> Result<usize, DataLayerError> {
    target.pool.min_connections = 1;
    target.pool.max_connections = 1;

    let postgres_pool =
        crate::driver::postgres::PostgresPoolFactory::new(source.to_postgres_config()?)?
            .connect_lazy()?;
    let sqlite_pool = crate::driver::sqlite::SqlitePoolFactory::new(target)?.connect_lazy()?;

    let source_tables = load_postgres_public_table_names(&postgres_pool).await?;
    let target_tables = load_sqlite_copy_table_names(&sqlite_pool).await?;

    ensure_no_nonempty_source_tables_outside_target_schema(
        &postgres_pool,
        &source_tables,
        &target_tables,
        options,
    )
    .await?;

    let mut imported = 0usize;
    sqlx::raw_sql("PRAGMA foreign_keys = OFF")
        .execute(&sqlite_pool)
        .await
        .map_sql_err()?;

    for table_name in target_tables {
        if copy_table_is_lifecycle(&table_name)
            || copy_table_is_sqlite_internal(&table_name)
            || !source_tables.contains(&table_name)
            || (options.omit_request_body_details && copy_table_is_request_body_detail(&table_name))
        {
            continue;
        }

        let table_plan = build_postgres_sqlite_copy_table_plan(
            &postgres_pool,
            &sqlite_pool,
            &table_name,
            options,
        )
        .await?;
        if table_plan.columns.is_empty() {
            continue;
        }
        imported = imported.saturating_add(
            copy_postgres_sqlite_table(&postgres_pool, &sqlite_pool, &table_plan).await?,
        );
    }

    sqlx::raw_sql("PRAGMA foreign_keys = ON")
        .execute(&sqlite_pool)
        .await
        .map_sql_err()?;
    ensure_sqlite_foreign_key_check_passes(&sqlite_pool).await?;
    Ok(imported)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn ensure_no_nonempty_source_tables_outside_target_schema(
    postgres_pool: &crate::driver::postgres::PostgresPool,
    source_tables: &BTreeSet<String>,
    target_tables: &BTreeSet<String>,
    options: DataCopyOptions,
) -> Result<(), DataLayerError> {
    let mut missing = Vec::new();
    for table_name in source_tables {
        if copy_table_is_lifecycle(table_name)
            || (options.omit_request_body_details && copy_table_is_request_body_detail(table_name))
            || target_tables.contains(table_name)
        {
            continue;
        }
        if postgres_public_table_has_rows(postgres_pool, table_name).await? {
            missing.push(table_name.clone());
        }
    }

    if !missing.is_empty() {
        return Err(DataLayerError::InvalidInput(format!(
            "source Postgres has non-empty public tables that do not exist in the target SQLite schema: {}",
            missing.join(", ")
        )));
    }
    Ok(())
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn build_postgres_sqlite_copy_table_plan(
    postgres_pool: &crate::driver::postgres::PostgresPool,
    sqlite_pool: &crate::driver::sqlite::SqlitePool,
    table_name: &str,
    options: DataCopyOptions,
) -> Result<SchemaCopyTable, DataLayerError> {
    let sqlite_columns = load_sqlite_copy_columns(sqlite_pool, table_name).await?;
    let postgres_columns =
        load_postgres_import_columns(postgres_pool, &format!("public.{table_name}")).await?;
    let source_has_rows = postgres_public_table_has_rows(postgres_pool, table_name).await?;
    let mut columns = Vec::new();

    for sqlite_column in sqlite_columns {
        if options.omit_request_body_details
            && table_name == "usage"
            && USAGE_REQUEST_BODY_DETAIL_COLUMNS.contains(&sqlite_column.name.as_str())
        {
            continue;
        }

        if let Some(postgres_column) = postgres_columns.get(&sqlite_column.name) {
            columns.push(SchemaCopyColumn {
                sqlite: sqlite_column,
                postgres: postgres_column.clone(),
            });
            continue;
        }

        if source_has_rows && sqlite_copy_column_is_required(&sqlite_column) {
            return Err(DataLayerError::InvalidInput(format!(
                "target SQLite table '{table_name}' has required column '{}' that does not exist in source Postgres",
                sqlite_column.name
            )));
        }
    }

    if source_has_rows && columns.is_empty() {
        return Err(DataLayerError::InvalidInput(format!(
            "source Postgres table '{table_name}' has rows, but none of its columns exist in target SQLite"
        )));
    }

    Ok(SchemaCopyTable {
        table_name: table_name.to_string(),
        columns,
    })
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn copy_postgres_sqlite_table(
    postgres_pool: &crate::driver::postgres::PostgresPool,
    sqlite_pool: &crate::driver::sqlite::SqlitePool,
    table: &SchemaCopyTable,
) -> Result<usize, DataLayerError> {
    let source_sql = postgres_schema_copy_select_sql(table)?;
    let target_sql = sqlite_schema_copy_insert_sql(table)?;
    let mut rows = sqlx::query(&source_sql).fetch(postgres_pool);
    let mut imported = 0usize;

    while let Some(row) = rows.try_next().await.map_sql_err()? {
        let payload = row.try_get::<Value, _>("payload").map_sql_err()?;
        let object = payload.as_object().ok_or_else(|| {
            DataLayerError::UnexpectedValue(format!(
                "postgres copy row for table '{}' did not produce a JSON object",
                table.table_name
            ))
        })?;
        let mut query = sqlx::query(&target_sql);
        for column in &table.columns {
            let value = object.get(&column.sqlite.name).ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "postgres copy row for table '{}' is missing column '{}'",
                    table.table_name, column.sqlite.name
                ))
            })?;
            query = bind_sqlite_copy_value(query, value, &column.sqlite)?;
        }
        query.execute(sqlite_pool).await.map_sql_err()?;
        imported = imported.saturating_add(1);
    }

    Ok(imported)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn postgres_schema_copy_select_sql(table: &SchemaCopyTable) -> Result<String, DataLayerError> {
    let table_sql = format!(
        "public.{}",
        postgres_quote_identifier(table.table_name.as_str())?
    );
    let mut payload_parts = Vec::new();
    for column in &table.columns {
        if let Some(expr) = postgres_schema_copy_override_expr(column)? {
            payload_parts.push(sql_string_literal(&column.sqlite.name));
            payload_parts.push(expr);
        }
    }
    let payload_sql = if payload_parts.is_empty() {
        "to_jsonb(t)".to_string()
    } else {
        format!(
            "to_jsonb(t) || jsonb_build_object({})",
            payload_parts.join(", ")
        )
    };

    let order_by = table
        .columns
        .iter()
        .filter(|column| column.sqlite.primary_key_position > 0)
        .map(|column| {
            postgres_quote_identifier(&column.sqlite.name).map(|quoted| format!("t.{quoted} ASC"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let order_sql = if order_by.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {}", order_by.join(", "))
    };

    Ok(format!(
        "SELECT {payload_sql} AS payload FROM {table_sql} AS t{order_sql}"
    ))
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn postgres_schema_copy_override_expr(
    column: &SchemaCopyColumn,
) -> Result<Option<String>, DataLayerError> {
    let column_sql = format!("t.{}", postgres_quote_identifier(&column.sqlite.name)?);
    let affinity = sqlite_copy_affinity(&column.sqlite);

    if affinity == SqliteCopyAffinity::Blob && is_postgres_bytea_column(&column.postgres) {
        return Ok(Some(format!(
            "CASE WHEN {column_sql} IS NULL THEN NULL ELSE encode({column_sql}, 'hex') END"
        )));
    }

    if affinity == SqliteCopyAffinity::Integer && is_postgres_boolean_column(&column.postgres) {
        return Ok(Some(format!(
            "CASE WHEN {column_sql} IS NULL THEN NULL WHEN {column_sql} THEN 1 ELSE 0 END"
        )));
    }

    if affinity == SqliteCopyAffinity::Integer
        && (is_postgres_timestamp_column(&column.postgres)
            || is_postgres_date_column(&column.postgres))
    {
        let timestamp_sql = if is_postgres_date_column(&column.postgres) {
            format!("{column_sql}::timestamp")
        } else {
            column_sql.clone()
        };
        let multiplier = if sqlite_copy_column_stores_unix_millis(&column.sqlite.name) {
            " * 1000"
        } else {
            ""
        };
        return Ok(Some(format!(
            "CASE WHEN {column_sql} IS NULL THEN NULL ELSE FLOOR(EXTRACT(EPOCH FROM {timestamp_sql}){multiplier})::bigint END"
        )));
    }

    Ok(None)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn sqlite_schema_copy_insert_sql(table: &SchemaCopyTable) -> Result<String, DataLayerError> {
    let table_sql = sqlite_quote_identifier(&table.table_name)?;
    let column_sql = table
        .columns
        .iter()
        .map(|column| sqlite_quote_identifier(&column.sqlite.name))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let placeholder_sql = vec!["?"; table.columns.len()].join(", ");
    Ok(format!(
        "INSERT OR REPLACE INTO {table_sql} ({column_sql}) VALUES ({placeholder_sql})"
    ))
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn load_postgres_public_table_names(
    pool: &crate::driver::postgres::PostgresPool,
) -> Result<BTreeSet<String>, DataLayerError> {
    let rows = sqlx::query(
        r#"
SELECT table_name
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_type = 'BASE TABLE'
ORDER BY table_name
"#,
    )
    .fetch_all(pool)
    .await
    .map_sql_err()?;

    let mut tables = BTreeSet::new();
    for row in rows {
        tables.insert(row.try_get::<String, _>("table_name").map_sql_err()?);
    }
    Ok(tables)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn load_sqlite_copy_table_names(
    pool: &crate::driver::sqlite::SqlitePool,
) -> Result<BTreeSet<String>, DataLayerError> {
    let rows = sqlx::query(
        r#"
SELECT name
FROM sqlite_schema
WHERE type = 'table'
  AND name NOT LIKE 'sqlite_%'
ORDER BY name
"#,
    )
    .fetch_all(pool)
    .await
    .map_sql_err()?;

    let mut tables = BTreeSet::new();
    for row in rows {
        let table_name = row.try_get::<String, _>("name").map_sql_err()?;
        if !copy_table_is_lifecycle(&table_name) && !copy_table_is_sqlite_internal(&table_name) {
            tables.insert(table_name);
        }
    }
    Ok(tables)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn load_sqlite_copy_columns(
    pool: &crate::driver::sqlite::SqlitePool,
    table_name: &str,
) -> Result<Vec<SqliteCopyColumn>, DataLayerError> {
    let table_sql = sqlite_quote_identifier(table_name)?;
    let rows = sqlx::query(&format!("PRAGMA table_info({table_sql})"))
        .fetch_all(pool)
        .await
        .map_sql_err()?;

    let mut columns = Vec::new();
    for row in rows {
        columns.push(SqliteCopyColumn {
            name: row.try_get::<String, _>("name").map_sql_err()?,
            declared_type: row
                .try_get::<Option<String>, _>("type")
                .map_sql_err()?
                .unwrap_or_default(),
            not_null: row.try_get::<i64, _>("notnull").map_sql_err()? != 0,
            has_default: row
                .try_get::<Option<String>, _>("dflt_value")
                .map_sql_err()?
                .is_some(),
            primary_key_position: row.try_get::<i64, _>("pk").map_sql_err()?,
        });
    }

    if columns.is_empty() {
        return Err(DataLayerError::UnexpectedValue(format!(
            "target SQLite table '{table_name}' has no visible columns"
        )));
    }
    Ok(columns)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn postgres_public_table_has_rows(
    pool: &crate::driver::postgres::PostgresPool,
    table_name: &str,
) -> Result<bool, DataLayerError> {
    let table_sql = format!("public.{}", postgres_quote_identifier(table_name)?);
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS (SELECT 1 FROM {table_sql} LIMIT 1)"
    ))
    .fetch_one(pool)
    .await
    .map_sql_err()
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
async fn ensure_sqlite_foreign_key_check_passes(
    pool: &crate::driver::sqlite::SqlitePool,
) -> Result<(), DataLayerError> {
    let rows = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(pool)
        .await
        .map_sql_err()?;
    if rows.is_empty() {
        return Ok(());
    }

    let mut violations = Vec::new();
    for row in rows.iter().take(10) {
        let table = row
            .try_get::<Option<String>, _>("table")
            .map_sql_err()?
            .unwrap_or_else(|| "<unknown>".to_string());
        let rowid = row.try_get::<Option<i64>, _>("rowid").map_sql_err()?;
        let parent = row
            .try_get::<Option<String>, _>("parent")
            .map_sql_err()?
            .unwrap_or_else(|| "<unknown>".to_string());
        violations.push(format!("{table} rowid={rowid:?} parent={parent}"));
    }
    Err(DataLayerError::InvalidInput(format!(
        "target SQLite foreign key check failed after copy: {}",
        violations.join("; ")
    )))
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn copy_table_is_lifecycle(table_name: &str) -> bool {
    LIFECYCLE_TABLES.contains(&table_name)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn copy_table_is_sqlite_internal(table_name: &str) -> bool {
    table_name.starts_with("sqlite_")
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn copy_table_is_request_body_detail(table_name: &str) -> bool {
    REQUEST_BODY_DETAIL_TABLES.contains(&table_name)
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn sqlite_copy_column_is_required(column: &SqliteCopyColumn) -> bool {
    (column.not_null || column.primary_key_position > 0) && !column.has_default
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn sqlite_copy_column_stores_unix_millis(column_name: &str) -> bool {
    column_name.ends_with("_unix_ms")
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn sqlite_copy_affinity(column: &SqliteCopyColumn) -> SqliteCopyAffinity {
    let declared_type = column.declared_type.to_ascii_uppercase();
    if declared_type.contains("INT") {
        SqliteCopyAffinity::Integer
    } else if declared_type.contains("CHAR")
        || declared_type.contains("CLOB")
        || declared_type.contains("TEXT")
    {
        SqliteCopyAffinity::Text
    } else if declared_type.contains("BLOB") || declared_type.trim().is_empty() {
        SqliteCopyAffinity::Blob
    } else if declared_type.contains("REAL")
        || declared_type.contains("FLOA")
        || declared_type.contains("DOUB")
    {
        SqliteCopyAffinity::Real
    } else {
        SqliteCopyAffinity::Numeric
    }
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn is_postgres_bytea_column(column: &PostgresImportColumn) -> bool {
    column.data_type == "bytea" || column.udt_name == "bytea"
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn is_postgres_date_column(column: &PostgresImportColumn) -> bool {
    column.data_type == "date" || column.udt_name == "date"
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn bind_sqlite_copy_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    value: &'q Value,
    column: &SqliteCopyColumn,
) -> Result<sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>, DataLayerError>
{
    Ok(match sqlite_copy_affinity(column) {
        SqliteCopyAffinity::Integer => match value {
            Value::Null => query.bind(Option::<i64>::None),
            Value::Bool(value) => query.bind(i64::from(*value)),
            Value::Number(number) => {
                let value = number
                    .as_i64()
                    .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))
                    .ok_or_else(|| {
                        DataLayerError::InvalidInput(format!(
                            "sqlite copy column '{}' expected integer, got {number}",
                            column.name
                        ))
                    })?;
                query.bind(value)
            }
            Value::String(value) => query.bind(value.parse::<i64>().map_err(|err| {
                DataLayerError::InvalidInput(format!(
                    "sqlite copy column '{}' expected integer string: {err}",
                    column.name
                ))
            })?),
            Value::Array(_) | Value::Object(_) => {
                return Err(DataLayerError::InvalidInput(format!(
                    "sqlite copy column '{}' expected integer-compatible value",
                    column.name
                )));
            }
        },
        SqliteCopyAffinity::Real => match value {
            Value::Null => query.bind(Option::<f64>::None),
            Value::Number(number) => query.bind(number.as_f64().ok_or_else(|| {
                DataLayerError::InvalidInput(format!(
                    "sqlite copy column '{}' expected finite real value",
                    column.name
                ))
            })?),
            Value::String(value) => query.bind(value.parse::<f64>().map_err(|err| {
                DataLayerError::InvalidInput(format!(
                    "sqlite copy column '{}' expected real string: {err}",
                    column.name
                ))
            })?),
            Value::Bool(value) => query.bind(if *value { 1.0 } else { 0.0 }),
            Value::Array(_) | Value::Object(_) => {
                return Err(DataLayerError::InvalidInput(format!(
                    "sqlite copy column '{}' expected real-compatible value",
                    column.name
                )));
            }
        },
        SqliteCopyAffinity::Blob => match value {
            Value::Null => query.bind(Option::<Vec<u8>>::None),
            Value::String(value) => query.bind(hex_decode(value, &column.name)?),
            Value::Array(values) => {
                let mut bytes = Vec::with_capacity(values.len());
                for value in values {
                    let Some(byte) = value.as_u64().and_then(|value| u8::try_from(value).ok())
                    else {
                        return Err(DataLayerError::InvalidInput(format!(
                            "sqlite copy column '{}' contains non-byte array value",
                            column.name
                        )));
                    };
                    bytes.push(byte);
                }
                query.bind(bytes)
            }
            Value::Bool(_) | Value::Number(_) | Value::Object(_) => {
                return Err(DataLayerError::InvalidInput(format!(
                    "sqlite copy column '{}' expected blob-compatible value",
                    column.name
                )));
            }
        },
        SqliteCopyAffinity::Text | SqliteCopyAffinity::Numeric => {
            bind_sqlite_json_value(query, value)?
        }
    })
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(all(feature = "postgres", feature = "sqlite"))]
fn hex_decode(value: &str, column_name: &str) -> Result<Vec<u8>, DataLayerError> {
    let value = value.trim();
    if !value.len().is_multiple_of(2) {
        return Err(DataLayerError::InvalidInput(format!(
            "sqlite copy column '{column_name}' has odd-length hex data"
        )));
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    for index in (0..value.len()).step_by(2) {
        let byte = u8::from_str_radix(&value[index..index + 2], 16).map_err(|err| {
            DataLayerError::InvalidInput(format!(
                "sqlite copy column '{column_name}' has invalid hex data at byte {}: {err}",
                index / 2
            ))
        })?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn export_order_by(domain: ExportDomain, id_column: &str) -> String {
    if domain == ExportDomain::UserGroupMembers {
        "group_id ASC, user_id ASC".to_string()
    } else {
        format!("{id_column} ASC")
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_quote_identifier(identifier: &str) -> Result<String, DataLayerError> {
    if identifier.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "sqlite import column name cannot be empty".to_string(),
        ));
    }
    if !identifier
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(DataLayerError::InvalidInput(format!(
            "sqlite import column name '{identifier}' contains unsupported characters"
        )));
    }
    Ok(format!(r#""{identifier}""#))
}

#[cfg(feature = "sqlite")]
fn bind_sqlite_json_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    value: &'q Value,
) -> Result<sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>, DataLayerError>
{
    Ok(match value {
        Value::Null => query.bind(Option::<String>::None),
        Value::Bool(value) => query.bind(i64::from(*value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                query.bind(value)
            } else if let Some(value) = value.as_u64() {
                let value = i64::try_from(value).map_err(|_| {
                    DataLayerError::InvalidInput(format!(
                        "sqlite import integer value {value} exceeds i64"
                    ))
                })?;
                query.bind(value)
            } else if let Some(value) = value.as_f64() {
                query.bind(value)
            } else {
                return Err(DataLayerError::InvalidInput(
                    "sqlite import number is not representable".to_string(),
                ));
            }
        }
        Value::String(value) => query.bind(value),
        Value::Array(_) | Value::Object(_) => {
            let value = serde_json::to_string(value)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
            query.bind(value)
        }
    })
}

#[cfg(feature = "postgres")]
pub(super) fn postgres_quote_identifier(identifier: &str) -> Result<String, DataLayerError> {
    if identifier.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "postgres import column name cannot be empty".to_string(),
        ));
    }
    if !identifier
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(DataLayerError::InvalidInput(format!(
            "postgres import column name '{identifier}' contains unsupported characters"
        )));
    }
    Ok(format!(r#""{identifier}""#))
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn filter_import_payload(
    driver_name: &str,
    table_name: &str,
    domain: ExportDomain,
    row: &ExportRow,
    target_columns: &ImportColumnNames,
) -> Result<serde_json::Map<String, Value>, DataLayerError> {
    let object = row.payload.as_object().ok_or_else(|| {
        DataLayerError::InvalidInput(format!(
            "{} export row '{}' payload must be a JSON object",
            domain.as_str(),
            row.id
        ))
    })?;
    if object.is_empty() {
        return Err(DataLayerError::InvalidInput(format!(
            "{} export row '{}' payload cannot be empty",
            domain.as_str(),
            row.id
        )));
    }

    let mut filtered = serde_json::Map::new();
    for (column_name, value) in object {
        if target_columns.contains(column_name) {
            filtered.insert(column_name.clone(), value.clone());
        }
    }

    if filtered.is_empty() {
        return Err(DataLayerError::InvalidInput(format!(
            "{} export row '{}' has no columns supported by {driver_name} table '{table_name}'",
            domain.as_str(),
            row.id
        )));
    }

    Ok(filtered)
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn payload_with_table(payload: Value, table_name: &str) -> Result<Value, DataLayerError> {
    let mut object = payload.as_object().cloned().ok_or_else(|| {
        DataLayerError::UnexpectedValue("export row payload must be a JSON object".to_string())
    })?;
    normalize_billing_payload(table_name, &mut object)?;
    object.insert("__table".to_string(), Value::String(table_name.to_string()));
    Ok(Value::Object(object))
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn normalize_billing_payload(
    table_name: &str,
    object: &mut serde_json::Map<String, Value>,
) -> Result<(), DataLayerError> {
    if table_name != "billing_rules" {
        return Ok(());
    }
    for field_name in ["variables", "dimension_mappings"] {
        let Some(Value::String(raw)) = object.get(field_name) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        let parsed = serde_json::from_str::<Value>(raw).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "billing_rules.{field_name} contains invalid JSON: {err}"
            ))
        })?;
        object.insert(field_name.to_string(), parsed);
    }
    Ok(())
}

fn billing_payload_table(row: &ExportRow) -> Result<(String, Value), DataLayerError> {
    domain_payload_table(row, "billing", None)
}

fn domain_payload_table(
    row: &ExportRow,
    domain_label: &str,
    default_table: Option<&str>,
) -> Result<(String, Value), DataLayerError> {
    let mut object = row.payload.as_object().cloned().ok_or_else(|| {
        DataLayerError::InvalidInput(format!(
            "{domain_label} export row '{}' payload must be a JSON object",
            row.id,
        ))
    })?;
    let table_name = match object.remove("__table") {
        Some(value) => value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "{domain_label} export row '{}' has non-string __table",
                row.id
            ))
        })?,
        None => default_table.map(str::to_string).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "{domain_label} export row '{}' is missing string __table",
                row.id
            ))
        })?,
    };
    Ok((table_name, Value::Object(object)))
}
