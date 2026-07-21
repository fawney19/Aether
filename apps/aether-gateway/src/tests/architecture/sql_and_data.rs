use super::*;

fn production_source(source: &str) -> &str {
    source.split("#[cfg(test)]").next().unwrap_or(source)
}

#[test]
fn handlers_do_not_inline_sql_queries() {
    assert_no_sqlx_queries("src/handlers");
}

#[test]
fn gateway_runtime_does_not_inline_sql_queries() {
    assert_no_sqlx_queries("src/state/runtime");
}

#[test]
fn aether_data_bootstrap_snapshot_is_built_from_schema_sources() {
    let build_rs = read_workspace_file("crates/aether-data/runtime/build.rs");
    assert!(
        build_rs.contains("schema/bootstrap/postgres/manifest.txt"),
        "build.rs should source the bootstrap snapshot from schema/bootstrap/postgres"
    );

    let compose_schema = read_workspace_file("crates/aether-data/runtime/schema/compose_schema.sh");
    assert!(
        compose_schema.contains("check_bootstrap_sources"),
        "compose_schema.sh should still validate bootstrap source fragments"
    );
    assert!(
        !compose_schema.contains("bootstrap/postgres/20260413020000_empty_database_snapshot.sql"),
        "compose_schema.sh should not depend on the outer bootstrap artifact anymore"
    );

    let bootstrap =
        read_workspace_file("crates/aether-data/runtime/src/lifecycle/bootstrap/postgres.rs");
    assert!(
        bootstrap
            .contains("include_str!(concat!(env!(\"OUT_DIR\"), \"/empty_database_snapshot.sql\"))"),
        "lifecycle/bootstrap/postgres.rs should embed the generated bootstrap snapshot from OUT_DIR"
    );
    assert!(
        !bootstrap
            .contains("../../../bootstrap/postgres/20260413020000_empty_database_snapshot.sql"),
        "lifecycle/bootstrap/postgres.rs should not read the outer bootstrap artifact directly"
    );

    let provider_catalog =
        read_workspace_file("crates/aether-data/adapters/postgres/src/provider_catalog.rs");
    assert!(
        !provider_catalog
            .contains("../../../bootstrap/postgres/20260413020000_empty_database_snapshot.sql"),
        "provider_catalog tests should use the shared bootstrap snapshot constant instead of the outer bootstrap artifact"
    );
}

#[test]
fn aether_data_backend_pool_modules_do_not_own_maintenance_sql() {
    for path in [
        "crates/aether-data/runtime/src/backend/postgres.rs",
        "crates/aether-data/runtime/src/backend/mysql.rs",
        "crates/aether-data/runtime/src/backend/sqlite.rs",
    ] {
        let source = read_workspace_file(path);
        let production = production_source(&source);
        for forbidden in [
            "run_table_maintenance(",
            "aggregate_wallet_daily_usage(",
            "aggregate_stats_hourly(",
            "aggregate_stats_daily(",
            "find_system_config_value(",
            "list_system_config_entries(",
            "upsert_system_config_entry(",
            "read_admin_system_stats(",
            "sqlx::query(",
            "sqlx::query_scalar",
            "sqlx::raw_sql(",
        ] {
            assert!(
                !production.contains(forbidden),
                "{path} should stay focused on pool and repository construction instead of owning maintenance SQL via {forbidden}"
            );
        }
    }

    let maintenance = read_workspace_file("crates/aether-data/runtime/src/backend/maintenance.rs");
    for pattern in [
        "Self::Postgres(postgres) => postgres.run_table_maintenance(table_names).await",
        "Self::Mysql(mysql) => mysql.run_table_maintenance(table_names).await",
        "Self::Sqlite(sqlite) => sqlite.run_table_maintenance(table_names).await",
        "Self::Postgres(postgres) => postgres.aggregate_wallet_daily_usage(input).await",
        "Self::Mysql(mysql) => mysql.aggregate_wallet_daily_usage(input).await",
        "Self::Sqlite(sqlite) => sqlite.aggregate_wallet_daily_usage(input).await",
        "Self::Postgres(postgres) => postgres.aggregate_stats_hourly(input).await",
        "Self::Mysql(mysql) => mysql.aggregate_stats_hourly(input).await",
        "Self::Sqlite(sqlite) => sqlite.aggregate_stats_hourly(input).await",
        "Self::Postgres(postgres) => postgres.aggregate_stats_daily(input).await",
        "Self::Mysql(mysql) => mysql.aggregate_stats_daily(input).await",
        "Self::Sqlite(sqlite) => sqlite.aggregate_stats_daily(input).await",
    ] {
        assert!(
            maintenance.contains(pattern),
            "backend/maintenance.rs should own SQL-driver maintenance dispatch {pattern}"
        );
    }
}

#[test]
fn wallet_maintenance_sql_is_partitioned_by_driver() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/backend/wallet.rs");
    for module in ["mod postgres;", "mod mysql;", "mod sqlite;"] {
        assert!(
            facade.contains(module),
            "wallet facade should declare {module}"
        );
    }
    for forbidden in [
        "sqlx::",
        "PostgresBackend",
        "MysqlBackend",
        "SqliteBackend",
        "SELECT ",
        "INSERT INTO ",
        "DELETE FROM ",
    ] {
        assert!(
            !facade.contains(forbidden),
            "wallet facade should not own driver implementation via {forbidden}"
        );
    }

    for (driver, backend) in [
        ("postgres", "PostgresBackend"),
        ("mysql", "MysqlBackend"),
        ("sqlite", "SqliteBackend"),
    ] {
        let path = format!("crates/aether-data/runtime/src/backend/wallet/{driver}.rs");
        let source = read_workspace_file(&path);
        assert!(source.contains(&format!("impl {backend}")));
        assert!(source.contains("aggregate_wallet_daily_usage"));
        assert!(source.contains("sqlx::query"));
    }
}

#[test]
fn table_maintenance_is_partitioned_for_each_driver() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/backend/maintenance.rs");
    for module in ["mod postgres;", "mod mysql;", "mod sqlite;"] {
        assert!(
            facade.contains(module),
            "maintenance facade should declare {module}"
        );
    }
    for forbidden in [
        "impl PostgresBackend",
        "impl MysqlBackend",
        "impl SqliteBackend",
        "VACUUM ANALYZE",
        "ANALYZE TABLE",
        "PRAGMA optimize",
        "sqlx::Row",
    ] {
        assert!(
            !facade.contains(forbidden),
            "maintenance dispatch should not own driver SQL via {forbidden}"
        );
    }

    let postgres =
        read_workspace_file("crates/aether-data/runtime/src/backend/maintenance/postgres.rs");
    assert!(postgres.contains("impl PostgresBackend"));
    assert!(postgres.contains("postgres_observability_snapshot"));
    assert!(postgres.contains("VACUUM ANALYZE"));

    let mysql = read_workspace_file("crates/aether-data/runtime/src/backend/maintenance/mysql.rs");
    assert!(mysql.contains("impl MysqlBackend"));
    assert!(mysql.contains("ANALYZE TABLE"));

    let sqlite =
        read_workspace_file("crates/aether-data/runtime/src/backend/maintenance/sqlite.rs");
    assert!(sqlite.contains("impl SqliteBackend"));
    assert!(sqlite.contains("PRAGMA optimize"));
}

#[test]
fn system_driver_database_operations_are_partitioned() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/backend/system.rs");
    for module in ["mod postgres;", "mod mysql;", "mod sqlite;"] {
        assert!(
            facade.contains(module),
            "system facade should declare {module}"
        );
    }
    for forbidden in [
        "impl PostgresBackend",
        "impl MysqlBackend",
        "impl SqliteBackend",
        "fn map_postgres_stats_daily_aggregate(",
        "fn map_postgres_stats_user_daily_aggregate(",
        "fn map_mysql_stats_daily_aggregate(",
        "fn map_mysql_stats_user_daily_aggregate(",
        "fn map_sqlite_stats_daily_aggregate(",
        "fn map_sqlite_stats_user_daily_aggregate(",
        "async fn export_postgres_admin_system_usage_aggregates(",
        "async fn export_mysql_admin_system_usage_aggregates(",
        "async fn export_sqlite_admin_system_usage_aggregates(",
        "async fn import_postgres_admin_system_usage_aggregates(",
        "async fn import_mysql_admin_system_usage_aggregates(",
        "async fn import_sqlite_admin_system_usage_aggregates(",
        "async fn pg_delete_table(",
        "async fn pg_execute_if_table(",
        "async fn pg_execute_batch_if_table(",
        "async fn pg_table_exists(",
        "async fn sqlite_delete_table(",
        "async fn sqlite_execute_if_table(",
        "async fn sqlite_execute_batch_if_table(",
        "async fn sqlite_table_exists(",
        "async fn mysql_delete_table(",
        "async fn mysql_execute_if_table(",
        "async fn mysql_execute_batch_if_table(",
        "async fn mysql_table_exists(",
        "async fn purge_postgres_admin_system_data(",
        "async fn purge_postgres_non_admin_users(",
        "async fn purge_postgres_request_bodies_batch(",
        "async fn purge_mysql_admin_system_data(",
        "async fn purge_mysql_non_admin_users(",
        "async fn purge_mysql_request_bodies_batch(",
        "async fn purge_sqlite_admin_system_data(",
        "async fn purge_sqlite_non_admin_users(",
        "async fn purge_sqlite_request_bodies_batch(",
    ] {
        assert!(
            !facade.contains(forbidden),
            "system facade should not own driver database operations via {forbidden}"
        );
    }

    for driver in ["postgres", "mysql", "sqlite"] {
        let source = read_workspace_file(&format!(
            "crates/aether-data/runtime/src/backend/system/{driver}.rs"
        ));
        let backend = match driver {
            "postgres" => "Postgres",
            "mysql" => "Mysql",
            "sqlite" => "Sqlite",
            _ => unreachable!(),
        };
        for required in [
            format!("impl {backend}Backend"),
            "map_stats_daily_aggregate".to_string(),
            "map_stats_user_daily_aggregate".to_string(),
            "map_stats_daily_api_key_aggregate".to_string(),
            "map_admin_system_stats".to_string(),
            format!("export_{driver}_admin_system_usage_aggregates"),
            format!("import_{driver}_admin_system_usage_aggregates"),
        ] {
            assert!(
                source.contains(&required),
                "system/{driver}.rs should own {required}"
            );
        }
        if driver == "postgres" {
            for required in [
                "pg_delete_table",
                "pg_execute_if_table",
                "pg_execute_batch_if_table",
                "pg_table_exists",
                "purge_postgres_admin_system_data",
                "purge_postgres_non_admin_users",
                "purge_postgres_request_bodies_batch",
            ] {
                assert!(
                    source.contains(required),
                    "system/postgres.rs should own {required}"
                );
            }
        }
        if driver == "sqlite" {
            for required in [
                "sqlite_delete_table",
                "sqlite_execute_if_table",
                "sqlite_execute_batch_if_table",
                "sqlite_table_exists",
                "purge_sqlite_admin_system_data",
                "purge_sqlite_non_admin_users",
                "purge_sqlite_request_bodies_batch",
            ] {
                assert!(
                    source.contains(required),
                    "system/sqlite.rs should own {required}"
                );
            }
        }
        if driver == "mysql" {
            for required in [
                "mysql_delete_table",
                "mysql_execute_if_table",
                "mysql_execute_batch_if_table",
                "mysql_table_exists",
                "purge_mysql_admin_system_data",
                "purge_mysql_non_admin_users",
                "purge_mysql_request_bodies_batch",
            ] {
                assert!(
                    source.contains(required),
                    "system/mysql.rs should own {required}"
                );
            }
        }
    }
}

#[test]
fn audit_repository_database_operations_are_partitioned() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/audit.rs");
    for module in ["mod types;", "mod tests;"] {
        assert!(
            facade.contains(module),
            "audit facade should declare {module}"
        );
    }
    for forbidden in [
        "sqlx::",
        "PostgresPool",
        "MysqlPool",
        "SqlitePool",
        "impl AuditLogReadRepository",
        "SELECT ",
        "DELETE FROM ",
    ] {
        assert!(
            !facade.contains(forbidden),
            "audit facade should not own driver database operations via {forbidden}"
        );
    }

    let types = read_workspace_file("crates/aether-data/contracts/src/repository/audit.rs");
    for required in [
        "pub trait AuditLogReadRepository",
        "pub struct StoredAdminAuditLog",
        "pub struct StoredSuspiciousActivity",
        "pub struct StoredUserAuditLog",
        "pub const SUSPICIOUS_EVENT_TYPES",
    ] {
        assert!(
            types.contains(required),
            "aether-data-contracts audit.rs should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !types.contains(forbidden),
            "aether-data-contracts audit.rs should remain driver-independent from {forbidden}"
        );
    }

    let request_types =
        read_workspace_file("crates/aether-data/runtime/src/repository/audit/types.rs");
    for required in [
        "pub trait RequestAuditReader",
        "pub struct RequestAuditBundle",
        "pub async fn read_request_audit_bundle",
    ] {
        assert!(
            request_types.contains(required),
            "aether-data request audit types should own {required}"
        );
    }

    for (driver, repository, row_mapper) in [
        (
            "postgres",
            "PostgresAuditLogReadRepository",
            "map_postgres_admin_audit_log_row",
        ),
        (
            "mysql",
            "MysqlAuditLogReadRepository",
            "map_mysql_admin_audit_log_row",
        ),
        (
            "sqlite",
            "SqliteAuditLogReadRepository",
            "map_sqlite_admin_audit_log_row",
        ),
    ] {
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{driver}/src/audit.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl AuditLogReadRepository for {repository}"),
            row_mapper.to_string(),
            "sqlx::query".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{driver}/src/audit.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/audit/{driver}.rs"
            )),
            "audit SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn auth_module_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/auth_modules/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "auth module facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/auth_modules.rs");
    for required in [
        "pub struct StoredOAuthProviderModuleConfig",
        "pub struct StoredLdapModuleConfig",
        "pub trait AuthModuleReadRepository",
        "pub trait AuthModuleWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "aether-data-contracts auth_modules.rs should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !contracts.contains(forbidden),
            "auth module contracts should remain driver-independent from {forbidden}"
        );
    }

    for (feature, adapter, read_repository, write_repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxAuthModuleReadRepository",
            "SqlxAuthModuleRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlAuthModuleReadRepository",
            "MysqlAuthModuleRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteAuthModuleReadRepository",
            "SqliteAuthModuleRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "auth module facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!(
                "pub use {adapter}::{{{read_repository}, {write_repository}}};"
            )),
            "auth module facade should re-export {feature} repositories from its adapter"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/auth_modules.rs"
        ));
        for required in [
            format!("pub struct {read_repository}"),
            format!("pub struct {write_repository}"),
            format!("impl AuthModuleReadRepository for {read_repository}"),
            format!("impl AuthModuleWriteRepository for {write_repository}"),
            "sqlx::query".to_string(),
            "ldap_configs".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/auth_modules.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/auth_modules/{feature}.rs"
            )),
            "auth module SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists("crates/aether-data/runtime/src/repository/auth_modules/types.rs"),
        "auth module contracts must not be duplicated in aether-data"
    );
}

#[test]
fn announcement_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/announcements/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "announcement facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/announcements.rs");
    for required in [
        "pub struct StoredAnnouncement",
        "pub struct AnnouncementListQuery",
        "pub struct CreateAnnouncementRecord",
        "pub struct UpdateAnnouncementRecord",
        "pub trait AnnouncementReadRepository",
        "pub trait AnnouncementWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "aether-data-contracts announcements.rs should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !contracts.contains(forbidden),
            "announcement contracts should remain driver-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxAnnouncementReadRepository",
        ),
        ("mysql", "aether_data_mysql", "MysqlAnnouncementRepository"),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteAnnouncementRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "announcement facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "announcement facade should re-export {feature} repository from its adapter"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/announcements.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl AnnouncementReadRepository for {repository}"),
            format!("impl AnnouncementWriteRepository for {repository}"),
            "sqlx::query".to_string(),
            "announcement_reads".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/announcements.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/announcements/{feature}.rs"
            )),
            "announcement SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists("crates/aether-data/runtime/src/repository/announcements/types.rs"),
        "announcement contracts must not be duplicated in aether-data"
    );
}

#[test]
fn oauth_provider_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/oauth_providers/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "oauth provider facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/oauth_providers.rs");
    for required in [
        "pub struct StoredOAuthProviderConfig",
        "pub enum EncryptedSecretUpdate",
        "pub struct UpsertOAuthProviderConfigRecord",
        "pub trait OAuthProviderReadRepository",
        "pub trait OAuthProviderWriteRepository",
        "pub trait OAuthProviderRepository",
    ] {
        assert!(
            contracts.contains(required),
            "aether-data-contracts oauth_providers.rs should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !contracts.contains(forbidden),
            "oauth provider contracts should remain driver-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxOAuthProviderRepository",
        ),
        ("mysql", "aether_data_mysql", "MysqlOAuthProviderRepository"),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteOAuthProviderRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "oauth provider facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "oauth provider facade should re-export {feature} repository from its adapter"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/oauth_providers.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl OAuthProviderReadRepository for {repository}"),
            format!("impl OAuthProviderWriteRepository for {repository}"),
            "oauth_providers".to_string(),
            "sqlx::query".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/oauth_providers.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/oauth_providers/{feature}.rs"
            )),
            "oauth provider SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists(
            "crates/aether-data/runtime/src/repository/oauth_providers/types.rs"
        ),
        "oauth provider contracts must not be duplicated in aether-data"
    );
}

#[test]
fn management_token_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/management_tokens/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "management token facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/management_tokens.rs");
    for required in [
        "pub struct StoredManagementToken",
        "pub struct StoredManagementTokenUserSummary",
        "pub struct CreateManagementTokenRecord",
        "pub struct UpdateManagementTokenRecord",
        "pub trait ManagementTokenReadRepository",
        "pub trait ManagementTokenWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "aether-data-contracts management_tokens.rs should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !contracts.contains(forbidden),
            "management token contracts should remain driver-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxManagementTokenRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlManagementTokenRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteManagementTokenRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "management token facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "management token facade should re-export {feature} repository from its adapter"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/management_tokens.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl ManagementTokenReadRepository for {repository}"),
            format!("impl ManagementTokenWriteRepository for {repository}"),
            "management_tokens".to_string(),
            "sqlx::query".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/management_tokens.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/management_tokens/{feature}.rs"
            )),
            "management token SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists(
            "crates/aether-data/runtime/src/repository/management_tokens/types.rs"
        ),
        "management token contracts must not be duplicated in aether-data"
    );
}

#[test]
fn gemini_file_mapping_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade = read_workspace_file(
        "crates/aether-data/runtime/src/repository/gemini_file_mappings/mod.rs",
    );
    for forbidden in [
        "sqlx::",
        "QueryBuilder",
        "PgPool",
        "MySqlPool",
        "SqlitePool",
    ] {
        assert!(
            !facade.contains(forbidden),
            "gemini file mapping facade should not own driver code via {forbidden}"
        );
    }
    assert!(
        facade.contains("pub use aether_data_contracts::repository::gemini_file_mappings::*;"),
        "legacy types path should remain a contracts-backed compatibility facade"
    );

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/gemini_file_mappings.rs");
    for required in [
        "pub struct StoredGeminiFileMapping",
        "pub struct GeminiFileMappingListQuery",
        "pub struct UpsertGeminiFileMappingRecord",
        "pub trait GeminiFileMappingReadRepository",
        "pub trait GeminiFileMappingWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "aether-data-contracts gemini_file_mappings.rs should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !contracts.contains(forbidden),
            "gemini file mapping contracts should remain driver-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxGeminiFileMappingRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlGeminiFileMappingRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteGeminiFileMappingRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "gemini file mapping facade should re-export {feature} repository from its adapter"
        );
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/gemini_file_mappings.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl GeminiFileMappingReadRepository for {repository}"),
            format!("impl GeminiFileMappingWriteRepository for {repository}"),
            "gemini_file_mappings".to_string(),
            "sqlx::query".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/gemini_file_mappings.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/gemini_file_mappings/{feature}.rs"
            )),
            "gemini file mapping SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists(
            "crates/aether-data/runtime/src/repository/gemini_file_mappings/types.rs"
        ),
        "gemini file mapping contracts must not be duplicated in aether-data"
    );
}

#[test]
fn video_task_repositories_are_owned_by_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/video_tasks/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "video task facade should not own driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/video_tasks/types.rs");
    for required in [
        "pub struct StoredVideoTask",
        "pub struct UpsertVideoTask",
        "pub trait VideoTaskReadRepository",
        "pub trait VideoTaskWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "aether-data-contracts video task types should own {required}"
        );
    }

    for (feature, adapter, repositories) in [
        (
            "postgres",
            "aether_data_postgres",
            vec!["SqlxVideoTaskReadRepository", "SqlxVideoTaskRepository"],
        ),
        (
            "mysql",
            "aether_data_mysql",
            vec!["MysqlVideoTaskRepository"],
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            vec!["SqliteVideoTaskRepository"],
        ),
    ] {
        for repository in repositories {
            assert!(
                facade.contains(repository) && facade.contains(adapter),
                "video task facade should re-export {feature} {repository} from its adapter"
            );
            let source = read_workspace_file(&format!(
                "crates/aether-data/adapters/{feature}/src/video_tasks.rs"
            ));
            assert!(
                source.contains(repository),
                "aether-data-{feature}/src/video_tasks.rs should own {repository}"
            );
            assert!(source.contains("sqlx::query"));
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/video_tasks/{feature}.rs"
            )),
            "video task SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn candidate_selection_repositories_are_owned_by_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/candidate_selection/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "PgPool",
        "MySqlPool",
        "SqlitePool",
    ] {
        assert!(
            !facade.contains(forbidden),
            "candidate selection facade should not own driver code via {forbidden}"
        );
    }

    let contracts = read_workspace_file(
        "crates/aether-data/contracts/src/repository/candidate_selection/types.rs",
    );
    for required in [
        "pub struct StoredMinimalCandidateSelectionRow",
        "pub trait MinimalCandidateSelectionReadRepository",
        "pub struct StoredPoolKeyCandidateRowsQuery",
        "pub struct StoredRequestedModelCandidateRowsQuery",
    ] {
        assert!(
            contracts.contains(required),
            "candidate selection contracts should own {required}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxMinimalCandidateSelectionReadRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlMinimalCandidateSelectionReadRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteMinimalCandidateSelectionReadRepository",
        ),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/candidate_selection.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl MinimalCandidateSelectionReadRepository for {repository}"),
            "provider_api_keys".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/candidate_selection.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/candidate_selection/{feature}.rs"
            )),
            "candidate selection SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn request_candidate_repositories_are_owned_by_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/candidates/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "PgPool",
        "MySqlPool",
        "SqlitePool",
    ] {
        assert!(
            !facade.contains(forbidden),
            "request candidate facade should not own driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/candidates/types.rs");
    for required in [
        "pub struct StoredRequestCandidate",
        "pub struct UpsertRequestCandidateRecord",
        "pub trait RequestCandidateReadRepository",
        "pub trait RequestCandidateWriteRepository",
        "pub fn request_candidate_lifecycle_would_regress",
    ] {
        assert!(
            contracts.contains(required),
            "request candidate contracts should own {required}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxRequestCandidateReadRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlRequestCandidateRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteRequestCandidateRepository",
        ),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/candidates.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl RequestCandidateReadRepository for {repository}"),
            format!("impl RequestCandidateWriteRepository for {repository}"),
            "request_candidates".to_string(),
            "sqlx::query".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/candidates.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/candidates/{feature}.rs"
            )),
            "request candidate SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn billing_repositories_are_owned_by_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/billing/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "billing facade should not own driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/billing/types.rs");
    for required in [
        "pub trait BillingReadRepository",
        "pub struct StoredBillingModelContext",
        "pub struct BillingPlanRecord",
        "pub struct AdminBillingRuleRecord",
        "pub struct PaymentGatewayConfigRecord",
    ] {
        assert!(
            contracts.contains(required),
            "billing contracts should own {required}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxBillingReadRepository",
        ),
        ("mysql", "aether_data_mysql", "MysqlBillingReadRepository"),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteBillingReadRepository",
        ),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/billing.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl BillingReadRepository for {repository}"),
            "billing_plans".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/billing.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/billing/{feature}.rs"
            )),
            "billing SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn settlement_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/settlement/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "settlement facade should not own driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/settlement/types.rs");
    for required in [
        "pub trait SettlementWriteRepository",
        "pub struct UsageSettlementInput",
        "pub struct StoredUsageSettlement",
        "pub struct WalletDebitPlan",
        "pub fn plan_finite_wallet_debit",
    ] {
        assert!(
            contracts.contains(required),
            "settlement contracts should own {required}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxSettlementRepository",
        ),
        ("mysql", "aether_data_mysql", "MysqlSettlementRepository"),
        ("sqlite", "aether_data_sqlite", "SqliteSettlementRepository"),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/settlement.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl SettlementWriteRepository for {repository}"),
            "usage_settlement_snapshots".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/settlement.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/settlement/{feature}.rs"
            )),
            "settlement SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn proxy_node_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/proxy_nodes/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "proxy node facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/proxy_nodes.rs");
    for required in [
        "pub struct StoredProxyNode",
        "pub struct ProxyNodeHeartbeatMutation",
        "pub struct StoredProxyNodeMetricsBucket",
        "pub trait ProxyNodeReadRepository",
        "pub trait ProxyNodeWriteRepository",
        "pub fn build_tunnel_metrics_sample",
    ] {
        assert!(
            contracts.contains(required),
            "proxy node contracts should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool", "tracing::"] {
        assert!(
            !contracts.contains(forbidden),
            "proxy node contracts should remain infrastructure-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxProxyNodeRepository",
        ),
        ("mysql", "aether_data_mysql", "MysqlProxyNodeReadRepository"),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteProxyNodeReadRepository",
        ),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/proxy_nodes.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl ProxyNodeReadRepository for {repository}"),
            format!("impl ProxyNodeWriteRepository for {repository}"),
            "proxy_node_metrics_1m".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/proxy_nodes.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/proxy_nodes/{feature}.rs"
            )),
            "proxy node SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists("crates/aether-data/runtime/src/repository/proxy_nodes/types.rs"),
        "proxy node contracts must not be duplicated in aether-data"
    );
}

#[test]
fn global_model_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/global_models/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
        "EMBEDDING_API_FORMATS",
    ] {
        assert!(
            !facade.contains(forbidden),
            "global model facade should not own driver/query policy via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/global_models/types.rs");
    for required in [
        "pub trait GlobalModelReadRepository",
        "pub trait GlobalModelWriteRepository",
        "pub struct StoredAdminGlobalModel",
        "pub fn metadata_supports_embedding",
    ] {
        assert!(
            contracts.contains(required),
            "global model contracts should own {required}"
        );
    }
    let snapshot = read_workspace_file(
        "crates/aether-data/contracts/src/repository/global_models/snapshot.rs",
    );
    for required in [
        "pub struct GlobalModelSnapshot",
        "pub fn list_public_models",
        "pub fn list_admin_global_models",
    ] {
        assert!(
            snapshot.contains(required),
            "global model snapshot policy should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool", "RwLock"] {
        assert!(
            !snapshot.contains(forbidden),
            "global model snapshot policy should remain infrastructure-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxGlobalModelReadRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlGlobalModelReadRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteGlobalModelReadRepository",
        ),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/global_models.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl GlobalModelReadRepository for {repository}"),
            format!("impl GlobalModelWriteRepository for {repository}"),
            "global_models".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/global_models.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/global_models/{feature}.rs"
            )),
            "global model SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn auth_api_key_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/auth/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "auth facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts = read_workspace_file("crates/aether-data/contracts/src/repository/auth.rs");
    for required in [
        "pub struct StoredAuthApiKeySnapshot",
        "pub struct ResolvedAuthApiKeySnapshot",
        "pub trait AuthApiKeyReadRepository",
        "pub trait AuthApiKeyWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "auth contracts should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !contracts.contains(forbidden),
            "auth contracts should remain infrastructure-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxAuthApiKeySnapshotReadRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlAuthApiKeyReadRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteAuthApiKeyReadRepository",
        ),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/auth.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl AuthApiKeyReadRepository for {repository}"),
            format!("impl AuthApiKeyWriteRepository for {repository}"),
            "api_keys".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/auth.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/auth/{feature}.rs"
            )),
            "auth SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists("crates/aether-data/runtime/src/repository/auth/types.rs"),
        "auth contracts must not be duplicated in aether-data"
    );
}

#[test]
fn user_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/users/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "users facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts = read_workspace_file("crates/aether-data/contracts/src/repository/users.rs");
    for required in [
        "pub struct StoredUserAuthRecord",
        "pub struct StoredUserGroup",
        "pub struct StoredUserSessionRecord",
        "pub trait UserReadRepository",
    ] {
        assert!(
            contracts.contains(required),
            "user contracts should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !contracts.contains(forbidden),
            "user contracts should remain infrastructure-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        ("postgres", "aether_data_postgres", "SqlxUserReadRepository"),
        ("mysql", "aether_data_mysql", "MysqlUserReadRepository"),
        ("sqlite", "aether_data_sqlite", "SqliteUserReadRepository"),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/users.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl UserReadRepository for {repository}"),
            "users".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/users.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/users/{feature}.rs"
            )),
            "user SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists("crates/aether-data/runtime/src/repository/users/types.rs"),
        "user contracts must not be duplicated in aether-data"
    );
}

#[test]
fn provider_catalog_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/provider_catalog/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "provider catalog facade should not own driver code via {forbidden}"
        );
    }

    let contracts = read_workspace_file(
        "crates/aether-data/contracts/src/repository/provider_catalog/types.rs",
    );
    for required in [
        "pub struct StoredProviderCatalogProvider",
        "pub struct StoredProviderCatalogKey",
        "pub trait ProviderCatalogReadRepository",
        "pub trait ProviderCatalogWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "provider catalog contracts should own {required}"
        );
    }
    let snapshot = read_workspace_file(
        "crates/aether-data/contracts/src/repository/provider_catalog/snapshot.rs",
    );
    for required in [
        "pub struct ProviderCatalogSnapshot",
        "pub fn list_providers",
        "pub fn list_keys_page",
    ] {
        assert!(
            snapshot.contains(required),
            "provider catalog snapshot policy should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool", "RwLock"] {
        assert!(
            !snapshot.contains(forbidden),
            "provider catalog snapshot should remain infrastructure-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxProviderCatalogReadRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlProviderCatalogReadRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteProviderCatalogReadRepository",
        ),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/provider_catalog.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl ProviderCatalogReadRepository for {repository}"),
            format!("impl ProviderCatalogWriteRepository for {repository}"),
            "provider_api_keys".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/provider_catalog.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/provider_catalog/{feature}.rs"
            )),
            "provider catalog SQL must be owned by adapter crates"
        );
    }
}

#[test]
fn wallet_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/wallet/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "wallet facade should not own contracts or driver code via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/wallet/types.rs");
    for required in [
        "pub struct StoredWalletSnapshot",
        "pub struct StoredAdminPaymentOrder",
        "pub trait WalletReadRepository",
        "pub trait WalletWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "wallet contracts should own {required}"
        );
    }
    let snapshot =
        read_workspace_file("crates/aether-data/contracts/src/repository/wallet/snapshot.rs");
    for required in [
        "pub struct WalletReadSeed",
        "pub struct WalletReadSnapshot",
        "pub fn list_admin_wallets",
    ] {
        assert!(
            snapshot.contains(required),
            "wallet snapshot policy should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool", "RwLock"] {
        assert!(
            !snapshot.contains(forbidden),
            "wallet snapshot should remain infrastructure-independent from {forbidden}"
        );
    }

    for (feature, adapter, repository) in [
        ("postgres", "aether_data_postgres", "SqlxWalletRepository"),
        ("mysql", "aether_data_mysql", "MysqlWalletReadRepository"),
        ("sqlite", "aether_data_sqlite", "SqliteWalletReadRepository"),
    ] {
        assert!(facade.contains(adapter) && facade.contains(repository));
        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/wallet.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl WalletReadRepository for {repository}"),
            format!("impl WalletWriteRepository for {repository}"),
            "wallets".to_string(),
            "sqlx::".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/wallet.rs should own {required}"
            );
        }
        assert!(
            !workspace_file_exists(&format!(
                "crates/aether-data/runtime/src/repository/wallet/{feature}.rs"
            )),
            "wallet SQL must be owned by adapter crates"
        );
    }
    assert!(
        !workspace_file_exists("crates/aether-data/runtime/src/repository/wallet/types.rs"),
        "wallet contracts must not be duplicated in aether-data"
    );
}

#[test]
fn usage_repositories_are_owned_by_contracts_and_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/usage/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
        "pub(crate) struct ApiKeyUsageDelta",
        "pub(crate) struct ProviderApiKeyUsageDelta",
    ] {
        assert!(
            !facade.contains(forbidden),
            "usage facade should not own driver or pure policy code via {forbidden}"
        );
    }
    let mysql_facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/usage/mysql.rs");
    assert!(mysql_facade.contains("aether_data_mysql::MysqlUsageStorage"));
    for forbidden in ["sqlx::query", "FROM `usage`", "INSERT INTO `usage`"] {
        assert!(
            !mysql_facade.contains(forbidden),
            "MySQL usage facade should only adapt shared read policy, not own SQL via {forbidden}"
        );
    }

    let contracts =
        read_workspace_file("crates/aether-data/contracts/src/repository/usage/types.rs");
    for required in [
        "pub struct StoredRequestUsageAudit",
        "pub struct UpsertUsageRecord",
        "pub trait UsageReadRepository",
        "pub trait UsageWriteRepository",
    ] {
        assert!(
            contracts.contains(required),
            "usage contracts should own {required}"
        );
    }
    let policy = read_workspace_file("crates/aether-data/contracts/src/repository/usage/policy.rs");
    for required in [
        "pub struct ApiKeyUsageDelta",
        "pub struct ProviderApiKeyUsageDelta",
        "pub fn usage_can_recover_terminal_failure",
        "pub fn strip_deprecated_usage_display_fields",
    ] {
        assert!(
            policy.contains(required),
            "usage policy should own {required}"
        );
    }
    for forbidden in ["sqlx::", "PgPool", "MySqlPool", "SqlitePool", "RwLock"] {
        assert!(
            !policy.contains(forbidden),
            "usage policy should remain infrastructure-independent from {forbidden}"
        );
    }

    let postgres = read_workspace_file("crates/aether-data/adapters/postgres/src/usage/mod.rs");
    for required in [
        "pub struct SqlxUsageReadRepository",
        "impl UsageReadRepository for SqlxUsageReadRepository",
        "impl UsageWriteRepository for SqlxUsageReadRepository",
        "sqlx::",
    ] {
        assert!(
            postgres.contains(required),
            "PostgreSQL usage adapter should own {required}"
        );
    }
    let mysql = read_workspace_file("crates/aether-data/adapters/mysql/src/usage.rs");
    for required in [
        "pub struct MysqlUsageStorage",
        "pub struct MysqlUsageWriteRepository",
        "impl UsageWriteRepository for MysqlUsageWriteRepository",
        "sqlx::",
    ] {
        assert!(
            mysql.contains(required),
            "MySQL usage adapter should own {required}"
        );
    }
    let sqlite = read_workspace_file("crates/aether-data/adapters/sqlite/src/usage.rs");
    for required in [
        "pub struct SqliteUsageReadRepository",
        "pub struct SqliteUsageWriteRepository",
        "impl UsageReadRepository for SqliteUsageReadRepository",
        "impl UsageWriteRepository for SqliteUsageWriteRepository",
        "sqlx::",
    ] {
        assert!(
            sqlite.contains(required),
            "SQLite usage adapter should own {required}"
        );
    }

    for path in [
        "crates/aether-data/runtime/src/repository/usage/postgres/mod.rs",
        "crates/aether-data/runtime/src/repository/usage/postgres/cleanup.rs",
        "crates/aether-data/runtime/src/repository/usage/sqlite.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "usage SQL must move out of {path}"
        );
    }
    assert_eq!(
        workspace_files_with_extension(
            "crates/aether-data/adapters/postgres/src/usage/queries",
            "sql"
        )
        .len(),
        26,
        "all PostgreSQL usage SQL fragments should be owned by the adapter crate"
    );
}

#[test]
fn lifecycle_backfills_are_partitioned_by_driver() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/lifecycle/backfill.rs");
    for module in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "mod tests;",
    ] {
        assert!(
            facade.contains(module),
            "backfill facade should declare {module}"
        );
    }
    for forbidden in [
        "sqlx::",
        "PgPool",
        "MysqlPool",
        "SqlitePool",
        "BACKFILL_MIGRATOR",
        "CREATE TABLE",
    ] {
        assert!(
            !facade.contains(forbidden),
            "backfill facade should not own driver operations via {forbidden}"
        );
    }

    let types = read_workspace_file("crates/aether-data/runtime/src/lifecycle/backfill/types.rs");
    assert!(types.contains("pub struct PendingBackfillInfo"));
    assert!(!types.contains("sqlx::"));

    let postgres =
        read_workspace_file("crates/aether-data/runtime/src/lifecycle/backfill/postgres.rs");
    for required in [
        "sqlx::migrate!",
        "pub async fn run_backfills",
        "pub async fn pending_backfills",
        "schema_backfills",
    ] {
        assert!(
            postgres.contains(required),
            "backfill/postgres.rs should own {required}"
        );
    }

    for (driver, pool) in [("mysql", "MysqlPool"), ("sqlite", "SqlitePool")] {
        let source = read_workspace_file(&format!(
            "crates/aether-data/runtime/src/lifecycle/backfill/{driver}.rs"
        ));
        for required in [
            format!("use crate::driver::{driver}::{pool}"),
            "pub async fn run_backfills".to_string(),
            "pub async fn pending_backfills".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "backfill/{driver}.rs should own {required}"
            );
        }
        for forbidden in ["PgPool", "BACKFILL_MIGRATOR", "schema_backfills"] {
            assert!(
                !source.contains(forbidden),
                "backfill/{driver}.rs should not depend on PostgreSQL via {forbidden}"
            );
        }
    }
}

#[test]
fn lifecycle_migrations_are_partitioned_by_driver() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/lifecycle/migrate.rs");
    for module in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "mod types;",
        "mod tests;",
    ] {
        assert!(
            facade.contains(module),
            "migration facade should declare {module}"
        );
    }
    for forbidden in [
        "POSTGRES_MIGRATOR",
        "PgConnection",
        "PgPool",
        "apply_snapshot_if_empty",
        "sqlx::migrate!",
    ] {
        assert!(
            !production_source(&facade).contains(forbidden),
            "migration facade should not own PostgreSQL execution via {forbidden}"
        );
    }

    let types = read_workspace_file("crates/aether-data/runtime/src/lifecycle/migrate/types.rs");
    let required = "pub use aether_data_contracts::PendingMigrationInfo";
    assert!(
        types.contains(required),
        "migrate/types.rs should own {required}"
    );
    for forbidden in ["PgPool", "MySqlPool", "SqlitePool"] {
        assert!(
            !types.contains(forbidden),
            "migrate/types.rs should remain driver-independent from {forbidden}"
        );
    }

    let postgres =
        read_workspace_file("crates/aether-data/runtime/src/lifecycle/migrate/postgres.rs");
    assert!(postgres.contains("aether_data_postgres"));
    assert!(postgres.contains("PgPool"));
    assert!(postgres.contains("pub async fn run_migrations"));
    assert!(postgres.contains("apply_snapshot_if_empty"));
    assert!(postgres.contains("pub async fn prepare_database_for_startup"));
    assert!(!postgres.contains("sqlx::migrate!"));
    let postgres_adapter =
        read_workspace_file("crates/aether-data/adapters/postgres/src/migrations.rs");
    assert!(postgres_adapter.contains("sqlx::migrate!(\"./migrations\")"));
    assert!(workspace_file_exists(
        "crates/aether-data/adapters/postgres/migrations/20260403000000_baseline.sql"
    ));
    for required in [
        "sqlx::migrate!",
        "PgPool",
        "pub async fn run_migrations",
        "pub async fn pending_migrations",
        "pub async fn run_migrations_with_bootstrap",
        "pub trait PostgresMigrationBootstrap",
    ] {
        assert!(
            postgres_adapter.contains(required),
            "crates/aether-data/adapters/postgres/src/migrations.rs should own {required}"
        );
    }

    for (driver, pool, adapter_module, adapter_dir) in [
        (
            "mysql",
            "MySqlPool",
            "aether_data_mysql",
            "aether-data-mysql",
        ),
        (
            "sqlite",
            "SqlitePool",
            "aether_data_sqlite",
            "aether-data-sqlite",
        ),
    ] {
        let source = read_workspace_file(&format!(
            "crates/aether-data/runtime/src/lifecycle/migrate/{driver}.rs"
        ));
        assert!(
            source.contains(&format!("pub(super) use {adapter_module}")),
            "migrate/{driver}.rs should remain an adapter compatibility facade"
        );
        assert!(!source.contains("sqlx::migrate!"));
        let adapter_source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{driver}/src/migrations.rs"
        ));
        assert!(adapter_source.contains("sqlx::migrate!(\"./migrations\")"));
        assert!(workspace_file_exists(&format!(
            "crates/aether-data/adapters/{driver}/migrations/20260403000000_baseline.sql"
        )));
        for required in [
            "sqlx::migrate!".to_string(),
            pool.to_string(),
            "pub async fn run_migrations".to_string(),
            "pub async fn pending_migrations".to_string(),
        ] {
            assert!(
                adapter_source.contains(&required),
                "{adapter_dir}/src/migrations.rs should own {required}"
            );
        }
    }
    assert!(
        !workspace_file_exists("crates/aether-data/runtime/migrations"),
        "driver migration SQL must be owned by adapter crates"
    );
}

#[test]
fn routing_profile_repositories_are_owned_by_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/routing_profiles/mod.rs");
    for forbidden in ["mod postgres;", "mod mysql;", "mod sqlite;", "sqlx::"] {
        assert!(
            !facade.contains(forbidden),
            "routing profile facade should not own driver implementation via {forbidden}"
        );
    }
    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "PostgresRoutingGroupRepository",
        ),
        ("mysql", "aether_data_mysql", "MysqlRoutingGroupRepository"),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteRoutingGroupRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "routing profile facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "routing profile facade should re-export {repository} from {adapter}"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/routing_profiles.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl RoutingGroupReadRepository for {repository}"),
            format!("impl RoutingGroupWriteRepository for {repository}"),
            "sqlx::query".to_string(),
            "FROM routing_groups".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/routing_profiles.rs should own {required}"
            );
        }
    }
}

#[test]
fn provider_quota_repositories_are_owned_by_driver_adapters() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/repository/quota/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "SelectQuery",
    ] {
        assert!(
            !facade.contains(forbidden),
            "provider quota facade should not own driver implementation via {forbidden}"
        );
    }
    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxProviderQuotaRepository",
        ),
        ("mysql", "aether_data_mysql", "MysqlProviderQuotaRepository"),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteProviderQuotaRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "provider quota facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "provider quota facade should re-export {repository} from {adapter}"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/quota.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl ProviderQuotaReadRepository for {repository}"),
            format!("impl ProviderQuotaWriteRepository for {repository}"),
            "sqlx::query".to_string(),
            "UPDATE providers".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/quota.rs should own {required}"
            );
        }
    }
}

#[test]
fn pool_score_repositories_are_owned_by_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/pool_scores/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "pool score facade should not own driver implementation via {forbidden}"
        );
    }
    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "PostgresPoolMemberScoreRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlPoolMemberScoreRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqlitePoolMemberScoreRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "pool score facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "pool score facade should re-export {repository} from {adapter}"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/pool_scores.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl PoolScoreReadRepository for {repository}"),
            format!("impl PoolMemberScoreWriteRepository for {repository}"),
            "sqlx::query".to_string(),
            "pool_member_scores".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/pool_scores.rs should own {required}"
            );
        }
    }
}

#[test]
fn background_task_repositories_are_owned_by_driver_adapters() {
    let facade =
        read_workspace_file("crates/aether-data/runtime/src/repository/background_tasks/mod.rs");
    for forbidden in [
        "mod postgres;",
        "mod mysql;",
        "mod sqlite;",
        "sqlx::",
        "QueryBuilder",
    ] {
        assert!(
            !facade.contains(forbidden),
            "background task facade should not own driver implementation via {forbidden}"
        );
    }
    for (feature, adapter, repository) in [
        (
            "postgres",
            "aether_data_postgres",
            "SqlxBackgroundTaskRepository",
        ),
        (
            "mysql",
            "aether_data_mysql",
            "MysqlBackgroundTaskRepository",
        ),
        (
            "sqlite",
            "aether_data_sqlite",
            "SqliteBackgroundTaskRepository",
        ),
    ] {
        assert!(
            facade.contains(&format!("#[cfg(feature = \"{feature}\")]")),
            "background task facade should preserve the {feature} feature boundary"
        );
        assert!(
            facade.contains(&format!("pub use {adapter}::{repository};")),
            "background task facade should re-export {repository} from {adapter}"
        );

        let source = read_workspace_file(&format!(
            "crates/aether-data/adapters/{feature}/src/background_tasks.rs"
        ));
        for required in [
            format!("pub struct {repository}"),
            format!("impl BackgroundTaskReadRepository for {repository}"),
            format!("impl BackgroundTaskWriteRepository for {repository}"),
            "sqlx::query".to_string(),
            "background_task_runs".to_string(),
            "background_task_events".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "aether-data-{feature}/src/background_tasks.rs should own {required}"
            );
        }
    }
}

#[test]
fn lifecycle_driver_export_operations_are_partitioned() {
    let facade = read_workspace_file("crates/aether-data/runtime/src/lifecycle/export.rs");
    for required in ["mod postgres;", "mod mysql;", "mod sqlite;", "mod tests;"] {
        assert!(
            facade.contains(required),
            "export facade should declare {required}"
        );
    }
    assert!(!facade.contains("mod tests {"));

    for (driver, implementation_marker) in [
        ("postgres", "fn normalize_postgres_import_payload("),
        ("mysql", "fn mysql_row_payload("),
        ("sqlite", "fn sqlite_row_payload("),
    ] {
        for required in [
            format!("pub use {driver}::"),
            format!("export_{driver}_core_jsonl"),
            format!("import_{driver}_plan"),
        ] {
            assert!(
                facade.contains(&required),
                "export facade should expose {driver} via {required}"
            );
        }
        for forbidden in [
            format!("pub async fn export_{driver}_core_jsonl("),
            format!("pub async fn export_{driver}_jsonl("),
            format!("pub async fn import_{driver}_jsonl("),
            format!("pub async fn import_{driver}_plan("),
            format!("fn {driver}_domain_table("),
            implementation_marker.to_string(),
        ] {
            assert!(
                !facade.contains(&forbidden),
                "export facade should not own {driver} implementation via {forbidden}"
            );
        }

        let source = read_workspace_file(&format!(
            "crates/aether-data/runtime/src/lifecycle/export/{driver}.rs"
        ));
        for required in [
            format!("pub async fn export_{driver}_core_jsonl("),
            format!("pub async fn export_{driver}_jsonl("),
            format!("pub async fn import_{driver}_jsonl("),
            format!("pub async fn import_{driver}_plan("),
            format!("fn {driver}_domain_table("),
            implementation_marker.to_string(),
            "sqlx::query".to_string(),
        ] {
            assert!(
                source.contains(&required),
                "export/{driver}.rs should own {required}"
            );
        }
    }

    let tests = read_workspace_file("crates/aether-data/runtime/src/lifecycle/export/tests.rs");
    assert!(tests.contains("jsonl_round_trips_manifest_and_domain_rows"));
    assert!(tests.contains("postgres_import_payload_normalizes_sqlite_values_for_target_columns"));
    assert!(tests.contains("sqlite_core_export_reads_migrated_database_rows"));
    assert!(tests.contains("mysql_core_export_reads_migrated_database_rows_when_url_is_set"));
}

#[test]
fn testkit_does_not_copy_aether_business_schema_sql() {
    let owner_relay_baseline = read_workspace_file(
        "crates/aether-testing/integration/src/bin/multi_instance_owner_relay_baseline.rs",
    );
    for forbidden in [
        "CREATE TYPE proxynodestatus",
        "CREATE TABLE IF NOT EXISTS system_configs",
        "CREATE TABLE IF NOT EXISTS proxy_nodes",
        "CREATE TABLE IF NOT EXISTS proxy_node_events",
        "PgConnection::connect",
        "sqlx::{Connection, Executor, PgConnection}",
    ] {
        assert!(
            !owner_relay_baseline.contains(forbidden),
            "owner relay baseline should use aether-data schema bootstrap instead of copying business schema SQL via {forbidden}"
        );
    }
    assert!(
        owner_relay_baseline.contains("prepare_aether_postgres_schema(&postgres_url).await?"),
        "owner relay baseline should prepare business schema through aether-testkit's aether-data helper"
    );

    let postgres_testkit = read_workspace_file("crates/aether-testing/testkit/src/postgres.rs");
    for required in [
        "pub async fn prepare_aether_postgres_schema",
        "DataBackends::from_config",
        ".prepare_database_for_startup()",
        ".run_database_migrations()",
    ] {
        assert!(
            postgres_testkit.contains(required),
            "shared Postgres helper should delegate Aether schema setup to aether-data via {required}"
        );
    }
}

#[test]
fn gateway_main_keeps_database_export_import_driver_selection_in_data_layer() {
    let main_rs = read_workspace_file("apps/aether-gateway/src/main.rs");
    for forbidden in [
        "PostgresPoolFactory",
        "MysqlPoolFactory",
        "SqlitePoolFactory",
        "to_postgres_config()",
    ] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs should delegate database export/import driver selection to aether-data instead of {forbidden}"
        );
    }
    for required in ["export_database_jsonl", "import_database_jsonl"] {
        assert!(
            main_rs.contains(required),
            "main.rs should use aether-data {required}"
        );
    }
}

#[test]
fn wallet_repository_does_not_reexport_settlement_types() {
    let wallet_mod = read_workspace_file("crates/aether-data/runtime/src/repository/wallet/mod.rs");
    let wallet_types =
        read_workspace_file("crates/aether-data/contracts/src/repository/wallet/types.rs");
    let wallet_sql = read_workspace_file("crates/aether-data/adapters/postgres/src/wallet.rs");
    let wallet_memory =
        read_workspace_file("crates/aether-data/runtime/src/repository/wallet/memory.rs");

    assert!(
        !wallet_mod.contains("StoredUsageSettlement"),
        "wallet/mod.rs should not export StoredUsageSettlement"
    );
    assert!(
        !wallet_mod.contains("UsageSettlementInput"),
        "wallet/mod.rs should not export UsageSettlementInput"
    );
    assert!(
        !wallet_types.contains("pub use crate::repository::settlement"),
        "wallet/types.rs should not re-export settlement types"
    );
    assert!(
        !wallet_types.contains("async fn settle_usage("),
        "wallet/types.rs should not own settlement entrypoints"
    );
    assert!(
        !wallet_sql.contains("impl SettlementWriteRepository"),
        "wallet/postgres.rs should not implement SettlementWriteRepository"
    );
    assert!(
        !wallet_memory.contains("impl SettlementWriteRepository"),
        "wallet/memory.rs should not implement SettlementWriteRepository"
    );
}

#[test]
fn gateway_system_config_types_are_owned_by_aether_data() {
    let state_mod = read_workspace_file("apps/aether-gateway/src/data/state/mod.rs");
    assert!(
        state_mod.contains("aether_data::repository::system"),
        "data/state/mod.rs should depend on aether-data system types"
    );
    assert!(
        !state_mod.contains("pub(crate) struct StoredSystemConfigEntry"),
        "data/state/mod.rs should not define StoredSystemConfigEntry locally"
    );

    let state_core = read_workspace_file("apps/aether-gateway/src/data/state/core.rs");
    for pattern in [
        "backends.list_system_config_entries().await",
        ".upsert_system_config_entry(key, value, description)",
        "backends.read_admin_system_stats().await",
        "AdminSystemStats::default()",
    ] {
        assert!(
            state_core.contains(pattern),
            "data/state/core.rs should use shared system DTO path {pattern}"
        );
    }
    let data_backends =
        read_workspace_file("crates/aether-data/runtime/src/backend/maintenance.rs");
    for pattern in [
        "postgres.list_system_config_entries().await",
        "mysql.list_system_config_entries().await",
        "sqlite.list_system_config_entries().await",
    ] {
        assert!(
            data_backends.contains(pattern),
            "aether-data backends should own driver-specific system config dispatch {pattern}"
        );
    }
    for pattern in [
        "|(key, value, description, updated_at_unix_secs)|",
        "Ok((0, 0, 0, 0))",
    ] {
        assert!(
            !state_core.contains(pattern),
            "data/state/core.rs should not own local system DTO projection {pattern}"
        );
    }

    let system_types = read_workspace_file("crates/aether-data/runtime/src/repository/system.rs");
    for pattern in [
        "pub struct StoredSystemConfigEntry",
        "pub struct AdminSystemStats",
        "pub struct AdminSecurityBlacklistEntry",
    ] {
        assert!(
            system_types.contains(pattern),
            "aether-data system module should own {pattern}"
        );
    }

    let admin_types = read_workspace_file("apps/aether-gateway/src/state/admin_types.rs");
    assert!(
        admin_types.contains("aether_data::repository::system::AdminSecurityBlacklistEntry"),
        "state/admin_types.rs should re-export AdminSecurityBlacklistEntry from aether-data"
    );
    assert!(
        !admin_types.contains("struct AdminSecurityBlacklistEntry"),
        "state/admin_types.rs should not define AdminSecurityBlacklistEntry locally"
    );

    let runtime_mod = read_workspace_file("apps/aether-gateway/src/state/runtime/mod.rs");
    assert!(
        !runtime_mod.contains("AdminSecurityBlacklistEntryPayload"),
        "state/runtime/mod.rs should not keep the unused blacklist payload wrapper"
    );
}

#[test]
fn gateway_auth_snapshot_type_is_owned_by_aether_data() {
    let gateway_auth = read_workspace_file("apps/aether-gateway/src/data/auth.rs");
    let runtime_mod = read_workspace_file("apps/aether-gateway/src/state/runtime/mod.rs");
    let auth_api_keys =
        read_workspace_file("apps/aether-gateway/src/state/runtime/auth/api_keys.rs");
    assert!(
        gateway_auth.contains("aether_data::repository::auth"),
        "data/auth.rs should depend on aether-data auth snapshot types"
    );
    assert!(
        gateway_auth.contains("ResolvedAuthApiKeySnapshot as GatewayAuthApiKeySnapshot"),
        "data/auth.rs should expose the shared resolved auth snapshot type under the gateway-facing name"
    );
    for pattern in [
        "pub(crate) struct GatewayAuthApiKeySnapshot",
        "pub(crate) async fn read_auth_api_key_snapshot(",
        "pub(crate) async fn read_auth_api_key_snapshot_by_key_hash(",
        "fn effective_allowed_providers(",
        "fn effective_allowed_api_formats(",
        "fn effective_allowed_models(",
    ] {
        assert!(
            !gateway_auth.contains(pattern),
            "data/auth.rs should not own local auth snapshot logic {pattern}"
        );
    }
    for pattern in [
        "pub(crate) async fn read_auth_api_key_snapshot(",
        "pub(crate) async fn read_auth_api_key_snapshots_by_ids(",
    ] {
        assert!(
            !auth_api_keys.contains(pattern),
            "state/runtime/auth/api_keys.rs should not keep auth snapshot read wrapper {pattern}"
        );
    }
    assert!(
        !runtime_mod.contains("mod audit;"),
        "state/runtime/mod.rs should not keep the obsolete audit runtime module"
    );
    assert!(
        auth_api_keys.contains("touch_auth_api_key_last_used_best_effort"),
        "state/runtime/auth/api_keys.rs should own auth api key last_used touch helper"
    );
    assert!(
        !auth_api_keys.contains("fn has_auth_api_key_writer("),
        "state/runtime/auth/api_keys.rs should not keep auth api key writer passthrough"
    );

    let auth_types = read_workspace_file("crates/aether-data/contracts/src/repository/auth.rs");
    for pattern in [
        "pub struct ResolvedAuthApiKeySnapshot",
        "pub trait ResolvedAuthApiKeySnapshotReader",
        "pub async fn read_resolved_auth_api_key_snapshot(",
        "pub async fn read_resolved_auth_api_key_snapshot_by_key_hash(",
        "pub async fn read_resolved_auth_api_key_snapshot_by_user_api_key_ids(",
        "pub fn effective_allowed_providers(&self)",
        "pub fn effective_allowed_api_formats(&self)",
        "pub fn effective_allowed_models(&self)",
    ] {
        assert!(
            auth_types.contains(pattern),
            "aether-data auth types should own {pattern}"
        );
    }
}

#[test]
fn gateway_auth_data_layer_does_not_keep_ldap_row_wrapper() {
    let gateway_auth_state = read_workspace_file("apps/aether-gateway/src/data/state/auth.rs");
    for pattern in [
        "struct StoredLdapAuthUserRow",
        "fn map_ldap_user_auth_row(",
        "Result<Option<StoredLdapAuthUserRow>, DataLayerError>",
        "existing.user.",
        "map_user_auth_row(row)",
    ] {
        assert!(
            !gateway_auth_state.contains(pattern),
            "data/state/auth.rs should not keep ldap row wrapper {pattern}"
        );
    }

    let user_sql = read_workspace_file("crates/aether-data/adapters/postgres/src/users.rs");
    for pattern in [
        "Result<Option<StoredUserAuthRecord>, DataLayerError>",
        "return map_user_auth_row(row).map(Some);",
    ] {
        assert!(
            user_sql.contains(pattern),
            "aether-data user repository should use shared user auth record directly via {pattern}"
        );
    }
}

#[test]
fn gateway_provider_oauth_storage_types_are_owned_by_aether_data() {
    let provider_oauth_storage = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/state/storage.rs",
    );
    let request_provider_oauth =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/request/provider/oauth.rs");
    assert!(
        request_provider_oauth.contains("aether_data::repository::provider_oauth"),
        "request/provider/oauth.rs should depend on aether-data provider oauth storage types"
    );
    for pattern in [
        "pub(crate) struct StoredAdminProviderOAuthDeviceSession",
        "pub(crate) struct StoredAdminProviderOAuthState",
        "const KIRO_DEVICE_AUTH_SESSION_PREFIX",
        "fn provider_oauth_device_session_key(",
        "fn build_provider_oauth_batch_task_status_payload(",
        "fn provider_oauth_batch_task_key(",
        "const PROVIDER_OAUTH_BATCH_TASK_TTL_SECS",
        "format!(\"provider_oauth_state:{nonce}\")",
    ] {
        assert!(
            !provider_oauth_storage.contains(pattern),
            "provider_oauth/state/storage.rs should not own local storage helper {pattern}"
        );
    }
    for pattern in [
        "StoredAdminProviderOAuthDeviceSession",
        "StoredAdminProviderOAuthState",
        "provider_oauth_batch_task_storage_key",
        "build_provider_oauth_batch_task_status_payload",
        "PROVIDER_OAUTH_BATCH_TASK_TTL_SECS",
    ] {
        assert!(
            request_provider_oauth.contains(pattern),
            "request/provider/oauth.rs should own aether-data provider oauth storage boundary {pattern}"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/provider/oauth/state.rs"),
        "provider_oauth/state.rs should not exist after oauth storage helpers move under state/storage.rs"
    );

    let dispatch_device_authorize = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device/authorize.rs",
    );
    assert!(
        dispatch_device_authorize.contains("aether_data::repository::provider_oauth"),
        "provider_oauth/dispatch/device/authorize.rs should use shared provider oauth storage DTOs"
    );
    assert!(
        !workspace_file_exists(
            "apps/aether-gateway/src/handlers/admin/provider/oauth/dispatch/device.rs"
        ),
        "provider_oauth/dispatch/device.rs should be removed once device flows move under dispatch/device/"
    );

    let shared_provider_oauth =
        read_workspace_file("crates/aether-data/runtime/src/repository/provider_oauth.rs");
    for pattern in [
        "pub struct StoredAdminProviderOAuthDeviceSession",
        "pub struct StoredAdminProviderOAuthState",
        "pub fn provider_oauth_device_session_storage_key(",
        "pub fn provider_oauth_state_storage_key(",
        "pub fn provider_oauth_batch_task_storage_key(",
        "pub fn build_provider_oauth_batch_task_status_payload(",
        "pub const KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS: u64 = 60;",
        "pub const PROVIDER_OAUTH_BATCH_TASK_TTL_SECS: u64 = 24 * 60 * 60;",
        "pub const PROVIDER_OAUTH_STATE_TTL_SECS: u64 = 600;",
    ] {
        assert!(
            shared_provider_oauth.contains(pattern),
            "aether-data provider oauth storage module should own {pattern}"
        );
    }
}
