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
    let mut tx = pool.begin().await.map_sql_err()?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
    let manifest = DataExportManifest::new(
        created_at_unix_secs,
        Some(DatabaseDriver::Postgres),
        domains.clone(),
    );
    let mut records = vec![DataExportRecord::manifest(manifest)];

    for domain in domains {
        if domain == ExportDomain::Auxiliary {
            export_postgres_auxiliary_records(&mut tx, &mut records).await?;
            continue;
        }
        if domain == ExportDomain::Billing {
            export_postgres_billing_records(&mut tx, &mut records).await?;
            continue;
        }
        if domain == ExportDomain::Wallets {
            export_postgres_wallet_records(&mut tx, &mut records).await?;
            continue;
        }
        let (table_name, id_column) = postgres_domain_table(domain)?;
        let export_id_sql = postgres_export_id_sql(domain, id_column);
        let order_by = export_order_by(domain, id_column);
        let sql = format!(
            "SELECT {export_id_sql} AS export_id, to_jsonb(t) AS payload FROM {table_name} AS t ORDER BY {order_by}"
        );
        let rows = sqlx::query(&sql).fetch_all(&mut *tx).await.map_sql_err()?;
        for row in rows {
            let id = row.try_get::<String, _>("export_id").map_sql_err()?;
            let payload = row.try_get::<Value, _>("payload").map_sql_err()?;
            records.push(DataExportRecord::row(domain, id, payload));
        }
    }

    tx.commit().await.map_sql_err()?;
    encode_jsonl(&records)
}

pub async fn import_postgres_jsonl(
    pool: &crate::driver::postgres::PostgresPool,
    input: &str,
) -> Result<usize, DataLayerError> {
    import_postgres_jsonl_with_options(pool, input, DataImportOptions::default()).await
}

pub(super) async fn import_postgres_jsonl_with_options(
    pool: &crate::driver::postgres::PostgresPool,
    input: &str,
    options: DataImportOptions,
) -> Result<usize, DataLayerError> {
    let plan = build_import_plan(input)?;
    import_postgres_plan_with_options(pool, &plan, options).await
}

pub async fn import_postgres_plan(
    pool: &crate::driver::postgres::PostgresPool,
    plan: &DataImportPlan,
) -> Result<usize, DataLayerError> {
    import_postgres_plan_with_options(pool, plan, DataImportOptions::default()).await
}

async fn import_postgres_plan_with_options(
    pool: &crate::driver::postgres::PostgresPool,
    plan: &DataImportPlan,
    options: DataImportOptions,
) -> Result<usize, DataLayerError> {
    let identity_scope = IdentityImportScope::from_plan(plan)?;
    let mut tx = pool.begin().await.map_sql_err()?;
    let identity_state = capture_postgres_identity_import_state(&mut tx, &identity_scope).await?;
    let mut imported = 0usize;
    let mut column_cache = BTreeMap::<String, PostgresImportColumns>::new();
    for domain in &plan.manifest.domains {
        if *domain == ExportDomain::Auxiliary {
            for row in plan.rows(*domain) {
                import_postgres_auxiliary_row(&mut tx, row, &mut column_cache, options).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        if *domain == ExportDomain::Billing {
            for row in plan.rows(*domain) {
                import_postgres_billing_row(&mut tx, row, &mut column_cache).await?;
                imported = imported.saturating_add(1);
            }
            continue;
        }
        if *domain == ExportDomain::Wallets {
            for row in plan.rows(*domain) {
                import_postgres_wallet_row(&mut tx, row, &mut column_cache).await?;
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
            postgres_import_columns_cached(&mut tx, &mut column_cache, table_name).await?;
        for row in rows {
            import_postgres_row(
                &mut tx,
                table_name,
                &conflict_columns,
                *domain,
                row,
                &target_columns,
                options,
            )
            .await?;
            imported = imported.saturating_add(1);
        }
    }
    enforce_postgres_identity_import_invariants(&mut tx, &identity_scope, identity_state).await?;
    if !plan.rows(ExportDomain::Auxiliary).is_empty() {
        reset_postgres_auxiliary_sequences(&mut tx).await?;
    }
    tx.commit().await.map_sql_err()?;
    Ok(imported)
}

async fn capture_postgres_identity_import_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &IdentityImportScope,
) -> Result<IdentityImportState, DataLayerError> {
    let mut affected_user_ids = if scope.finalizes_oauth_links {
        scope.user_ids.iter().cloned().collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    for link_id in &scope.oauth_link_ids {
        if let Some(user_id) = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM public.user_oauth_links WHERE id = $1",
        )
        .bind(link_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()?
        {
            affected_user_ids.insert(user_id);
        }
    }
    for provider_type in &scope.oauth_provider_types {
        let user_ids = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM public.user_oauth_links WHERE provider_type = $1",
        )
        .bind(provider_type)
        .fetch_all(&mut **tx)
        .await
        .map_sql_err()?;
        affected_user_ids.extend(user_ids);
    }
    Ok(IdentityImportState { affected_user_ids })
}

async fn enforce_postgres_identity_import_invariants(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    scope: &IdentityImportScope,
    mut state: IdentityImportState,
) -> Result<(), DataLayerError> {
    for user_id in &scope.user_ids {
        let auth_source = sqlx::query_scalar::<_, String>(
            "SELECT auth_source::text FROM public.users WHERE id = $1 LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()?
        .ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "imported users row '{user_id}' did not produce a user record"
            ))
        })?;
        if !matches!(auth_source.as_str(), "local" | "ldap" | "oauth") {
            return Err(DataLayerError::InvalidInput(format!(
                "imported user '{user_id}' has unsupported auth_source '{auth_source}'"
            )));
        }
        if auth_source == "oauth" {
            sqlx::query("UPDATE public.users SET email_verified = FALSE WHERE id = $1")
                .bind(user_id)
                .execute(&mut **tx)
                .await
                .map_sql_err()?;
        }
    }

    for link_id in &scope.oauth_link_ids {
        let user_id = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM public.user_oauth_links WHERE id = $1 LIMIT 1",
        )
        .bind(link_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()?
        .ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "imported OAuth link row '{link_id}' did not produce a link record"
            ))
        })?;
        state.affected_user_ids.insert(user_id);
    }

    for link_id in &scope.oauth_link_ids {
        if let Some((provider_type, provider_user_id)) = sqlx::query_as::<_, (String, String)>(
            r#"
SELECT imported.provider_type, imported.provider_user_id
FROM public.user_oauth_links imported
JOIN public.user_oauth_links duplicate
  ON duplicate.provider_type = imported.provider_type
 AND duplicate.provider_user_id = imported.provider_user_id
 AND duplicate.id <> imported.id
WHERE imported.id = $1
LIMIT 1
"#,
        )
        .bind(link_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()?
        {
            return Err(DataLayerError::InvalidInput(format!(
                "OAuth import assigns provider identity '{provider_type}:{provider_user_id}' more than once"
            )));
        }
    }

    for link_id in &scope.oauth_link_ids {
        if let Some((user_id, provider_type)) = sqlx::query_as::<_, (String, String)>(
            r#"
SELECT imported.user_id, imported.provider_type
FROM public.user_oauth_links imported
JOIN public.user_oauth_links duplicate
  ON duplicate.user_id = imported.user_id
 AND duplicate.provider_type = imported.provider_type
 AND duplicate.id <> imported.id
WHERE imported.id = $1
LIMIT 1
"#,
        )
        .bind(link_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()?
        {
            return Err(DataLayerError::InvalidInput(format!(
                "OAuth import links user '{user_id}' to provider '{provider_type}' more than once"
            )));
        }
    }

    for link_id in &scope.oauth_link_ids {
        if let Some(invalid_id) = sqlx::query_scalar::<_, String>(
            r#"
SELECT links.id
FROM public.user_oauth_links links
LEFT JOIN public.users users ON users.id = links.user_id
LEFT JOIN public.oauth_providers providers ON providers.provider_type = links.provider_type
WHERE links.id = $1
  AND (
       users.id IS NULL
    OR providers.provider_type IS NULL
    OR links.provider_type <> LOWER(TRIM(links.provider_type))
    OR links.provider_type = ''
    OR links.provider_user_id <> TRIM(links.provider_user_id)
    OR links.provider_user_id = ''
    OR providers.provider_type <> LOWER(TRIM(providers.provider_type))
  )
LIMIT 1
"#,
        )
        .bind(link_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()?
        {
            return Err(DataLayerError::InvalidInput(format!(
                "OAuth import produced invalid or orphaned link '{invalid_id}'"
            )));
        }
    }

    if !scope.validates_oauth_login_methods {
        return Ok(());
    }
    for user_id in state.affected_user_ids {
        if sqlx::query_scalar::<_, String>(
            r#"
SELECT users.id
FROM public.users users
WHERE users.id = $1
  AND users.auth_source = 'oauth'::public.authsource
  AND users.is_active IS TRUE
  AND users.is_deleted IS FALSE
  AND NOT EXISTS (
    SELECT 1
    FROM public.user_oauth_links links
    JOIN public.oauth_providers providers ON providers.provider_type = links.provider_type
    WHERE links.user_id = users.id
      AND providers.is_enabled IS TRUE
      AND links.provider_type = LOWER(TRIM(links.provider_type))
      AND links.provider_user_id = TRIM(links.provider_user_id)
      AND links.provider_user_id <> ''
  )
LIMIT 1
"#,
        )
        .bind(&user_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()?
        .is_some()
        {
            return Err(DataLayerError::InvalidInput(format!(
                "OAuth import would leave active user '{user_id}' without an enabled identity binding"
            )));
        }
    }

    Ok(())
}

async fn reset_postgres_auxiliary_sequences(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), DataLayerError> {
    for table in AUXILIARY_TABLES {
        let [primary_key] = table.primary_key else {
            continue;
        };
        let relation_name = format!("public.{}", table.name);
        let sequence =
            sqlx::query_scalar::<_, Option<String>>("SELECT pg_get_serial_sequence($1, $2)")
                .bind(&relation_name)
                .bind(*primary_key)
                .fetch_one(&mut **tx)
                .await
                .map_sql_err()?;
        let Some(sequence) = sequence else {
            continue;
        };

        let table_sql = postgres_quote_identifier(table.name)?;
        let primary_key_sql = postgres_quote_identifier(primary_key)?;
        let maximum = sqlx::query_scalar::<_, Option<i64>>(&format!(
            "SELECT MAX({primary_key_sql})::bigint FROM public.{table_sql}"
        ))
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()?;
        let (value, is_called) = maximum.map_or((1_i64, false), |value| (value, true));
        sqlx::query("SELECT setval($1::regclass, $2, $3)")
            .bind(sequence)
            .bind(value)
            .bind(is_called)
            .execute(&mut **tx)
            .await
            .map_sql_err()?;
    }
    Ok(())
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
        ExportDomain::Auxiliary => Err(DataLayerError::InvalidInput(
            "postgres auxiliary export uses multiple tables and must be handled as a domain"
                .to_string(),
        )),
    }
}

async fn export_postgres_auxiliary_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for table in AUXILIARY_TABLES {
        let table_sql = postgres_quote_identifier(table.name)?;
        let order_sql = table
            .primary_key
            .iter()
            .map(|column| postgres_quote_identifier(column).map(|column| format!("{column} ASC")))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let rows = sqlx::query(&format!(
            "SELECT to_jsonb(t) AS payload FROM public.{table_sql} AS t ORDER BY {order_sql}"
        ))
        .fetch_all(&mut **tx)
        .await
        .map_sql_err()?;
        for row in rows {
            let payload = row.try_get::<Value, _>("payload").map_sql_err()?;
            let id = auxiliary_row_id(*table, &payload)?;
            records.push(DataExportRecord::row(
                ExportDomain::Auxiliary,
                id,
                payload_with_table(payload, table.name)?,
            ));
        }
    }
    Ok(())
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cache: &mut BTreeMap<String, PostgresImportColumns>,
    table_name: &str,
) -> Result<PostgresImportColumns, DataLayerError> {
    if let Some(columns) = cache.get(table_name) {
        return Ok(columns.clone());
    }

    let columns = load_postgres_import_columns(&mut **tx, table_name).await?;
    cache.insert(table_name.to_string(), columns.clone());
    Ok(columns)
}

pub(super) async fn load_postgres_import_columns<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
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
    .fetch_all(executor)
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
        let rows = sqlx::query(&sql).fetch_all(&mut **tx).await.map_sql_err()?;
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    records: &mut Vec<DataExportRecord>,
) -> Result<(), DataLayerError> {
    for (table_name, export_table, id_column) in postgres_wallet_tables() {
        let sql = format!(
            "SELECT {id_column}::text AS export_id, to_jsonb(t) || jsonb_build_object('__table', '{export_table}') AS payload FROM {table_name} AS t ORDER BY {id_column} ASC"
        );
        let rows = sqlx::query(&sql).fetch_all(&mut **tx).await.map_sql_err()?;
        for row in rows {
            let id = row.try_get::<String, _>("export_id").map_sql_err()?;
            let mut payload = row.try_get::<Value, _>("payload").map_sql_err()?;
            let object = payload.as_object_mut().ok_or_else(|| {
                DataLayerError::UnexpectedValue(format!(
                    "wallet export row in table '{export_table}' must be an object"
                ))
            })?;
            sanitize_payment_security_payload(export_table, object);
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table_name: &str,
    conflict_columns: &[&str],
    domain: ExportDomain,
    row: &ExportRow,
    target_columns: &PostgresImportColumns,
    options: DataImportOptions,
) -> Result<(), DataLayerError> {
    let mut object = normalize_postgres_import_payload(table_name, domain, row, target_columns)?;
    apply_import_credential_policy(
        table_name,
        &mut object,
        |column_name| target_columns.contains_key(column_name),
        options,
    );

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
        .execute(&mut **tx)
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
                normalize_postgres_import_value(table_name, column_name, target_column, value)?,
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
    table_name: &str,
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
    if is_postgres_integer_column(target_column) && import_column_stores_timestamp(column_name) {
        return normalize_imported_integer_timestamp("postgres", table_name, column_name, value)
            .map(|timestamp| timestamp.map_or(Value::Null, |value| Value::Number(value.into())));
    }
    if is_postgres_timestamp_column(target_column) {
        return normalize_postgres_timestamp_value(table_name, column_name, value);
    }
    if is_postgres_bytea_column(target_column) {
        return postgres_bytea_json_value(column_name, value);
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

fn is_postgres_integer_column(target_column: &PostgresImportColumn) -> bool {
    matches!(
        target_column.data_type.as_str(),
        "smallint" | "integer" | "bigint"
    ) || matches!(target_column.udt_name.as_str(), "int2" | "int4" | "int8")
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
    table_name: &str,
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

    let datetime = if import_timestamp_uses_millis(table_name, column_name)
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, PostgresImportColumns>,
) -> Result<(), DataLayerError> {
    let (export_table_name, payload) = billing_payload_table(row)?;
    let (table_name, conflict_column) = postgres_billing_table_name(&export_table_name)?;
    let target_columns = postgres_import_columns_cached(tx, column_cache, table_name).await?;
    import_postgres_row(
        tx,
        table_name,
        &[conflict_column],
        ExportDomain::Billing,
        &ExportRow {
            id: row.id.clone(),
            payload,
        },
        &target_columns,
        DataImportOptions::default(),
    )
    .await
}

async fn import_postgres_auxiliary_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, PostgresImportColumns>,
    options: DataImportOptions,
) -> Result<(), DataLayerError> {
    let (table_name, payload) = domain_payload_table(row, "auxiliary", None)?;
    let table = auxiliary_table(&table_name)?;
    let target_table = format!("public.{}", postgres_quote_identifier(table.name)?);
    let target_columns = postgres_import_columns_cached(tx, column_cache, &target_table).await?;
    import_postgres_row(
        tx,
        &target_table,
        table.primary_key,
        ExportDomain::Auxiliary,
        &ExportRow {
            id: row.id.clone(),
            payload,
        },
        &target_columns,
        options,
    )
    .await
}

fn postgres_billing_table_name(
    table_name: &str,
) -> Result<(&'static str, &'static str), DataLayerError> {
    match table_name {
        "billing_rules" => Ok(("public.billing_rules", "id")),
        "dimension_collectors" => Ok(("public.dimension_collectors", "id")),
        "usage_settlement_snapshots" => Ok(("public.usage_settlement_snapshots", "request_id")),
        other => Err(DataLayerError::InvalidInput(format!(
            "unsupported postgres billing export table '{other}'"
        ))),
    }
}

async fn import_postgres_wallet_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ExportRow,
    column_cache: &mut BTreeMap<String, PostgresImportColumns>,
) -> Result<(), DataLayerError> {
    let (export_table_name, payload) = domain_payload_table(row, "wallet", Some("wallets"))?;
    let (table_name, id_column) = postgres_wallet_table_name(&export_table_name)?;
    let target_columns = postgres_import_columns_cached(tx, column_cache, table_name).await?;
    import_postgres_row(
        tx,
        table_name,
        &[id_column],
        ExportDomain::Wallets,
        &ExportRow {
            id: row.id.clone(),
            payload,
        },
        &target_columns,
        DataImportOptions::default(),
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

#[cfg(test)]
mod billing_table_tests {
    use super::postgres_billing_table_name;

    #[test]
    fn settlement_snapshot_import_uses_request_id_conflict_key() {
        assert_eq!(
            postgres_billing_table_name("usage_settlement_snapshots")
                .expect("settlement snapshot table should be supported"),
            ("public.usage_settlement_snapshots", "request_id")
        );
    }
}
