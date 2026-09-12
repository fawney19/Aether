use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::{
    apply_import_credential_policy, build_import_plan, deactivate_imported_credentials,
    decode_jsonl, decode_jsonl_with_limits, encode_jsonl, export_postgres_core_jsonl,
    normalize_imported_binary, normalize_imported_integer_timestamp,
    normalize_postgres_import_payload, postgres_bytea_json_value, postgres_core_export_domains,
    DataExportManifest, DataExportRecord, DataImportOptions, ExportDomain, ExportRow,
    PostgresImportColumn,
};
use crate::driver::postgres::{PostgresPoolConfig, PostgresPoolFactory};
use crate::lifecycle::migrate::run_migrations as run_postgres_migrations;
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
fn core_export_domains_include_auxiliary_tables() {
    assert!(postgres_core_export_domains().contains(&ExportDomain::Auxiliary));
}

#[test]
fn version_one_exports_remain_importable_after_full_export_expansion() {
    let records = decode_jsonl(
        r#"{"record_type":"manifest","manifest":{"format_version":1,"created_at_unix_secs":1,"source_driver":null,"domains":["users"]}}
{"record_type":"row","domain":"users","id":"user-1","payload":{"id":"user-1"}}"#,
    )
    .expect("version one exports should remain supported");

    assert_eq!(records.len(), 2);
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
            Some(DatabaseDriver::Postgres),
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
fn jsonl_rejects_input_and_record_limits_before_materializing_rows() {
    let oversized = "x".repeat(11);
    let err = decode_jsonl_with_limits(&oversized, 10, 100, 10)
        .expect_err("input byte limit should be enforced");
    assert!(err.to_string().contains("10 byte input limit"));

    let manifest = r#"{"record_type":"manifest","manifest":{"format_version":1,"created_at_unix_secs":1,"source_driver":null,"domains":[]}}"#;
    let err = decode_jsonl_with_limits(manifest, usize::MAX, 10, 10)
        .expect_err("line byte limit should be enforced");
    assert!(err.to_string().contains("byte line limit"));

    let row = r#"{"record_type":"manifest","manifest":{"format_version":1,"created_at_unix_secs":1,"source_driver":null,"domains":[]}}"#;
    let input = format!("{row}\n{row}\n");
    let err = decode_jsonl_with_limits(&input, usize::MAX, usize::MAX, 1)
        .expect_err("record limit should be enforced");
    assert!(err.to_string().contains("1 record limit"));
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
fn postgres_import_payload_normalizes_imported_values_for_target_columns() {
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
fn cross_driver_timestamp_normalization_preserves_usage_second_contract() {
    assert_eq!(
        normalize_imported_integer_timestamp(
            "postgres",
            r#""usage""#,
            "created_at_unix_ms",
            &json!("1970-01-01T00:00:01.234900Z"),
        )
        .expect("usage timestamp should normalize"),
        Some(1),
    );
    assert_eq!(
        normalize_imported_integer_timestamp(
            "postgres",
            "request_candidates",
            "created_at_unix_ms",
            &json!("1970-01-01T00:00:01.234900Z"),
        )
        .expect("millisecond timestamp should normalize"),
        Some(1_234),
    );

    let target_columns = BTreeMap::from([(
        "created_at_unix_ms".to_string(),
        postgres_column("timestamp with time zone", "timestamptz"),
    )]);
    let row = ExportRow {
        id: "usage-1".to_string(),
        payload: json!({ "created_at_unix_ms": 1_700_000_000 }),
    };
    let normalized = normalize_postgres_import_payload(
        "public.usage",
        ExportDomain::Usage,
        &row,
        &target_columns,
    )
    .expect("postgres usage timestamp should normalize");
    assert_eq!(
        normalized["created_at_unix_ms"],
        json!("2023-11-14T22:13:20+00:00")
    );

    let target_columns = BTreeMap::from([(
        "created_at_unix_ms".to_string(),
        postgres_column("bigint", "int8"),
    )]);
    let row = ExportRow {
        id: "usage-1".to_string(),
        payload: json!({ "created_at_unix_ms": "1970-01-01T00:00:01.234900Z" }),
    };
    let normalized = normalize_postgres_import_payload(
        "public.usage",
        ExportDomain::Usage,
        &row,
        &target_columns,
    )
    .expect("postgres integer usage timestamp should normalize");
    assert_eq!(normalized["created_at_unix_ms"], json!(1));
}

#[test]
fn cross_driver_binary_normalization_preserves_raw_bytes() {
    assert_eq!(
        normalize_imported_binary("postgres", "payload_gzip", &json!([0, 1, 127, 255]))
            .expect("byte array should normalize"),
        Some(vec![0, 1, 127, 255]),
    );
    assert_eq!(
        normalize_imported_binary("postgres", "payload_gzip", &json!("\\x00017fff"))
            .expect("postgres hex should normalize"),
        Some(vec![0, 1, 127, 255]),
    );
    assert!(normalize_imported_binary("postgres", "payload_gzip", &json!([256])).is_err());
    assert_eq!(
        postgres_bytea_json_value("payload_gzip", &json!([0, 1, 127, 255]))
            .expect("postgres bytea should normalize"),
        json!("\\x00017fff"),
    );
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

#[test]
fn imported_identity_credentials_are_replaced_with_disabled_tombstones() {
    let columns = BTreeSet::from([
        "password_hash".to_string(),
        "key_hash".to_string(),
        "key_encrypted".to_string(),
        "status".to_string(),
        "is_active".to_string(),
        "is_locked".to_string(),
        "token_hash".to_string(),
        "refresh_token_hash".to_string(),
        "prev_refresh_token_hash".to_string(),
        "revoked_at".to_string(),
        "revoke_reason".to_string(),
    ]);

    let mut user =
        serde_json::Map::from_iter([("password_hash".to_string(), json!("$2b$12$backup-hash"))]);
    deactivate_imported_credentials("users", &mut user, |column| columns.contains(column));
    assert_ne!(user["password_hash"], json!("$2b$12$backup-hash"));
    assert!(user["password_hash"]
        .as_str()
        .is_some_and(|value| value.starts_with("$aether-import-revoked$")));

    let mut api_key = serde_json::Map::from_iter([
        ("key_hash".to_string(), json!("backup-key-hash")),
        ("key_encrypted".to_string(), json!("backup-ciphertext")),
        ("is_active".to_string(), json!(true)),
        ("is_locked".to_string(), json!(false)),
        ("status".to_string(), json!("active")),
    ]);
    deactivate_imported_credentials("api_keys", &mut api_key, |column| columns.contains(column));
    assert_ne!(api_key["key_hash"], json!("backup-key-hash"));
    assert_eq!(api_key["key_encrypted"], Value::Null);
    assert_eq!(api_key["is_active"], json!(false));
    assert_eq!(api_key["is_locked"], json!(true));
    assert_eq!(api_key["status"], json!("disabled"));

    let mut token = serde_json::Map::from_iter([
        ("token_hash".to_string(), json!("backup-token-hash")),
        ("is_active".to_string(), json!(true)),
    ]);
    deactivate_imported_credentials("management_tokens", &mut token, |column| {
        columns.contains(column)
    });
    assert_ne!(token["token_hash"], json!("backup-token-hash"));
    assert_eq!(token["is_active"], json!(false));

    let mut session = serde_json::Map::from_iter([
        (
            "refresh_token_hash".to_string(),
            json!("backup-refresh-hash"),
        ),
        (
            "prev_refresh_token_hash".to_string(),
            json!("backup-previous-hash"),
        ),
        ("revoked_at".to_string(), Value::Null),
        ("revoke_reason".to_string(), Value::Null),
    ]);
    deactivate_imported_credentials("user_sessions", &mut session, |column| {
        columns.contains(column)
    });
    assert_ne!(session["refresh_token_hash"], json!("backup-refresh-hash"));
    assert_eq!(session["prev_refresh_token_hash"], Value::Null);
    assert!(session["revoked_at"].as_i64().is_some());
    assert_eq!(
        session["revoke_reason"],
        json!("imported_credentials_revoked")
    );
}

#[test]
fn imported_proxy_nodes_receive_a_new_offline_tunnel_generation() {
    let columns = BTreeSet::from([
        "tunnel_generation".to_string(),
        "tunnel_connected".to_string(),
        "status".to_string(),
        "active_connections".to_string(),
    ]);
    let mut node = serde_json::Map::from_iter([
        (
            "tunnel_generation".to_string(),
            json!("backup-tunnel-generation"),
        ),
        ("tunnel_connected".to_string(), json!(true)),
        ("status".to_string(), json!("online")),
        ("active_connections".to_string(), json!(42)),
        (
            "proxy_metadata".to_string(),
            json!({"tunnel_security": {"encryption_key": "preserved-psk"}}),
        ),
    ]);

    deactivate_imported_credentials("public.proxy_nodes", &mut node, |column| {
        columns.contains(column)
    });

    let generation = node["tunnel_generation"]
        .as_str()
        .expect("imported node generation should be a string");
    assert_ne!(generation, "backup-tunnel-generation");
    assert!(uuid::Uuid::parse_str(generation).is_ok());
    assert_eq!(node["tunnel_connected"], json!(false));
    assert_eq!(node["status"], json!("offline"));
    assert_eq!(node["active_connections"], json!(0));
    assert_eq!(
        node["proxy_metadata"]["tunnel_security"]["encryption_key"],
        json!("preserved-psk")
    );
}

#[test]
fn trusted_import_preserves_stable_credentials_only_when_explicitly_requested() {
    assert!(!DataImportOptions::default().preserve_credentials);
    for (table, payload) in [
        ("users", json!({"password_hash": "$2b$12$trusted-hash"})),
        (
            "public.\"api_keys\"",
            json!({
                "key_hash": "trusted-key-hash", "key_encrypted": "trusted-ciphertext",
                "status": "active", "is_active": true, "is_locked": false,
            }),
        ),
        (
            "management_tokens",
            json!({"token_hash": "trusted-token-hash", "is_active": true}),
        ),
    ] {
        for preserve_credentials in [false, true] {
            let mut object = payload.as_object().unwrap().clone();
            apply_import_credential_policy(
                table,
                &mut object,
                |_| true,
                DataImportOptions {
                    preserve_credentials,
                },
            );
            if preserve_credentials {
                assert_eq!(&object, payload.as_object().unwrap());
            } else {
                assert_ne!(&object, payload.as_object().unwrap());
            }
        }
    }
}

#[test]
fn trusted_import_still_revokes_imported_sessions_and_live_tunnels() {
    let options = DataImportOptions {
        preserve_credentials: true,
    };
    let mut session = json!({
        "refresh_token_hash": "old-session", "prev_refresh_token_hash": "older-session",
        "revoked_at": null, "revoke_reason": null,
    })
    .as_object()
    .unwrap()
    .clone();
    apply_import_credential_policy("public.user_sessions", &mut session, |_| true, options);
    assert_ne!(session["refresh_token_hash"], json!("old-session"));
    assert_eq!(session["prev_refresh_token_hash"], Value::Null);
    assert_eq!(
        session["revoke_reason"],
        json!("imported_credentials_revoked")
    );

    let mut node = json!({
        "tunnel_generation": "old-generation", "tunnel_connected": true,
        "status": "online", "active_connections": 10,
    })
    .as_object()
    .unwrap()
    .clone();
    apply_import_credential_policy("proxy_nodes", &mut node, |_| true, options);
    assert_ne!(node["tunnel_generation"], json!("old-generation"));
    assert_eq!(node["tunnel_connected"], json!(false));
    assert_eq!(node["status"], json!("offline"));
    assert_eq!(node["active_connections"], json!(0));
}

#[tokio::test]
#[ignore = "requires AETHER_TEST_POSTGRES_URL and PostgreSQL migrations"]
async fn live_import_credential_policy_round_trips_through_postgres() {
    let pool = PostgresPoolFactory::new(PostgresPoolConfig {
        database_url: std::env::var("AETHER_TEST_POSTGRES_URL").unwrap(),
        ..Default::default()
    })
    .unwrap()
    .connect_lazy()
    .unwrap();
    run_postgres_migrations(&pool).await.unwrap();
    for preserve_credentials in [false, true] {
        let user_id = uuid::Uuid::new_v4().to_string();
        let key_id = uuid::Uuid::new_v4().to_string();
        let password_hash = "$2b$12$trusted-import-hash";
        let key_hash = format!("trusted-{key_id}");
        sqlx::query("INSERT INTO users (id, username, password_hash, auth_source, email_verified) VALUES ($1, $2, $3, 'local', FALSE)")
            .bind(&user_id).bind(format!("import-{}", &user_id[..8])).bind(password_hash)
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO api_keys (id, user_id, key_hash, key_encrypted, name) VALUES ($1, $2, $3, 'trusted-ciphertext', 'Import probe')")
            .bind(&key_id).bind(&user_id).bind(&key_hash).execute(&pool).await.unwrap();
        let user: Value = sqlx::query_scalar("SELECT to_jsonb(users) FROM users WHERE id = $1")
            .bind(&user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let key: Value =
            sqlx::query_scalar("SELECT to_jsonb(api_keys) FROM api_keys WHERE id = $1")
                .bind(&key_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let input = encode_jsonl(&[
            DataExportRecord::manifest(DataExportManifest::new(
                1_788_739_200,
                Some(DatabaseDriver::Postgres),
                vec![ExportDomain::Users, ExportDomain::ApiKeys],
            )),
            DataExportRecord::row(ExportDomain::Users, &user_id, user),
            DataExportRecord::row(ExportDomain::ApiKeys, &key_id, key),
        ])
        .unwrap();
        super::postgres::import_postgres_jsonl_with_options(
            &pool,
            &input,
            DataImportOptions {
                preserve_credentials,
            },
        )
        .await
        .unwrap();
        let imported_password: String =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
                .bind(&user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let imported_key: (String, Option<String>, bool) =
            sqlx::query_as("SELECT key_hash, key_encrypted, is_active FROM api_keys WHERE id = $1")
                .bind(&key_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(imported_password == password_hash, preserve_credentials);
        assert_eq!(imported_key.0 == key_hash, preserve_credentials);
        assert_eq!(
            imported_key.1.as_deref(),
            preserve_credentials.then_some("trusted-ciphertext")
        );
        assert_eq!(imported_key.2, preserve_credentials);
        sqlx::query("DELETE FROM api_keys WHERE id = $1")
            .bind(&key_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(&user_id)
            .execute(&pool)
            .await
            .unwrap();
    }
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
