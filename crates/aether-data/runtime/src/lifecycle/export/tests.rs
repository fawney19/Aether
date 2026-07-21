use std::collections::BTreeMap;

use serde_json::json;

use super::{
    build_import_plan, decode_jsonl, encode_jsonl, export_mysql_core_jsonl, export_mysql_jsonl,
    export_postgres_core_jsonl, export_sqlite_core_jsonl, import_mysql_jsonl,
    import_postgres_jsonl, import_sqlite_jsonl, mysql_core_export_domains,
    normalize_postgres_import_payload, postgres_core_export_domains, sqlite_core_export_domains,
    DataExportManifest, DataExportRecord, DataImportPlan, ExportDomain, ExportRow,
    PostgresImportColumn,
};
use crate::driver::postgres::{PostgresPoolConfig, PostgresPoolFactory};
use crate::lifecycle::migrate::{
    run_migrations as run_postgres_migrations, run_mysql_migrations, run_sqlite_migrations,
};
use crate::DatabaseDriver;

#[test]
fn jsonl_round_trips_manifest_and_domain_rows() {
    let records = vec![
        DataExportRecord::manifest(DataExportManifest::new(
            1_700_000_000,
            Some(DatabaseDriver::Postgres),
            vec![ExportDomain::Users, ExportDomain::ApiKeys],
        )),
        DataExportRecord::row(
            ExportDomain::Users,
            "user-1",
            json!({
                "id": "user-1",
                "email": "owner@example.com"
            }),
        ),
        DataExportRecord::row(
            ExportDomain::ApiKeys,
            "api-key-1",
            json!({
                "id": "api-key-1",
                "key_hash": "ciphertext-preserved"
            }),
        ),
    ];

    let encoded = encode_jsonl(&records).expect("records should encode");
    assert_eq!(encoded.lines().count(), 3);

    let decoded = decode_jsonl(&encoded).expect("records should decode");
    assert_eq!(decoded, records);

    let import_plan = build_import_plan(&encoded).expect("import plan should build");
    assert_eq!(
        import_plan.manifest.source_driver,
        Some(DatabaseDriver::Postgres)
    );
    assert_eq!(import_plan.rows(ExportDomain::Users).len(), 1);
    assert_eq!(
        import_plan.rows(ExportDomain::ApiKeys)[0].payload["key_hash"],
        "ciphertext-preserved"
    );
}

#[test]
fn core_export_domains_match_across_sql_drivers() {
    assert_eq!(sqlite_core_export_domains(), mysql_core_export_domains());
    assert_eq!(sqlite_core_export_domains(), postgres_core_export_domains());
}

#[test]
fn jsonl_rejects_missing_manifest() {
    let err = decode_jsonl(r#"{"record_type":"row","domain":"users","id":"user-1","payload":{}}"#)
        .expect_err("missing manifest should fail");

    assert!(err.to_string().contains("must start with a manifest"));
}

#[test]
fn jsonl_rejects_rows_outside_manifest_domains() {
    let records = vec![
        DataExportRecord::manifest(DataExportManifest::new(
            1_700_000_000,
            Some(DatabaseDriver::Sqlite),
            vec![ExportDomain::Users],
        )),
        DataExportRecord::row(
            ExportDomain::Wallets,
            "wallet-1",
            json!({ "id": "wallet-1" }),
        ),
    ];

    let err = encode_jsonl(&records).expect_err("undeclared domain should fail");
    assert!(err.to_string().contains("not declared in manifest"));
}

#[test]
fn jsonl_rejects_bad_json_with_line_number() {
    let err = decode_jsonl(
            r#"{"record_type":"manifest","manifest":{"format_version":1,"created_at_unix_secs":1,"source_driver":null,"domains":["users"]}}
not-json"#,
        )
        .expect_err("bad json should fail");

    assert!(err.to_string().contains("line 2"));
}

#[test]
fn jsonl_rejects_duplicate_domain_ids() {
    let records = vec![
        DataExportRecord::manifest(DataExportManifest::new(
            1_700_000_000,
            None,
            vec![ExportDomain::Users],
        )),
        DataExportRecord::row(ExportDomain::Users, "user-1", json!({ "id": "user-1" })),
        DataExportRecord::row(ExportDomain::Users, "user-1", json!({ "id": "user-1" })),
    ];

    let err = encode_jsonl(&records).expect_err("duplicate id should fail");
    assert!(err.to_string().contains("duplicate"));
}

#[test]
fn postgres_import_payload_normalizes_sqlite_values_for_target_columns() {
    let target_columns = BTreeMap::from([
        (
            "id".to_string(),
            postgres_column("character varying", "varchar"),
        ),
        (
            "email_verified".to_string(),
            postgres_column("boolean", "bool"),
        ),
        (
            "created_at".to_string(),
            postgres_column("timestamp with time zone", "timestamptz"),
        ),
        (
            "allowed_models".to_string(),
            postgres_column("json", "json"),
        ),
        (
            "role".to_string(),
            postgres_not_null_default_column("USER-DEFINED", "userrole"),
        ),
    ]);
    let row = ExportRow {
        id: "user-1".to_string(),
        payload: json!({
            "id": "user-1",
            "email_verified": 1,
            "created_at": 1,
            "allowed_models": "[\"gpt-test\"]",
            "role": null,
            "legacy_nullable": null
        }),
    };

    let normalized = normalize_postgres_import_payload(
        "public.users",
        ExportDomain::Users,
        &row,
        &target_columns,
    )
    .expect("postgres payload should normalize");

    assert_eq!(normalized["email_verified"], json!(true));
    assert_eq!(normalized["created_at"], json!("1970-01-01T00:00:01+00:00"));
    assert_eq!(normalized["allowed_models"], json!(["gpt-test"]));
    assert!(!normalized.contains_key("role"));
    assert!(!normalized.contains_key("legacy_nullable"));
}

#[test]
fn postgres_import_payload_rejects_non_null_unknown_columns() {
    let target_columns = BTreeMap::from([(
        "id".to_string(),
        postgres_column("character varying", "varchar"),
    )]);
    let row = ExportRow {
        id: "user-1".to_string(),
        payload: json!({
            "id": "user-1",
            "unexpected_column": "value"
        }),
    };

    let err = normalize_postgres_import_payload(
        "public.users",
        ExportDomain::Users,
        &row,
        &target_columns,
    )
    .expect_err("non-null unknown columns should fail");

    assert!(err.to_string().contains("unexpected_column"));
    assert!(err.to_string().contains("does not exist"));
}

fn postgres_column(data_type: &str, udt_name: &str) -> PostgresImportColumn {
    PostgresImportColumn {
        data_type: data_type.to_ascii_lowercase(),
        udt_name: udt_name.to_ascii_lowercase(),
        is_nullable: true,
        has_default: false,
    }
}

fn postgres_not_null_default_column(data_type: &str, udt_name: &str) -> PostgresImportColumn {
    PostgresImportColumn {
        data_type: data_type.to_ascii_lowercase(),
        udt_name: udt_name.to_ascii_lowercase(),
        is_nullable: false,
        has_default: true,
    }
}

#[tokio::test]
async fn sqlite_core_export_reads_migrated_database_rows() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_sqlite_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    sqlx::query(
            r#"
INSERT INTO users (id, email, username, auth_source, created_at, updated_at)
VALUES ('user-1', 'owner@example.com', 'owner', 'local', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO user_groups (id, name, normalized_name, description, priority, allowed_models, allowed_models_mode, created_at, updated_at)
VALUES ('group-1', 'Export Group', 'export group', 'Exported group', 10, '["gpt-test"]', 'specific', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO user_group_members (group_id, user_id, created_at)
VALUES ('group-1', 'user-1', '1970-01-01T00:00:01Z');
INSERT INTO api_keys (id, user_id, key_hash, key_encrypted, name, created_at, updated_at)
VALUES ('api-key-1', 'user-1', 'hash-1', 'ciphertext-1', 'Default', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO providers (id, name, provider_type, created_at, updated_at)
VALUES ('provider-1', 'Provider One', 'openai', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO provider_api_keys (id, provider_id, name, encrypted_key, created_at, updated_at)
VALUES ('provider-key-1', 'provider-1', 'Provider Key', 'ciphertext-provider', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO provider_endpoints (id, provider_id, name, base_url, created_at, updated_at)
VALUES ('endpoint-1', 'provider-1', 'Primary', 'https://example.test', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO global_models (id, name, created_at, updated_at)
VALUES ('global-model-1', 'gpt-test', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO models (id, provider_id, global_model_id, provider_model_name, created_at, updated_at)
VALUES ('model-1', 'provider-1', 'global-model-1', 'gpt-test', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO billing_rules (id, global_model_id, name, task_type, expression, variables, dimension_mappings, is_enabled, created_at, updated_at)
VALUES ('billing-rule-1', 'global-model-1', 'Rule One', 'chat', 'input_tokens * 0.01', '{}', '{"input":"input_tokens"}', 1, '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO dimension_collectors (id, api_format, task_type, dimension_name, source_type, value_type, transform_expression, priority, is_enabled, created_at, updated_at)
VALUES ('collector-1', 'openai', 'chat', 'input_tokens', 'computed', 'float', 'usage.input_tokens', 10, 1, '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO system_configs (id, key, value, created_at, updated_at)
VALUES ('config-1', 'billing.enabled', 'true', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO wallets (id, user_id, created_at, updated_at)
VALUES ('wallet-1', 'user-1', '1970-01-01T00:00:01Z', '1970-01-01T00:00:02Z');
INSERT INTO "usage" (request_id, id, user_id, provider_name, model, status, billing_status, created_at_unix_ms, updated_at_unix_secs)
VALUES ('request-1', 'request-1', 'user-1', 'Provider One', 'gpt-test', 'completed', 'settled', 1, 2);
"#,
        )
        .execute(&pool)
        .await
        .expect("sqlite export rows should seed");

    let encoded = export_sqlite_core_jsonl(&pool, 1_700_000_000)
        .await
        .expect("sqlite export should encode");
    let import_plan = build_import_plan(&encoded).expect("sqlite export should decode");

    assert_eq!(
        import_plan.manifest.source_driver,
        Some(DatabaseDriver::Sqlite)
    );
    assert_eq!(import_plan.manifest.domains, sqlite_core_export_domains());
    assert_eq!(
        import_plan.rows(ExportDomain::Users)[0].payload["email"],
        "owner@example.com"
    );
    assert!(import_plan
        .rows(ExportDomain::UserGroups)
        .iter()
        .any(|row| row.id == "group-1" && row.payload["name"] == "Export Group"));
    assert!(import_plan
        .rows(ExportDomain::UserGroupMembers)
        .iter()
        .any(|row| row.id == "group-1:user-1"
            && row.payload["group_id"] == "group-1"
            && row.payload["user_id"] == "user-1"));
    assert_eq!(
        import_plan.rows(ExportDomain::ApiKeys)[0].payload["key_encrypted"],
        "ciphertext-1"
    );
    assert_eq!(
        import_plan.rows(ExportDomain::ProviderKeys)[0].payload["encrypted_key"],
        "ciphertext-provider"
    );
    assert_eq!(import_plan.rows(ExportDomain::Usage)[0].id, "request-1");
    assert_eq!(import_plan.rows(ExportDomain::Billing).len(), 2);
    assert_eq!(
        import_plan.rows(ExportDomain::Billing)[0].payload["__table"],
        "billing_rules"
    );
    assert_eq!(
        import_plan.rows(ExportDomain::Billing)[0].payload["dimension_mappings"]["input"],
        "input_tokens"
    );

    let target_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("target sqlite pool should connect");
    run_sqlite_migrations(&target_pool)
        .await
        .expect("target sqlite migrations should run");
    let imported = import_sqlite_jsonl(&target_pool, &encoded)
        .await
        .expect("sqlite import should load exported rows");
    assert_eq!(imported, 16);

    let imported_api_key =
        sqlx::query_as::<_, (String,)>("SELECT key_encrypted FROM api_keys WHERE id = 'api-key-1'")
            .fetch_one(&target_pool)
            .await
            .expect("imported api key should load");
    assert_eq!(imported_api_key.0, "ciphertext-1");

    let imported_usage = sqlx::query_as::<_, (String,)>(
        "SELECT request_id FROM \"usage\" WHERE request_id = 'request-1'",
    )
    .fetch_one(&target_pool)
    .await
    .expect("imported usage should load");
    assert_eq!(imported_usage.0, "request-1");

    let imported_group_member = sqlx::query_as::<_, (String, String)>(
            "SELECT group_id, user_id FROM user_group_members WHERE group_id = 'group-1' AND user_id = 'user-1'",
        )
        .fetch_one(&target_pool)
        .await
        .expect("imported user group member should load");
    assert_eq!(imported_group_member.0, "group-1");
    assert_eq!(imported_group_member.1, "user-1");

    let imported_billing_rule = sqlx::query_as::<_, (String,)>(
        "SELECT expression FROM billing_rules WHERE id = 'billing-rule-1'",
    )
    .fetch_one(&target_pool)
    .await
    .expect("imported billing rule should load");
    assert_eq!(imported_billing_rule.0, "input_tokens * 0.01");

    if let Some(database_url) = std::env::var("AETHER_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let config = PostgresPoolConfig {
            database_url,
            min_connections: 1,
            max_connections: 1,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        };
        let postgres_pool = PostgresPoolFactory::new(config)
            .expect("postgres factory should build")
            .connect_lazy()
            .expect("postgres pool should build");
        run_postgres_migrations(&postgres_pool)
            .await
            .expect("postgres migrations should run");

        let imported = import_postgres_jsonl(&postgres_pool, &encoded)
            .await
            .expect("postgres import should load exported rows");
        assert_eq!(imported, 16);

        let imported_api_key = sqlx::query_as::<_, (String,)>(
            "SELECT key_encrypted FROM api_keys WHERE id = 'api-key-1'",
        )
        .fetch_one(&postgres_pool)
        .await
        .expect("imported postgres api key should load");
        assert_eq!(imported_api_key.0, "ciphertext-1");
    }
}

#[tokio::test]
async fn postgres_core_export_reads_migrated_database_rows_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "skipping postgres core export smoke test because AETHER_TEST_POSTGRES_URL is unset"
        );
        return;
    };

    let config = PostgresPoolConfig {
        database_url,
        min_connections: 1,
        max_connections: 1,
        acquire_timeout_ms: 1_000,
        idle_timeout_ms: 5_000,
        max_lifetime_ms: 30_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    };
    let pool = PostgresPoolFactory::new(config)
        .expect("postgres factory should build")
        .connect_lazy()
        .expect("postgres pool should build");
    run_postgres_migrations(&pool)
        .await
        .expect("postgres migrations should run");

    let suffix = unique_suffix();
    let user_id = format!("export-user-{suffix}");
    let api_key_id = format!("export-api-key-{suffix}");
    let provider_id = format!("export-provider-{suffix}");
    let provider_key_id = format!("export-provider-key-{suffix}");
    let endpoint_id = format!("export-endpoint-{suffix}");
    let global_model_id = format!("export-global-model-{suffix}");
    let model_id = format!("export-model-{suffix}");
    let billing_rule_id = format!("export-billing-rule-{suffix}");
    let collector_id = format!("export-collector-{suffix}");
    let config_id = format!("export-config-{suffix}");
    let config_key = format!("export.config.{suffix}");
    let wallet_id = format!("export-wallet-{suffix}");
    let request_id = format!("export-request-{suffix}");
    let group_id = format!("export-group-{suffix}");

    sqlx::query(
            "INSERT INTO users (id, email, username, auth_source, email_verified, created_at, updated_at) VALUES ($1, $2, $3, 'local', TRUE, to_timestamp(1), to_timestamp(2))",
        )
        .bind(&user_id)
        .bind(format!("{user_id}@example.com"))
        .bind(format!("owner-{suffix}"))
        .execute(&pool)
        .await
        .expect("user should seed");
    sqlx::query(
            "INSERT INTO user_groups (id, name, normalized_name, priority, allowed_models, allowed_models_mode, created_at, updated_at) VALUES ($1, $2, $3, 10, '[\"provider-model\"]', 'specific', to_timestamp(1), to_timestamp(2))",
        )
        .bind(&group_id)
        .bind(format!("Export Group {suffix}"))
        .bind(format!("export group {suffix}"))
        .execute(&pool)
        .await
        .expect("user group should seed");
    sqlx::query(
            "INSERT INTO user_group_members (group_id, user_id, created_at) VALUES ($1, $2, to_timestamp(1))",
        )
        .bind(&group_id)
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("user group member should seed");
    sqlx::query(
            "INSERT INTO api_keys (id, user_id, key_hash, key_encrypted, name, created_at, updated_at) VALUES ($1, $2, $3, 'ciphertext-1', 'Default', to_timestamp(1), to_timestamp(2))",
        )
        .bind(&api_key_id)
        .bind(&user_id)
        .bind(format!("hash-{api_key_id}"))
        .execute(&pool)
        .await
        .expect("api key should seed");
    sqlx::query(
            "INSERT INTO providers (id, name, provider_type, created_at, updated_at) VALUES ($1, $2, 'openai', to_timestamp(1), to_timestamp(2))",
        )
        .bind(&provider_id)
        .bind(format!("Provider {suffix}"))
        .execute(&pool)
        .await
        .expect("provider should seed");
    sqlx::query(
            "INSERT INTO provider_api_keys (id, provider_id, name, encrypted_key, total_tokens, total_cost_usd, created_at, updated_at) VALUES ($1, $2, 'Provider Key', 'ciphertext-provider', 0, 0, to_timestamp(1), to_timestamp(2))",
        )
        .bind(&provider_key_id)
        .bind(&provider_id)
        .execute(&pool)
        .await
        .expect("provider key should seed");
    sqlx::query(
            "INSERT INTO provider_endpoints (id, provider_id, name, base_url, created_at, updated_at) VALUES ($1, $2, 'Primary', 'https://example.test', to_timestamp(1), to_timestamp(2))",
        )
        .bind(&endpoint_id)
        .bind(&provider_id)
        .execute(&pool)
        .await
        .expect("endpoint should seed");
    sqlx::query(
            "INSERT INTO global_models (id, name, created_at, updated_at) VALUES ($1, $2, to_timestamp(1), to_timestamp(2))",
        )
        .bind(&global_model_id)
        .bind(format!("global-model-{suffix}"))
        .execute(&pool)
        .await
        .expect("global model should seed");
    sqlx::query(
            "INSERT INTO models (id, provider_id, global_model_id, provider_model_name, created_at, updated_at) VALUES ($1, $2, $3, 'provider-model', to_timestamp(1), to_timestamp(2))",
        )
        .bind(&model_id)
        .bind(&provider_id)
        .bind(&global_model_id)
        .execute(&pool)
        .await
        .expect("model should seed");
    sqlx::query(
            "INSERT INTO billing_rules (id, global_model_id, name, task_type, expression, variables, dimension_mappings, is_enabled, created_at, updated_at) VALUES ($1, $2, 'Rule One', 'chat', 'input_tokens * 0.01', '{}', '{\"input\":\"input_tokens\"}', TRUE, to_timestamp(1), to_timestamp(2))",
        )
        .bind(&billing_rule_id)
        .bind(&global_model_id)
        .execute(&pool)
        .await
        .expect("billing rule should seed");
    sqlx::query(
            "INSERT INTO dimension_collectors (id, api_format, task_type, dimension_name, source_type, value_type, transform_expression, priority, is_enabled, created_at, updated_at) VALUES ($1, 'openai', 'chat', $2, 'computed', 'float', 'usage.input_tokens', 10, TRUE, to_timestamp(1), to_timestamp(2))",
        )
        .bind(&collector_id)
        .bind(format!("input_tokens_{suffix}"))
        .execute(&pool)
        .await
        .expect("dimension collector should seed");
    sqlx::query(
            "INSERT INTO system_configs (id, key, value, created_at, updated_at) VALUES ($1, $2, 'true', to_timestamp(1), to_timestamp(2))",
        )
        .bind(&config_id)
        .bind(&config_key)
        .execute(&pool)
        .await
        .expect("system config should seed");
    sqlx::query(
            "INSERT INTO wallets (id, user_id, created_at, updated_at) VALUES ($1, $2, to_timestamp(1), to_timestamp(2))",
        )
        .bind(&wallet_id)
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("wallet should seed");
    sqlx::query(
            "INSERT INTO \"usage\" (request_id, id, user_id, provider_name, model, status, billing_status, created_at_unix_ms, updated_at_unix_secs) VALUES ($1, $2, $3, 'Provider One', 'provider-model', 'completed', 'settled', 1, 2)",
        )
        .bind(&request_id)
        .bind(&request_id)
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("usage should seed");

    let encoded = export_postgres_core_jsonl(&pool, 1_700_000_000)
        .await
        .expect("postgres export should encode");
    let import_plan = build_import_plan(&encoded).expect("postgres export should decode");

    assert_eq!(
        import_plan.manifest.source_driver,
        Some(DatabaseDriver::Postgres)
    );
    assert_eq!(import_plan.manifest.domains, postgres_core_export_domains());
    assert!(import_plan
        .rows(ExportDomain::Users)
        .iter()
        .any(|row| row.id == user_id));
    assert!(import_plan
        .rows(ExportDomain::UserGroups)
        .iter()
        .any(|row| row.id == group_id));
    assert!(import_plan
        .rows(ExportDomain::UserGroupMembers)
        .iter()
        .any(|row| row.id == format!("{group_id}:{user_id}")));
    assert!(import_plan
        .rows(ExportDomain::ApiKeys)
        .iter()
        .any(|row| row.id == api_key_id && row.payload["key_encrypted"] == "ciphertext-1"));
    assert!(import_plan
        .rows(ExportDomain::ProviderKeys)
        .iter()
        .any(|row| {
            row.id == provider_key_id && row.payload["encrypted_key"] == "ciphertext-provider"
        }));
    assert!(import_plan
        .rows(ExportDomain::GlobalModels)
        .iter()
        .any(|row| row.id == global_model_id));
    assert!(import_plan
        .rows(ExportDomain::Models)
        .iter()
        .any(|row| row.id == model_id));
    assert!(import_plan
        .rows(ExportDomain::Usage)
        .iter()
        .any(|row| row.id == request_id));

    let target_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("target sqlite pool should connect");
    run_sqlite_migrations(&target_pool)
        .await
        .expect("target sqlite migrations should run");
    let imported = import_sqlite_jsonl(&target_pool, &encoded)
        .await
        .expect("sqlite import should load postgres exported rows");
    assert_eq!(imported, import_plan_row_count(&import_plan));

    let imported_api_key =
        sqlx::query_as::<_, (String,)>("SELECT key_encrypted FROM api_keys WHERE id = $1")
            .bind(&api_key_id)
            .fetch_one(&target_pool)
            .await
            .expect("imported sqlite api key should load");
    assert_eq!(imported_api_key.0, "ciphertext-1");
    let imported_group_member = sqlx::query_as::<_, (String, String)>(
        "SELECT group_id, user_id FROM user_group_members WHERE group_id = ? AND user_id = ?",
    )
    .bind(&group_id)
    .bind(&user_id)
    .fetch_one(&target_pool)
    .await
    .expect("imported sqlite user group member should load");
    assert_eq!(imported_group_member.0, group_id);
    assert_eq!(imported_group_member.1, user_id);
}

#[tokio::test]
async fn mysql_core_export_reads_migrated_database_rows_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql core export smoke test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");
    run_mysql_migrations(&pool)
        .await
        .expect("mysql migrations should run");

    let suffix = unique_suffix();
    let user_id = format!("export-user-{suffix}");
    let api_key_id = format!("export-api-key-{suffix}");
    let provider_id = format!("export-provider-{suffix}");
    let provider_key_id = format!("export-provider-key-{suffix}");
    let endpoint_id = format!("export-endpoint-{suffix}");
    let global_model_id = format!("export-global-model-{suffix}");
    let model_id = format!("export-model-{suffix}");
    let config_id = format!("export-config-{suffix}");
    let wallet_id = format!("export-wallet-{suffix}");
    let request_id = format!("export-request-{suffix}");
    let group_id = format!("export-group-{suffix}");

    sqlx::query(
            "INSERT INTO users (id, email, username, auth_source, created_at, updated_at) VALUES (?, ?, ?, 'local', 1, 2)",
        )
        .bind(&user_id)
        .bind(format!("{user_id}@example.com"))
        .bind(format!("owner-{suffix}"))
        .execute(&pool)
        .await
        .expect("user should seed");
    sqlx::query(
            "INSERT INTO user_groups (id, name, normalized_name, priority, allowed_models, allowed_models_mode, created_at, updated_at) VALUES (?, ?, ?, 10, '[\"provider-model\"]', 'specific', 1, 2)",
        )
        .bind(&group_id)
        .bind(format!("Export Group {suffix}"))
        .bind(format!("export group {suffix}"))
        .execute(&pool)
        .await
        .expect("user group should seed");
    sqlx::query("INSERT INTO user_group_members (group_id, user_id, created_at) VALUES (?, ?, 1)")
        .bind(&group_id)
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("user group member should seed");
    sqlx::query(
            "INSERT INTO api_keys (id, user_id, key_hash, key_encrypted, name, created_at, updated_at) VALUES (?, ?, ?, 'ciphertext-1', 'Default', 1, 2)",
        )
        .bind(&api_key_id)
        .bind(&user_id)
        .bind(format!("hash-{api_key_id}"))
        .execute(&pool)
        .await
        .expect("api key should seed");
    sqlx::query(
            "INSERT INTO providers (id, name, provider_type, created_at, updated_at) VALUES (?, ?, 'openai', 1, 2)",
        )
        .bind(&provider_id)
        .bind(format!("Provider {suffix}"))
        .execute(&pool)
        .await
        .expect("provider should seed");
    sqlx::query(
            "INSERT INTO provider_api_keys (id, provider_id, name, encrypted_key, created_at, updated_at) VALUES (?, ?, 'Provider Key', 'ciphertext-provider', 1, 2)",
        )
        .bind(&provider_key_id)
        .bind(&provider_id)
        .execute(&pool)
        .await
        .expect("provider key should seed");
    sqlx::query(
            "INSERT INTO provider_endpoints (id, provider_id, name, base_url, created_at, updated_at) VALUES (?, ?, 'Primary', 'https://example.test', 1, 2)",
        )
        .bind(&endpoint_id)
        .bind(&provider_id)
        .execute(&pool)
        .await
        .expect("endpoint should seed");
    sqlx::query("INSERT INTO global_models (id, name, created_at, updated_at) VALUES (?, ?, 1, 2)")
        .bind(&global_model_id)
        .bind(format!("global-model-{suffix}"))
        .execute(&pool)
        .await
        .expect("global model should seed");
    sqlx::query(
            "INSERT INTO models (id, provider_id, global_model_id, provider_model_name, created_at, updated_at) VALUES (?, ?, ?, 'provider-model', 1, 2)",
        )
        .bind(&model_id)
        .bind(&provider_id)
        .bind(&global_model_id)
        .execute(&pool)
        .await
        .expect("model should seed");
    sqlx::query(
            "INSERT INTO system_configs (id, `key`, value, created_at, updated_at) VALUES (?, ?, 'true', 1, 2)",
        )
        .bind(&config_id)
        .bind(format!("export.config.{suffix}"))
        .execute(&pool)
        .await
        .expect("system config should seed");
    sqlx::query("INSERT INTO wallets (id, user_id, created_at, updated_at) VALUES (?, ?, 1, 2)")
        .bind(&wallet_id)
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("wallet should seed");
    sqlx::query(
            "INSERT INTO `usage` (request_id, id, user_id, provider_name, model, status, billing_status, created_at_unix_ms, updated_at_unix_secs) VALUES (?, ?, ?, 'Provider One', 'provider-model', 'completed', 'settled', 1, 2)",
        )
        .bind(&request_id)
        .bind(&request_id)
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("usage should seed");

    let encoded = export_mysql_core_jsonl(&pool, 1_700_000_000)
        .await
        .expect("mysql export should encode");
    let import_plan = build_import_plan(&encoded).expect("mysql export should decode");

    assert_eq!(
        import_plan.manifest.source_driver,
        Some(DatabaseDriver::Mysql)
    );
    assert_eq!(import_plan.manifest.domains, mysql_core_export_domains());
    assert!(import_plan
        .rows(ExportDomain::Users)
        .iter()
        .any(|row| row.id == user_id));
    assert!(import_plan
        .rows(ExportDomain::UserGroups)
        .iter()
        .any(|row| row.id == group_id));
    assert!(import_plan
        .rows(ExportDomain::UserGroupMembers)
        .iter()
        .any(|row| row.id == format!("{group_id}:{user_id}")));
    assert!(import_plan
        .rows(ExportDomain::ApiKeys)
        .iter()
        .any(|row| row.id == api_key_id && row.payload["key_encrypted"] == "ciphertext-1"));
    assert!(import_plan
        .rows(ExportDomain::ProviderKeys)
        .iter()
        .any(|row| {
            row.id == provider_key_id && row.payload["encrypted_key"] == "ciphertext-provider"
        }));
    assert!(import_plan
        .rows(ExportDomain::Usage)
        .iter()
        .any(|row| row.id == request_id));

    let selected_export = export_mysql_jsonl(
        &pool,
        vec![
            ExportDomain::Users,
            ExportDomain::UserGroups,
            ExportDomain::UserGroupMembers,
            ExportDomain::ApiKeys,
            ExportDomain::ProviderKeys,
            ExportDomain::Usage,
        ],
        1_700_000_001,
    )
    .await
    .expect("selected mysql export should encode");
    let imported = import_mysql_jsonl(&pool, &selected_export)
        .await
        .expect("mysql import should be idempotent");
    assert!(imported >= 6);

    let imported_api_key =
        sqlx::query_as::<_, (String,)>("SELECT key_encrypted FROM api_keys WHERE id = ?")
            .bind(&api_key_id)
            .fetch_one(&pool)
            .await
            .expect("imported mysql api key should load");
    assert_eq!(imported_api_key.0, "ciphertext-1");
}

fn unique_suffix() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{:016x}", nanos ^ counter.rotate_left(17))
}

fn import_plan_row_count(plan: &DataImportPlan) -> usize {
    plan.manifest
        .domains
        .iter()
        .map(|domain| plan.rows(*domain).len())
        .sum()
}
