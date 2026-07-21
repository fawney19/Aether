use super::*;

pub async fn export_mysql_core_jsonl(
    pool: &crate::driver::mysql::MysqlPool,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    export_mysql_jsonl(pool, mysql_core_export_domains(), created_at_unix_secs).await
}

pub async fn export_mysql_jsonl(
    pool: &crate::driver::mysql::MysqlPool,
    domains: Vec<ExportDomain>,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    let manifest = DataExportManifest::new(
        created_at_unix_secs,
        Some(DatabaseDriver::Mysql),
        domains.clone(),
    );
    let mut records = vec![DataExportRecord::manifest(manifest)];

    for domain in domains {
        if domain == ExportDomain::Billing {
            export_mysql_billing_records(pool, &mut records).await?;
            continue;
        }
        if domain == ExportDomain::Wallets {
            export_mysql_wallet_records(pool, &mut records).await?;
            continue;
        }
        let (table_name, id_column) = mysql_domain_table(domain)?;
        let order_by = export_order_by(domain, id_column);
        let sql = format!("SELECT * FROM {table_name} ORDER BY {order_by}");
        let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
        for row in rows {
            let id = mysql_export_row_id(domain, &row, id_column)?;
            records.push(DataExportRecord::row(domain, id, mysql_row_payload(&row)?));
        }
    }

    encode_jsonl(&records)
}

pub async fn import_mysql_jsonl(
    pool: &crate::driver::mysql::MysqlPool,
    input: &str,
) -> Result<usize, DataLayerError> {
    let plan = build_import_plan(input)?;
    import_mysql_plan(pool, &plan).await
}

pub async fn import_mysql_plan(
    pool: &crate::driver::mysql::MysqlPool,
    plan: &DataImportPlan,
) -> Result<usize, DataLayerError> {
    let mut imported = 0usize;
    let mut column_cache = BTreeMap::<String, ImportColumnNames>::new();
    for domain in &plan.manifest.domains {
        if *domain == ExportDomain::Billing {
            for row in plan.rows(*domain) {
                import_mysql_billing_row(pool, row, &mut column_cache).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        if *domain == ExportDomain::Wallets {
            for row in plan.rows(*domain) {
                import_mysql_wallet_row(pool, row, &mut column_cache).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        let (table_name, _id_column) = mysql_domain_table(*domain)?;
        let target_columns =
            mysql_import_columns_cached(pool, &mut column_cache, table_name).await?;
        for row in plan.rows(*domain) {
            import_mysql_row(pool, table_name, *domain, row, &target_columns).await?;
            imported = imported.saturating_add(1);
        }
    }
    Ok(imported)
}

fn mysql_domain_table(
    domain: ExportDomain,
) -> Result<(&'static str, &'static str), DataLayerError> {
    match domain {
        ExportDomain::Users => Ok(("users", "id")),
        ExportDomain::ApiKeys => Ok(("api_keys", "id")),
        ExportDomain::Providers => Ok(("providers", "id")),
        ExportDomain::ProviderKeys => Ok(("provider_api_keys", "id")),
        ExportDomain::Endpoints => Ok(("provider_endpoints", "id")),
        ExportDomain::Models => Ok(("models", "id")),
        ExportDomain::GlobalModels => Ok(("global_models", "id")),
        ExportDomain::AuthModules => Ok(("auth_modules", "id")),
        ExportDomain::OAuthProviders => Ok(("oauth_providers", "provider_type")),
        ExportDomain::UserOAuthLinks => Ok(("user_oauth_links", "id")),
        ExportDomain::UserGroups => Ok(("user_groups", "id")),
        ExportDomain::UserGroupMembers => Ok(("user_group_members", "group_id")),
        ExportDomain::ProxyNodes => Ok(("proxy_nodes", "id")),
        ExportDomain::SystemConfigs => Ok(("system_configs", "id")),
        ExportDomain::Wallets => Err(DataLayerError::InvalidInput(
            "mysql wallet export uses multiple tables and must be handled as a domain".to_string(),
        )),
        ExportDomain::Usage => Ok(("`usage`", "request_id")),
        ExportDomain::Billing => Err(DataLayerError::InvalidInput(
            "mysql billing export uses multiple tables and must be handled as a domain".to_string(),
        )),
    }
}

fn mysql_export_row_id(
    domain: ExportDomain,
    row: &sqlx::mysql::MySqlRow,
    id_column: &str,
) -> Result<String, DataLayerError> {
    if domain == ExportDomain::UserGroupMembers {
        let group_id = mysql_required_export_text(row, "group_id", domain)?;
        let user_id = mysql_required_export_text(row, "user_id", domain)?;
        return Ok(format!("{group_id}:{user_id}"));
    }
    mysql_required_export_text(row, id_column, domain)
}

fn mysql_required_export_text(
    row: &sqlx::mysql::MySqlRow,
    column: &str,
    domain: ExportDomain,
) -> Result<String, DataLayerError> {
    row.try_get::<Option<String>, _>(column)
        .map_sql_err()?
        .ok_or_else(|| {
            DataLayerError::UnexpectedValue(format!(
                "{} export row has null id column '{}'",
                domain.as_str(),
                column
            ))
        })
}

async fn export_mysql_billing_records(
    pool: &crate::driver::mysql::MysqlPool,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for (table_name, id_column) in [
        ("billing_rules", "id"),
        ("dimension_collectors", "id"),
        ("usage_settlement_snapshots", "request_id"),
    ] {
        let sql = format!("SELECT * FROM {table_name} ORDER BY {id_column} ASC");
        let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
        for row in rows {
            let id = row
                .try_get::<Option<String>, _>(id_column)
                .map_sql_err()?
                .ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "billing export row in table '{table_name}' has null id"
                    ))
                })?;
            records.push(DataExportRecord::row(
                ExportDomain::Billing,
                format!("{table_name}:{id}"),
                payload_with_table(mysql_row_payload(&row)?, table_name)?,
            ));
        }
    }
    Ok(())
}

async fn export_mysql_wallet_records(
    pool: &crate::driver::mysql::MysqlPool,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for (table_name, id_column) in mysql_wallet_tables() {
        let sql = format!("SELECT * FROM {table_name} ORDER BY {id_column} ASC");
        let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
        for row in rows {
            let id = row
                .try_get::<Option<String>, _>(id_column)
                .map_sql_err()?
                .ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "wallet export row in table '{table_name}' has null id"
                    ))
                })?;
            records.push(DataExportRecord::row(
                ExportDomain::Wallets,
                format!("{table_name}:{id}"),
                payload_with_table(mysql_row_payload(&row)?, table_name)?,
            ));
        }
    }
    Ok(())
}

async fn import_mysql_row(
    pool: &crate::driver::mysql::MysqlPool,
    table_name: &str,
    domain: ExportDomain,
    row: &ExportRow,
    target_columns: &ImportColumnNames,
) -> Result<(), DataLayerError> {
    let object = filter_import_payload("mysql", table_name, domain, row, target_columns)?;

    let columns = object.keys().map(String::as_str).collect::<Vec<_>>();
    let column_sql = columns
        .iter()
        .map(|column| mysql_quote_identifier(column))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let placeholder_sql = vec!["?"; columns.len()].join(", ");
    let update_sql = columns
        .iter()
        .map(|column| {
            let quoted = mysql_quote_identifier(column)?;
            Ok(format!("{quoted} = VALUES({quoted})"))
        })
        .collect::<Result<Vec<_>, DataLayerError>>()?
        .join(", ");
    let sql = format!(
        "INSERT INTO {table_name} ({column_sql}) VALUES ({placeholder_sql}) ON DUPLICATE KEY UPDATE {update_sql}"
    );
    let mut query = sqlx::query(&sql);
    for column in columns {
        let value = object
            .get(column)
            .expect("column name came from payload object keys");
        query = bind_mysql_json_value(query, value)?;
    }
    query.execute(pool).await.map_sql_err()?;
    Ok(())
}

async fn import_mysql_billing_row(
    pool: &crate::driver::mysql::MysqlPool,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, ImportColumnNames>,
) -> Result<(), DataLayerError> {
    let (table_name, payload) = billing_payload_table(row)?;
    let table_name = mysql_billing_table_name(&table_name)?;
    let target_columns = mysql_import_columns_cached(pool, column_cache, table_name).await?;
    import_mysql_row(
        pool,
        table_name,
        ExportDomain::Billing,
        &ExportRow {
            id: row.id.clone(),
            payload,
        },
        &target_columns,
    )
    .await
}

fn mysql_billing_table_name(table_name: &str) -> Result<&'static str, DataLayerError> {
    match table_name {
        "billing_rules" => Ok("billing_rules"),
        "dimension_collectors" => Ok("dimension_collectors"),
        "usage_settlement_snapshots" => Ok("usage_settlement_snapshots"),
        other => Err(DataLayerError::InvalidInput(format!(
            "unsupported mysql billing export table '{other}'"
        ))),
    }
}

async fn import_mysql_wallet_row(
    pool: &crate::driver::mysql::MysqlPool,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, ImportColumnNames>,
) -> Result<(), DataLayerError> {
    let (table_name, payload) = domain_payload_table(row, "wallet", Some("wallets"))?;
    let table_name = mysql_wallet_table_name(&table_name)?;
    let target_columns = mysql_import_columns_cached(pool, column_cache, table_name).await?;
    import_mysql_row(
        pool,
        table_name,
        ExportDomain::Wallets,
        &ExportRow {
            id: row.id.clone(),
            payload,
        },
        &target_columns,
    )
    .await
}

fn mysql_wallet_tables() -> &'static [(&'static str, &'static str)] {
    &[
        ("wallets", "id"),
        ("wallet_transactions", "id"),
        ("wallet_daily_usage_ledgers", "id"),
        ("payment_orders", "id"),
        ("payment_callbacks", "id"),
        ("refund_requests", "id"),
        ("redeem_code_batches", "id"),
        ("redeem_codes", "id"),
    ]
}

fn mysql_wallet_table_name(table_name: &str) -> Result<&'static str, DataLayerError> {
    mysql_wallet_tables()
        .iter()
        .find(|(candidate, _)| *candidate == table_name)
        .map(|(table, _)| *table)
        .ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "unsupported mysql wallet export table '{table_name}'"
            ))
        })
}

async fn mysql_import_columns_cached(
    pool: &crate::driver::mysql::MysqlPool,
    cache: &mut BTreeMap<String, ImportColumnNames>,
    table_name: &str,
) -> Result<ImportColumnNames, DataLayerError> {
    if let Some(columns) = cache.get(table_name) {
        return Ok(columns.clone());
    }

    let columns = load_mysql_import_columns(pool, table_name).await?;
    cache.insert(table_name.to_string(), columns.clone());
    Ok(columns)
}

async fn load_mysql_import_columns(
    pool: &crate::driver::mysql::MysqlPool,
    table_name: &str,
) -> Result<ImportColumnNames, DataLayerError> {
    let relation_name = table_name.trim_matches('`');
    let rows = sqlx::query(
        r#"
SELECT COLUMN_NAME AS column_name
FROM information_schema.columns
WHERE table_schema = DATABASE()
  AND table_name = ?
"#,
    )
    .bind(relation_name)
    .fetch_all(pool)
    .await
    .map_sql_err()?;

    let mut columns = ImportColumnNames::new();
    for row in rows {
        columns.insert(row.try_get::<String, _>("column_name").map_sql_err()?);
    }

    if columns.is_empty() {
        return Err(DataLayerError::UnexpectedValue(format!(
            "mysql import target table '{table_name}' has no visible columns"
        )));
    }

    Ok(columns)
}

fn mysql_quote_identifier(identifier: &str) -> Result<String, DataLayerError> {
    if identifier.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "mysql import column name cannot be empty".to_string(),
        ));
    }
    if !identifier
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(DataLayerError::InvalidInput(format!(
            "mysql import column name '{identifier}' contains unsupported characters"
        )));
    }
    Ok(format!("`{identifier}`"))
}

fn bind_mysql_json_value<'q>(
    query: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    value: &'q Value,
) -> Result<sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>, DataLayerError> {
    Ok(match value {
        Value::Null => query.bind(Option::<String>::None),
        Value::Bool(value) => query.bind(i64::from(*value)),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                query.bind(value)
            } else if let Some(value) = value.as_u64() {
                let value = i64::try_from(value).map_err(|_| {
                    DataLayerError::InvalidInput(format!(
                        "mysql import integer value {value} exceeds i64"
                    ))
                })?;
                query.bind(value)
            } else if let Some(value) = value.as_f64() {
                query.bind(value)
            } else {
                return Err(DataLayerError::InvalidInput(
                    "mysql import number is not representable".to_string(),
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

fn mysql_row_payload(row: &sqlx::mysql::MySqlRow) -> Result<Value, DataLayerError> {
    let mut object = serde_json::Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        object.insert(column.name().to_string(), mysql_value_to_json(row, index)?);
    }
    Ok(Value::Object(object))
}

fn mysql_value_to_json(row: &sqlx::mysql::MySqlRow, index: usize) -> Result<Value, DataLayerError> {
    let raw = row.try_get_raw(index).map_sql_err()?;
    if raw.is_null() {
        return Ok(Value::Null);
    }

    match raw.type_info().name().to_ascii_uppercase().as_str() {
        "BOOL" | "BOOLEAN" => Ok(Value::Bool(row.try_get::<bool, _>(index).map_sql_err()?)),
        "TINYINT" | "TINY" | "SMALLINT" | "SHORT" | "MEDIUMINT" | "INT24" | "INT" | "INTEGER"
        | "LONG" | "BIGINT" | "LONGLONG" | "YEAR" => {
            Ok(Value::from(row.try_get::<i64, _>(index).map_sql_err()?))
        }
        "FLOAT" | "DOUBLE" => {
            let value = row.try_get::<f64, _>(index).map_sql_err()?;
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "mysql export column {} contains non-finite float",
                        index
                    ))
                })
        }
        "DECIMAL" | "NEWDECIMAL" => Ok(Value::String(
            row.try_get::<String, _>(index).map_sql_err()?,
        )),
        "VARCHAR" | "VAR_STRING" | "STRING" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT"
        | "JSON" | "ENUM" | "SET" | "DATE" | "DATETIME" | "TIMESTAMP" | "TIME" => Ok(
            Value::String(row.try_get::<String, _>(index).map_sql_err()?),
        ),
        "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" | "BIT" | "GEOMETRY" => {
            let bytes = row.try_get::<Vec<u8>, _>(index).map_sql_err()?;
            Ok(Value::Array(bytes.into_iter().map(Value::from).collect()))
        }
        other => Err(DataLayerError::UnexpectedValue(format!(
            "unsupported mysql export column type '{other}' at index {index}"
        ))),
    }
}
