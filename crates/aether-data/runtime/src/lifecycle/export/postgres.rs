use super::*;

pub async fn export_postgres_core_jsonl(
    pool: &crate::driver::postgres::PostgresPool,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    export_postgres_jsonl(pool, postgres_core_export_domains(), created_at_unix_secs).await
}

pub async fn export_postgres_jsonl(
    pool: &crate::driver::postgres::PostgresPool,
    domains: Vec<ExportDomain>,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    let manifest = DataExportManifest::new(
        created_at_unix_secs,
        Some(DatabaseDriver::Postgres),
        domains.clone(),
    );
    let mut records = vec![DataExportRecord::manifest(manifest)];

    for domain in domains {
        if domain == ExportDomain::Billing {
            export_postgres_billing_records(pool, &mut records).await?;
            continue;
        }
        if domain == ExportDomain::Wallets {
            export_postgres_wallet_records(pool, &mut records).await?;
            continue;
        }
        let (table_name, id_column) = postgres_domain_table(domain)?;
        let export_id_sql = postgres_export_id_sql(domain, id_column);
        let order_by = export_order_by(domain, id_column);
        let sql = format!(
            "SELECT {export_id_sql} AS export_id, to_jsonb(t) AS payload FROM {table_name} AS t ORDER BY {order_by}"
        );
        let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
        for row in rows {
            let id = row.try_get::<String, _>("export_id").map_sql_err()?;
            let payload = row.try_get::<Value, _>("payload").map_sql_err()?;
            records.push(DataExportRecord::row(domain, id, payload));
        }
    }

    encode_jsonl(&records)
}

pub async fn import_postgres_jsonl(
    pool: &crate::driver::postgres::PostgresPool,
    input: &str,
) -> Result<usize, DataLayerError> {
    let plan = build_import_plan(input)?;
    import_postgres_plan(pool, &plan).await
}

pub async fn import_postgres_plan(
    pool: &crate::driver::postgres::PostgresPool,
    plan: &DataImportPlan,
) -> Result<usize, DataLayerError> {
    let mut imported = 0usize;
    let mut column_cache = BTreeMap::<String, PostgresImportColumns>::new();
    for domain in &plan.manifest.domains {
        if *domain == ExportDomain::Billing {
            for row in plan.rows(*domain) {
                import_postgres_billing_row(pool, row, &mut column_cache).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        if *domain == ExportDomain::Wallets {
            for row in plan.rows(*domain) {
                import_postgres_wallet_row(pool, row, &mut column_cache).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        let (table_name, id_column) = postgres_domain_table(*domain)?;
        let conflict_columns = postgres_conflict_columns(*domain, id_column);
        let rows = plan.rows(*domain);
        if rows.is_empty() {
            continue;
        }
        let target_columns =
            postgres_import_columns_cached(pool, &mut column_cache, table_name).await?;
        for row in rows {
            import_postgres_row(
                pool,
                table_name,
                &conflict_columns,
                *domain,
                row,
                &target_columns,
            )
            .await?;
            imported = imported.saturating_add(1);
        }
    }
    Ok(imported)
}

fn postgres_domain_table(
    domain: ExportDomain,
) -> Result<(&'static str, &'static str), DataLayerError> {
    match domain {
        ExportDomain::Users => Ok(("public.users", "id")),
        ExportDomain::ApiKeys => Ok(("public.api_keys", "id")),
        ExportDomain::Providers => Ok(("public.providers", "id")),
        ExportDomain::ProviderKeys => Ok(("public.provider_api_keys", "id")),
        ExportDomain::Endpoints => Ok(("public.provider_endpoints", "id")),
        ExportDomain::Models => Ok(("public.models", "id")),
        ExportDomain::GlobalModels => Ok(("public.global_models", "id")),
        ExportDomain::AuthModules => Ok(("public.auth_modules", "id")),
        ExportDomain::OAuthProviders => Ok(("public.oauth_providers", "provider_type")),
        ExportDomain::UserOAuthLinks => Ok(("public.user_oauth_links", "id")),
        ExportDomain::UserGroups => Ok(("public.user_groups", "id")),
        ExportDomain::UserGroupMembers => Ok(("public.user_group_members", "group_id")),
        ExportDomain::ProxyNodes => Ok(("public.proxy_nodes", "id")),
        ExportDomain::SystemConfigs => Ok(("public.system_configs", "id")),
        ExportDomain::Wallets => Err(DataLayerError::InvalidInput(
            "postgres wallet export uses multiple tables and must be handled as a domain"
                .to_string(),
        )),
        ExportDomain::Usage => Ok(("public.usage", "request_id")),
        ExportDomain::Billing => Err(DataLayerError::InvalidInput(
            "postgres billing export uses multiple tables and must be handled as a domain"
                .to_string(),
        )),
    }
}

fn postgres_export_id_sql(domain: ExportDomain, id_column: &str) -> String {
    if domain == ExportDomain::UserGroupMembers {
        "group_id::text || ':' || user_id::text".to_string()
    } else {
        format!("{id_column}::text")
    }
}

fn postgres_conflict_columns(domain: ExportDomain, id_column: &str) -> Vec<&str> {
    if domain == ExportDomain::UserGroupMembers {
        vec!["group_id", "user_id"]
    } else {
        vec![id_column]
    }
}

async fn postgres_import_columns_cached(
    pool: &crate::driver::postgres::PostgresPool,
    cache: &mut BTreeMap<String, PostgresImportColumns>,
    table_name: &str,
) -> Result<PostgresImportColumns, DataLayerError> {
    if let Some(columns) = cache.get(table_name) {
        return Ok(columns.clone());
    }

    let columns = load_postgres_import_columns(pool, table_name).await?;
    cache.insert(table_name.to_string(), columns.clone());
    Ok(columns)
}

pub(super) async fn load_postgres_import_columns(
    pool: &crate::driver::postgres::PostgresPool,
    table_name: &str,
) -> Result<PostgresImportColumns, DataLayerError> {
    let (schema_name, relation_name) = postgres_table_parts(table_name)?;
    let rows = sqlx::query(
        r#"
SELECT column_name, data_type, udt_name, is_nullable, column_default IS NOT NULL AS has_default
FROM information_schema.columns
WHERE table_schema = $1
  AND table_name = $2
"#,
    )
    .bind(schema_name)
    .bind(relation_name)
    .fetch_all(pool)
    .await
    .map_sql_err()?;

    let mut columns = PostgresImportColumns::new();
    for row in rows {
        let column_name = row.try_get::<String, _>("column_name").map_sql_err()?;
        let data_type = row
            .try_get::<String, _>("data_type")
            .map_sql_err()?
            .to_ascii_lowercase();
        let udt_name = row
            .try_get::<String, _>("udt_name")
            .map_sql_err()?
            .to_ascii_lowercase();
        let is_nullable = row.try_get::<String, _>("is_nullable").map_sql_err()? == "YES";
        let has_default = row.try_get::<bool, _>("has_default").map_sql_err()?;
        columns.insert(
            column_name,
            PostgresImportColumn {
                data_type,
                udt_name,
                is_nullable,
                has_default,
            },
        );
    }

    if columns.is_empty() {
        return Err(DataLayerError::UnexpectedValue(format!(
            "postgres import target table '{table_name}' has no visible columns"
        )));
    }

    Ok(columns)
}

fn postgres_table_parts(table_name: &str) -> Result<(&str, &str), DataLayerError> {
    let Some((schema_name, relation_name)) = table_name.split_once('.') else {
        return Err(DataLayerError::InvalidInput(format!(
            "postgres import target table '{table_name}' must include a schema"
        )));
    };
    Ok((
        schema_name.trim_matches('"'),
        relation_name.trim_matches('"'),
    ))
}

async fn export_postgres_billing_records(
    pool: &crate::driver::postgres::PostgresPool,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for (table_name, export_table, id_column) in [
        ("public.billing_rules", "billing_rules", "id"),
        ("public.dimension_collectors", "dimension_collectors", "id"),
        (
            "public.usage_settlement_snapshots",
            "usage_settlement_snapshots",
            "request_id",
        ),
    ] {
        let sql = format!(
            "SELECT {id_column}::text AS export_id, to_jsonb(t) || jsonb_build_object('__table', '{export_table}') AS payload FROM {table_name} AS t ORDER BY {id_column} ASC"
        );
        let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
        for row in rows {
            let id = row.try_get::<String, _>("export_id").map_sql_err()?;
            let payload = row.try_get::<Value, _>("payload").map_sql_err()?;
            records.push(DataExportRecord::row(
                ExportDomain::Billing,
                format!("{export_table}:{id}"),
                payload,
            ));
        }
    }
    Ok(())
}

async fn export_postgres_wallet_records(
    pool: &crate::driver::postgres::PostgresPool,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for (table_name, export_table, id_column) in postgres_wallet_tables() {
        let sql = format!(
            "SELECT {id_column}::text AS export_id, to_jsonb(t) || jsonb_build_object('__table', '{export_table}') AS payload FROM {table_name} AS t ORDER BY {id_column} ASC"
        );
        let rows = sqlx::query(&sql).fetch_all(pool).await.map_sql_err()?;
        for row in rows {
            let id = row.try_get::<String, _>("export_id").map_sql_err()?;
            let payload = row.try_get::<Value, _>("payload").map_sql_err()?;
            records.push(DataExportRecord::row(
                ExportDomain::Wallets,
                format!("{export_table}:{id}"),
                payload,
            ));
        }
    }
    Ok(())
}

async fn import_postgres_row(
    pool: &crate::driver::postgres::PostgresPool,
    table_name: &str,
    conflict_columns: &[&str],
    domain: ExportDomain,
    row: &ExportRow,
    target_columns: &PostgresImportColumns,
) -> Result<(), DataLayerError> {
    let object = normalize_postgres_import_payload(table_name, domain, row, target_columns)?;

    let columns = object.keys().map(String::as_str).collect::<Vec<_>>();
    let column_sql = columns
        .iter()
        .map(|column| postgres_quote_identifier(column))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let update_sql = columns
        .iter()
        .filter(|column| !conflict_columns.contains(column))
        .map(|column| {
            let quoted = postgres_quote_identifier(column)?;
            Ok(format!("{quoted} = EXCLUDED.{quoted}"))
        })
        .collect::<Result<Vec<_>, DataLayerError>>()?
        .join(", ");
    let conflict_target_sql = conflict_columns
        .iter()
        .map(|column| postgres_quote_identifier(column))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let conflict_sql = if update_sql.is_empty() {
        format!("ON CONFLICT ({conflict_target_sql}) DO NOTHING")
    } else {
        format!("ON CONFLICT ({conflict_target_sql}) DO UPDATE SET {update_sql}")
    };
    let sql = format!(
        "INSERT INTO {table_name} ({column_sql}) SELECT {column_sql} FROM jsonb_populate_record(NULL::{table_name}, $1::jsonb) {conflict_sql}"
    );
    let payload = Value::Object(object);

    sqlx::query(&sql)
        .bind(&payload)
        .execute(pool)
        .await
        .map_sql_err()?;
    Ok(())
}

pub(super) fn normalize_postgres_import_payload(
    table_name: &str,
    domain: ExportDomain,
    row: &ExportRow,
    target_columns: &PostgresImportColumns,
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

    let mut normalized = serde_json::Map::new();
    for (column_name, value) in object {
        if let Some(target_column) = target_columns.get(column_name) {
            if value.is_null() && !target_column.is_nullable && target_column.has_default {
                continue;
            }
            normalized.insert(
                column_name.clone(),
                normalize_postgres_import_value(column_name, target_column, value)?,
            );
            continue;
        }
        if value.is_null() {
            continue;
        }
        return Err(DataLayerError::InvalidInput(format!(
            "{} export row '{}' contains column '{}' that does not exist in postgres table '{}'",
            domain.as_str(),
            row.id,
            column_name,
            table_name
        )));
    }

    if normalized.is_empty() {
        return Err(DataLayerError::InvalidInput(format!(
            "{} export row '{}' has no columns supported by postgres table '{}'",
            domain.as_str(),
            row.id,
            table_name
        )));
    }

    Ok(normalized)
}

fn normalize_postgres_import_value(
    column_name: &str,
    target_column: &PostgresImportColumn,
    value: &Value,
) -> Result<Value, DataLayerError> {
    if value.is_null() {
        return Ok(Value::Null);
    }

    if is_postgres_boolean_column(target_column) {
        return normalize_postgres_boolean_value(column_name, value);
    }
    if is_postgres_timestamp_column(target_column) {
        return normalize_postgres_timestamp_value(column_name, value);
    }
    if is_postgres_json_column(target_column) {
        return normalize_postgres_json_value(value);
    }

    Ok(value.clone())
}

pub(super) fn is_postgres_boolean_column(target_column: &PostgresImportColumn) -> bool {
    target_column.data_type == "boolean" || target_column.udt_name == "bool"
}

pub(super) fn is_postgres_timestamp_column(target_column: &PostgresImportColumn) -> bool {
    matches!(
        target_column.data_type.as_str(),
        "timestamp with time zone" | "timestamp without time zone"
    ) || matches!(target_column.udt_name.as_str(), "timestamptz" | "timestamp")
}

fn is_postgres_json_column(target_column: &PostgresImportColumn) -> bool {
    matches!(target_column.data_type.as_str(), "json" | "jsonb")
        || matches!(target_column.udt_name.as_str(), "json" | "jsonb")
}

fn normalize_postgres_boolean_value(
    column_name: &str,
    value: &Value,
) -> Result<Value, DataLayerError> {
    match value {
        Value::Bool(_) => Ok(value.clone()),
        Value::Number(number) => {
            let Some(value) = number
                .as_i64()
                .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))
            else {
                return Err(DataLayerError::InvalidInput(format!(
                    "postgres boolean import column '{column_name}' has non-integer value {number}"
                )));
            };
            match value {
                0 => Ok(Value::Bool(false)),
                1 => Ok(Value::Bool(true)),
                other => Err(DataLayerError::InvalidInput(format!(
                    "postgres boolean import column '{column_name}' has unsupported integer value {other}"
                ))),
            }
        }
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "0" | "false" => Ok(Value::Bool(false)),
            "1" | "true" => Ok(Value::Bool(true)),
            _ => Ok(Value::String(value.clone())),
        },
        _ => Ok(value.clone()),
    }
}

fn normalize_postgres_timestamp_value(
    column_name: &str,
    value: &Value,
) -> Result<Value, DataLayerError> {
    let Value::Number(number) = value else {
        return Ok(value.clone());
    };
    let Some(timestamp) = number
        .as_i64()
        .or_else(|| number.as_u64().and_then(|value| i64::try_from(value).ok()))
    else {
        return Err(DataLayerError::InvalidInput(format!(
            "postgres timestamp import column '{column_name}' has non-integer value {number}"
        )));
    };

    let datetime = if column_name.ends_with("_unix_ms")
        || timestamp >= 100_000_000_000
        || timestamp <= -100_000_000_000
    {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp)
    } else {
        chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0)
    }
    .ok_or_else(|| {
        DataLayerError::InvalidInput(format!(
            "postgres timestamp import column '{column_name}' has out-of-range unix value {timestamp}"
        ))
    })?;

    Ok(Value::String(datetime.to_rfc3339()))
}

fn normalize_postgres_json_value(value: &Value) -> Result<Value, DataLayerError> {
    let Value::String(raw) = value else {
        return Ok(value.clone());
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(value.clone());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(parsed) => Ok(parsed),
        Err(_) => Ok(value.clone()),
    }
}

async fn import_postgres_billing_row(
    pool: &crate::driver::postgres::PostgresPool,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, PostgresImportColumns>,
) -> Result<(), DataLayerError> {
    let (export_table_name, payload) = billing_payload_table(row)?;
    let table_name = postgres_billing_table_name(&export_table_name)?;
    let target_columns = postgres_import_columns_cached(pool, column_cache, table_name).await?;
    import_postgres_row(
        pool,
        table_name,
        &["id"],
        ExportDomain::Billing,
        &ExportRow {
            id: row.id.clone(),
            payload,
        },
        &target_columns,
    )
    .await
}

fn postgres_billing_table_name(table_name: &str) -> Result<&'static str, DataLayerError> {
    match table_name {
        "billing_rules" => Ok("public.billing_rules"),
        "dimension_collectors" => Ok("public.dimension_collectors"),
        "usage_settlement_snapshots" => Ok("public.usage_settlement_snapshots"),
        other => Err(DataLayerError::InvalidInput(format!(
            "unsupported postgres billing export table '{other}'"
        ))),
    }
}

async fn import_postgres_wallet_row(
    pool: &crate::driver::postgres::PostgresPool,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, PostgresImportColumns>,
) -> Result<(), DataLayerError> {
    let (export_table_name, payload) = domain_payload_table(row, "wallet", Some("wallets"))?;
    let (table_name, id_column) = postgres_wallet_table_name(&export_table_name)?;
    let target_columns = postgres_import_columns_cached(pool, column_cache, table_name).await?;
    import_postgres_row(
        pool,
        table_name,
        &[id_column],
        ExportDomain::Wallets,
        &ExportRow {
            id: row.id.clone(),
            payload,
        },
        &target_columns,
    )
    .await
}

fn postgres_wallet_tables() -> &'static [(&'static str, &'static str, &'static str)] {
    &[
        ("public.wallets", "wallets", "id"),
        ("public.wallet_transactions", "wallet_transactions", "id"),
        (
            "public.wallet_daily_usage_ledgers",
            "wallet_daily_usage_ledgers",
            "id",
        ),
        ("public.payment_orders", "payment_orders", "id"),
        ("public.payment_callbacks", "payment_callbacks", "id"),
        ("public.refund_requests", "refund_requests", "id"),
        ("public.redeem_code_batches", "redeem_code_batches", "id"),
        ("public.redeem_codes", "redeem_codes", "id"),
    ]
}

fn postgres_wallet_table_name(
    table_name: &str,
) -> Result<(&'static str, &'static str), DataLayerError> {
    postgres_wallet_tables()
        .iter()
        .find(|(_, export_table, _)| *export_table == table_name)
        .map(|(table, _, id_column)| (*table, *id_column))
        .ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "unsupported postgres wallet export table '{table_name}'"
            ))
        })
}
