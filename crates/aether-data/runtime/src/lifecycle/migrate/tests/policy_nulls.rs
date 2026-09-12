use aether_data_contracts::repository::{
    auth::AuthApiKeyLookupKey, management_tokens::ManagementTokenReadRepository,
};
use aether_data_postgres::{
    SqlxAuthApiKeySnapshotReadRepository, SqlxManagementTokenRepository, SqlxUserReadRepository,
};
use serde_json::{json, Value};
use sqlx::{migrate::Migrate, query, query_scalar, Connection, PgConnection, PgPool};

use super::{prepare_database_for_startup, ManagedPostgresServer, POSTGRES_MIGRATOR};
use crate::lifecycle::migrate::run_migrations;

const POLICY_NULL_MIGRATION_VERSION: i64 = 20260908000000;
const POLICY_COLUMNS: &[(&str, &[&str])] = &[
    (
        "api_keys",
        &[
            "allowed_providers",
            "allowed_api_formats",
            "allowed_models",
            "ip_rules",
        ],
    ),
    (
        "users",
        &["allowed_providers", "allowed_api_formats", "allowed_models"],
    ),
    (
        "user_groups",
        &["allowed_providers", "allowed_api_formats", "allowed_models"],
    ),
    ("provider_api_keys", &["api_formats", "allowed_models"]),
    ("management_tokens", &["allowed_ips", "permissions"]),
];

#[tokio::test]
async fn postgres_policy_null_migration_preserves_non_null_policies_and_is_idempotent() {
    let migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == POLICY_NULL_MIGRATION_VERSION)
        .expect("legacy policy null migration should be embedded");
    let legacy_values = [
        Value::Null,
        json!("null"),
        json!(" NULL "),
        json!("\tNuLl\r\n"),
        json!(""),
        json!(" \t\r\n"),
    ];
    let preserved_values = [
        json!([]),
        json!(["provider-allowed"]),
        json!(["openai:chat", "claude:chat"]),
        json!(["127.0.0.1/32"]),
        json!(["null"]),
        json!("provider-allowed"),
        json!("[\"provider-allowed\"]"),
        json!("null-provider"),
        json!("nullnull"),
        json!([null]),
        json!([" "]),
        json!([1]),
        json!({}),
        json!({"policy": null}),
        json!(true),
        json!(42),
    ];
    let cases = std::iter::once((None, None))
        .chain(legacy_values.into_iter().map(|value| (Some(value), None)))
        .chain(
            preserved_values
                .into_iter()
                .map(|value| (Some(value.clone()), Some(value))),
        )
        .collect::<Vec<_>>();
    for storage_type in ["json", "jsonb"] {
        let Some(server) = ManagedPostgresServer::try_start()
            .await
            .expect("postgres policy null test should start or skip")
        else {
            return;
        };
        let pool = PgPool::connect(server.database_url())
            .await
            .expect("policy fixture pool should connect");

        for &(table_name, columns) in POLICY_COLUMNS {
            let definitions = columns
                .iter()
                .map(|column| format!("{column} {storage_type}"))
                .collect::<Vec<_>>()
                .join(", ");
            let modes = if matches!(table_name, "users" | "user_groups") {
                ", allowed_providers_mode text DEFAULT 'deny_all', \
                 allowed_api_formats_mode text DEFAULT 'specific', \
                 allowed_models_mode text DEFAULT 'inherit'"
            } else {
                ""
            };
            query(&format!(
                "CREATE TABLE public.{table_name} \
                 (id integer PRIMARY KEY, {definitions}, metadata {storage_type}{modes})"
            ))
            .execute(&pool)
            .await
            .expect("policy fixture table should be created");
            let placeholders = vec![format!("$2::{storage_type}"); columns.len()].join(", ");
            let insert_sql = format!(
                "INSERT INTO public.{table_name} (id, {}, metadata) \
                 VALUES ($1, {placeholders}, 'null'::{storage_type})",
                columns.join(", ")
            );
            for (case_index, (input, _)) in cases.iter().enumerate() {
                query(&insert_sql)
                    .bind(i32::try_from(case_index).expect("fixture index should fit i32"))
                    .bind(input.clone())
                    .execute(&pool)
                    .await
                    .expect("policy fixture should insert");
            }
        }

        for attempt in 0..2 {
            sqlx::raw_sql(&migration.sql)
                .execute(&pool)
                .await
                .expect("legacy policy null migration should execute");

            for &(table_name, columns) in POLICY_COLUMNS {
                let expected_values = cases
                    .iter()
                    .map(|(input, expected)| match (table_name, input) {
                        ("management_tokens", Some(Value::String(_))) => input.clone(),
                        _ => expected.clone(),
                    })
                    .collect::<Vec<_>>();
                let expected_sql_null_ids = expected_values
                    .iter()
                    .enumerate()
                    .filter(|(_, value)| value.is_none())
                    .map(|(index, _)| i32::try_from(index).expect("fixture index should fit i32"))
                    .collect::<Vec<_>>();
                for column in columns {
                    let actual_values = query_scalar::<_, Option<Value>>(&format!(
                        "SELECT {column} FROM public.{table_name} ORDER BY id"
                    ))
                    .fetch_all(&pool)
                    .await
                    .expect("migrated policy values should be readable");
                    assert_eq!(
                        actual_values, expected_values,
                        "{storage_type}: {table_name}.{column}, attempt {attempt}"
                    );
                    let sql_null_ids = query_scalar::<_, i32>(&format!(
                        "SELECT id FROM public.{table_name} WHERE {column} IS NULL ORDER BY id"
                    ))
                    .fetch_all(&pool)
                    .await
                    .expect("actual SQL NULL rows should be readable");
                    assert_eq!(
                        sql_null_ids, expected_sql_null_ids,
                        "{storage_type}: {table_name}.{column} must contain actual SQL NULL"
                    );
                }
                let metadata_preserved: bool = query_scalar(&format!(
                    "SELECT bool_and(metadata IS NOT NULL AND json_typeof(metadata::json) = 'null') \
                     FROM public.{table_name}"
                ))
                .fetch_one(&pool)
                .await
                .expect("unrelated JSON null metadata should remain readable");
                assert!(
                    metadata_preserved,
                    "{table_name}.metadata must not be cleared"
                );
                if matches!(table_name, "users" | "user_groups") {
                    let modes: Vec<(String, String, String)> = sqlx::query_as(&format!(
                        "SELECT DISTINCT allowed_providers_mode, allowed_api_formats_mode, \
                         allowed_models_mode FROM public.{table_name}"
                    ))
                    .fetch_all(&pool)
                    .await
                    .expect("policy modes should be readable");
                    assert_eq!(
                        modes,
                        vec![(
                            "deny_all".to_string(),
                            "specific".to_string(),
                            "inherit".to_string(),
                        )],
                        "{table_name} policy modes must not change"
                    );
                }
            }
        }
        pool.close().await;
    }
}

#[tokio::test]
async fn postgres_policy_null_migration_repairs_legacy_upgrade_before_auth_reads() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres policy upgrade test should start or skip")
    else {
        return;
    };
    let mut connection = PgConnection::connect(server.database_url())
        .await
        .expect("legacy database connection should open");
    connection
        .ensure_migrations_table()
        .await
        .expect("legacy migration bookkeeping should be created");
    for migration in POSTGRES_MIGRATOR
        .iter()
        .filter(|migration| migration.version < POLICY_NULL_MIGRATION_VERSION)
    {
        connection
            .apply(migration)
            .await
            .expect("previous PostgreSQL migrations should apply");
    }
    drop(connection);

    let pool = PgPool::connect(server.database_url())
        .await
        .expect("legacy database pool should connect");
    sqlx::raw_sql(
        r#"
INSERT INTO public.users (id, username, email_verified)
VALUES ('policy-key-owner', 'policy-key-owner', FALSE),
       ('legacy-policy-user', 'legacy-policy-user', FALSE);

UPDATE public.users
SET allowed_providers = 'null',
    allowed_api_formats = '"null"',
    allowed_models = '""',
    allowed_models_mode = 'deny_all'
WHERE id = 'legacy-policy-user';

INSERT INTO public.api_keys (
    id, user_id, key_hash, allowed_providers, allowed_api_formats, allowed_models, ip_rules
)
VALUES (
    'legacy-policy-key', 'policy-key-owner', repeat('a', 64), 'null', '"null"', '""', 'null'
);

INSERT INTO public.user_groups (
    id, name, normalized_name, allowed_providers, allowed_api_formats, allowed_models,
    allowed_providers_mode, allowed_api_formats_mode, allowed_models_mode
)
VALUES (
    'legacy-policy-group', 'Legacy policy', 'legacy-policy', 'null', '"null"', '""',
    'deny_all', 'specific', 'inherit'
);

INSERT INTO public.management_tokens (
    id, user_id, token_hash, name, allowed_ips, permissions
)
VALUES (
    'legacy-policy-token', 'policy-key-owner', repeat('b', 64), 'Legacy token', 'null', 'null'
), (
    'restricted-policy-token', 'policy-key-owner', repeat('c', 64), 'Restricted token',
    '["127.0.0.1"]', '["admin:users:read"]'
);
"#,
    )
    .execute(&pool)
    .await
    .expect("legacy policy fixtures should insert");

    let auth_repository = SqlxAuthApiKeySnapshotReadRepository::new(pool.clone());
    let users_repository = SqlxUserReadRepository::new(pool.clone());
    let tokens_repository = SqlxManagementTokenRepository::new(pool.clone());
    let api_key_error = auth_repository
        .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("legacy-policy-key"))
        .await
        .expect_err("legacy API key JSON null should reproduce the upgrade failure");
    assert!(api_key_error
        .to_string()
        .contains("api_keys.allowed_providers contains JSON null"));
    assert!(users_repository
        .find_user_auth_by_id("legacy-policy-user")
        .await
        .expect_err("legacy user JSON null should fail strict policy decoding")
        .to_string()
        .contains("users.allowed_providers contains JSON null"));
    assert!(users_repository
        .find_user_group_by_id("legacy-policy-group")
        .await
        .expect_err("legacy group JSON null should fail strict policy decoding")
        .to_string()
        .contains("user_groups.allowed_providers contains JSON null"));
    let legacy_token = tokens_repository
        .get_management_token_with_user("legacy-policy-token")
        .await
        .expect("legacy management token should be readable")
        .expect("legacy management token should exist");
    assert_eq!(legacy_token.token.allowed_ips, Some(Value::Null));
    assert_eq!(legacy_token.token.permissions, Some(Value::Null));

    let pending = prepare_database_for_startup(&pool)
        .await
        .expect("legacy database startup preparation should succeed");
    assert_eq!(
        pending.first().map(|migration| migration.version),
        Some(POLICY_NULL_MIGRATION_VERSION)
    );
    run_migrations(&pool)
        .await
        .expect("startup should normalize legacy policies");

    let snapshot = auth_repository
        .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("legacy-policy-key"))
        .await
        .expect("upgraded API key should decode")
        .expect("upgraded API key should still exist");
    assert!(snapshot.api_key_allowed_providers.is_none());
    assert!(snapshot.api_key_allowed_api_formats.is_none());
    assert!(snapshot.api_key_allowed_models.is_none());
    assert!(snapshot.api_key_ip_rules.is_none());
    let exported_keys = auth_repository
        .list_export_api_keys_by_ids(&["legacy-policy-key".to_string()])
        .await
        .expect("upgraded API key listing should decode");
    assert_eq!(exported_keys.len(), 1);
    assert!(exported_keys[0].allowed_providers.is_none());

    let user = users_repository
        .find_user_auth_by_id("legacy-policy-user")
        .await
        .expect("upgraded user should decode")
        .expect("upgraded user should still exist");
    assert!(user.allowed_providers.is_none());
    assert!(user.allowed_api_formats.is_none());
    assert!(user.allowed_models.is_none());
    assert_eq!(user.allowed_models_mode, "deny_all");
    let group = users_repository
        .find_user_group_by_id("legacy-policy-group")
        .await
        .expect("upgraded group should decode")
        .expect("upgraded group should still exist");
    assert!(group.allowed_providers.is_none());
    assert!(group.allowed_api_formats.is_none());
    assert!(group.allowed_models.is_none());
    assert_eq!(group.allowed_providers_mode, "deny_all");
    assert_eq!(group.allowed_api_formats_mode, "specific");
    assert_eq!(group.allowed_models_mode, "inherit");

    let legacy_token = tokens_repository
        .get_management_token_with_user("legacy-policy-token")
        .await
        .expect("upgraded management token should decode")
        .expect("upgraded management token should still exist");
    assert!(legacy_token.token.allowed_ips.is_none());
    assert!(legacy_token.token.permissions.is_none());
    let restricted_token = tokens_repository
        .get_management_token_with_user("restricted-policy-token")
        .await
        .expect("restricted management token should decode")
        .expect("restricted management token should still exist");
    assert_eq!(
        restricted_token.token.allowed_ips,
        Some(json!(["127.0.0.1"]))
    );
    assert_eq!(
        restricted_token.token.permissions,
        Some(json!(["admin:users:read"]))
    );

    run_migrations(&pool)
        .await
        .expect("restarting an upgraded database should be safe");
    assert!(prepare_database_for_startup(&pool)
        .await
        .expect("upgraded database should remain current")
        .is_empty());
    pool.close().await;
}
