use super::*;

pub async fn export_sqlite_core_jsonl(
    pool: &crate::driver::sqlite::SqlitePool,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    export_sqlite_jsonl(pool, sqlite_core_export_domains(), created_at_unix_secs).await
}

pub async fn export_sqlite_jsonl(
    pool: &crate::driver::sqlite::SqlitePool,
    domains: Vec<ExportDomain>,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    let manifest = DataExportManifest::new(
        created_at_unix_secs,
        Some(DatabaseDriver::Sqlite),
        domains.clone(),
    );
    let mut records = vec![DataExportRecord::manifest(manifest)];

    for domain in domains {
        if domain == ExportDomain::Billing {
            export_sqlite_billing_records(pool, &mut records).await?;
            continue;
        }
        if domain == ExportDomain::Wallets {
            export_sqlite_wallet_records(pool, &mut records).await?;
            continue;
        }
        let (table_name, id_column) = sqlite_domain_table(domain)?;
        let order_by = export_order_by(domain, id_column);
        let sql = format!("SELECT * FROM {table_name} ORDER BY {order_by}");
        let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
        for row in rows {
            let id = sqlite_export_row_id(domain, &row, id_column)?;
            records.push(DataExportRecord::row(domain, id, sqlite_row_payload(&row)?));
        }
    }

    encode_jsonl(&records)
}

pub async fn import_sqlite_jsonl(
    pool: &crate::driver::sqlite::SqlitePool,
    input: &str,
) -> Result<usize, DataLayerError> {
    let plan = build_import_plan(input)?;
    import_sqlite_plan(pool, &plan).await
}

pub async fn import_sqlite_plan(
    pool: &crate::driver::sqlite::SqlitePool,
    plan: &DataImportPlan,
) -> Result<usize, DataLayerError> {
    let mut imported = 0usize;
    let mut column_cache = BTreeMap::<String, ImportColumnNames>::new();
    for domain in &plan.manifest.domains {
        if *domain == ExportDomain::Billing {
            for row in plan.rows(*domain) {
                import_sqlite_billing_row(pool, row, &mut column_cache).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        if *domain == ExportDomain::Wallets {
            for row in plan.rows(*domain) {
                import_sqlite_wallet_row(pool, row, &mut column_cache).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        let (table_name, _id_column) = sqlite_domain_table(*domain)?;
        let target_columns =
            sqlite_import_columns_cached(pool, &mut column_cache, table_name).await?;
        for row in plan.rows(*domain) {
            import_sqlite_row(pool, table_name, *domain, row, &target_columns).await?;
            imported = imported.saturating_add(1);
        }
    }
    Ok(imported)
}

fn sqlite_domain_table(
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
            "sqlite wallet export uses multiple tables and must be handled as a domain".to_string(),
        )),
        ExportDomain::Usage => Ok((r#""usage""#, "request_id")),
        ExportDomain::Billing => Err(DataLayerError::InvalidInput(
            "sqlite billing export uses multiple tables and must be handled as a domain"
                .to_string(),
        )),
    }
}

fn sqlite_export_row_id(
    domain: ExportDomain,
    row: &sqlx::sqlite::SqliteRow,
    id_column: &str,
) -> Result<String, DataLayerError> {
    if domain == ExportDomain::UserGroupMembers {
        let group_id = sqlite_required_export_text(row, "group_id", domain)?;
        let user_id = sqlite_required_export_text(row, "user_id", domain)?;
        return Ok(format!("{group_id}:{user_id}"));
    }
    sqlite_required_export_text(row, id_column, domain)
}

fn sqlite_required_export_text(
    row: &sqlx::sqlite::SqliteRow,
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

async fn export_sqlite_billing_records(
    pool: &crate::driver::sqlite::SqlitePool,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for table_name in [
        "billing_rules",
        "dimension_collectors",
        "usage_settlement_snapshots",
    ] {
        let id_column = if table_name == "usage_settlement_snapshots" {
            "request_id"
        } else {
            "id"
        };
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
                payload_with_table(sqlite_row_payload(&row)?, table_name)?,
            ));
        }
    }
    Ok(())
}

async fn export_sqlite_wallet_records(
    pool: &crate::driver::sqlite::SqlitePool,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for (table_name, id_column) in sqlite_wallet_tables() {
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
                payload_with_table(sqlite_row_payload(&row)?, table_name)?,
            ));
        }
    }
    Ok(())
}

async fn import_sqlite_row(
    pool: &crate::driver::sqlite::SqlitePool,
    table_name: &str,
    domain: ExportDomain,
    row: &ExportRow,
    target_columns: &ImportColumnNames,
) -> Result<(), DataLayerError> {
    let object = filter_import_payload("sqlite", table_name, domain, row, target_columns)?;

    let columns = object.keys().map(String::as_str).collect::<Vec<_>>();
    let column_sql = columns
        .iter()
        .map(|column| sqlite_quote_identifier(column))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let placeholder_sql = vec!["?"; columns.len()].join(", ");
    let sql =
        format!("INSERT OR REPLACE INTO {table_name} ({column_sql}) VALUES ({placeholder_sql})");
    let mut query = sqlx::query(&sql);
    for column in columns {
        let value = object
            .get(column)
            .expect("column name came from payload object keys");
        query = bind_sqlite_json_value(query, value)?;
    }
    query.execute(pool).await.map_sql_err()?;
    Ok(())
}

async fn import_sqlite_billing_row(
    pool: &crate::driver::sqlite::SqlitePool,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, ImportColumnNames>,
) -> Result<(), DataLayerError> {
    let (table_name, payload) = billing_payload_table(row)?;
    let table_name = sqlite_billing_table_name(&table_name)?;
    let target_columns = sqlite_import_columns_cached(pool, column_cache, table_name).await?;
    import_sqlite_row(
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

fn sqlite_billing_table_name(table_name: &str) -> Result<&'static str, DataLayerError> {
    match table_name {
        "billing_rules" => Ok("billing_rules"),
        "dimension_collectors" => Ok("dimension_collectors"),
        "usage_settlement_snapshots" => Ok("usage_settlement_snapshots"),
        other => Err(DataLayerError::InvalidInput(format!(
            "unsupported sqlite billing export table '{other}'"
        ))),
    }
}

async fn import_sqlite_wallet_row(
    pool: &crate::driver::sqlite::SqlitePool,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, ImportColumnNames>,
) -> Result<(), DataLayerError> {
    let (table_name, payload) = domain_payload_table(row, "wallet", Some("wallets"))?;
    let table_name = sqlite_wallet_table_name(&table_name)?;
    let target_columns = sqlite_import_columns_cached(pool, column_cache, table_name).await?;
    import_sqlite_row(
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

fn sqlite_wallet_tables() -> &'static [(&'static str, &'static str)] {
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

fn sqlite_wallet_table_name(table_name: &str) -> Result<&'static str, DataLayerError> {
    sqlite_wallet_tables()
        .iter()
        .find(|(candidate, _)| *candidate == table_name)
        .map(|(table, _)| *table)
        .ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "unsupported sqlite wallet export table '{table_name}'"
            ))
        })
}

async fn sqlite_import_columns_cached(
    pool: &crate::driver::sqlite::SqlitePool,
    cache: &mut BTreeMap<String, ImportColumnNames>,
    table_name: &str,
) -> Result<ImportColumnNames, DataLayerError> {
    if let Some(columns) = cache.get(table_name) {
        return Ok(columns.clone());
    }

    let columns = load_sqlite_import_columns(pool, table_name).await?;
    cache.insert(table_name.to_string(), columns.clone());
    Ok(columns)
}

async fn load_sqlite_import_columns(
    pool: &crate::driver::sqlite::SqlitePool,
    table_name: &str,
) -> Result<ImportColumnNames, DataLayerError> {
    let sql = format!("PRAGMA table_info({table_name})");
    let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
    let mut columns = ImportColumnNames::new();
    for row in rows {
        columns.insert(row.try_get::<String, _>("name").map_sql_err()?);
    }

    if columns.is_empty() {
        return Err(DataLayerError::UnexpectedValue(format!(
            "sqlite import target table '{table_name}' has no visible columns"
        )));
    }

    Ok(columns)
}

fn sqlite_row_payload(row: &sqlx::sqlite::SqliteRow) -> Result<Value, DataLayerError> {
    let mut object = serde_json::Map::new();
    for (index, column) in row.columns().iter().enumerate() {
        object.insert(
            column.name().to_string(),
            sqlite_value_to_json(row, index, column.name())?,
        );
    }
    Ok(Value::Object(object))
}

fn sqlite_value_to_json(
    row: &sqlx::sqlite::SqliteRow,
    index: usize,
    column_name: &str,
) -> Result<Value, DataLayerError> {
    let raw = row.try_get_raw(index).map_sql_err()?;
    if raw.is_null() {
        return Ok(Value::Null);
    }

    match raw.type_info().name().to_ascii_uppercase().as_str() {
        "INTEGER" => {
            let value = row.try_get::<i64, _>(index).map_sql_err()?;
            if sqlite_integer_column_is_boolean(column_name) {
                match value {
                    0 => return Ok(Value::Bool(false)),
                    1 => return Ok(Value::Bool(true)),
                    _ => {}
                }
            }
            Ok(Value::from(value))
        }
        "REAL" | "FLOAT" | "DOUBLE" => {
            let value = row.try_get::<f64, _>(index).map_sql_err()?;
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "sqlite export column {} contains non-finite float",
                        index
                    ))
                })
        }
        "TEXT" => Ok(Value::String(
            row.try_get::<String, _>(index).map_sql_err()?,
        )),
        "BLOB" => {
            let bytes = row.try_get::<Vec<u8>, _>(index).map_sql_err()?;
            Ok(Value::Array(bytes.into_iter().map(Value::from).collect()))
        }
        other => Err(DataLayerError::UnexpectedValue(format!(
            "unsupported sqlite export column type '{other}' at index {index}"
        ))),
    }
}

fn sqlite_integer_column_is_boolean(column_name: &str) -> bool {
    column_name.starts_with("is_")
        || column_name.starts_with("has_")
        || column_name.starts_with("supports_")
        || column_name.starts_with("enable_")
        || column_name.starts_with("use_")
        || matches!(
            column_name,
            "announcement_notifications"
                | "auto_delete_on_expiry"
                | "auto_fetch_models"
                | "email_notifications"
                | "email_verified"
                | "format_converted"
                | "keep_priority_on_conversion"
                | "signature_valid"
                | "tunnel_connected"
                | "tunnel_mode"
                | "usage_alerts"
                | "webhook_sent"
        )
}
