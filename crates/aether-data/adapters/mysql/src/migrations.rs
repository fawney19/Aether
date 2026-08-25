use sqlx::{
    migrate::{AppliedMigration, Migrate, MigrateError, Migrator},
    MySqlPool,
};

use aether_data_contracts::PendingMigrationInfo;

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub async fn run_migrations(pool: &MySqlPool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}

pub async fn pending_migrations(
    pool: &MySqlPool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    let mut conn = pool.acquire().await?;
    let applied_migrations = match conn.list_applied_migrations().await {
        Ok(applied_migrations) => applied_migrations,
        Err(err) if is_missing_sqlx_migrations_table_error(&err) => {
            return Ok(pending_migrations_from_applied(&[]));
        }
        Err(err) => return Err(err),
    };
    if let Some(version) = conn.dirty_version().await? {
        return Err(MigrateError::Dirty(version));
    }
    validate_applied_migrations(&applied_migrations)?;
    Ok(pending_migrations_from_applied(&applied_migrations))
}

pub async fn prepare_database_for_startup(
    pool: &MySqlPool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    pending_migrations(pool).await
}

fn is_missing_sqlx_migrations_table_error(err: &MigrateError) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("_sqlx_migrations")
        && (message.contains("no such table")
            || message.contains("doesn't exist")
            || message.contains("does not exist")
            || message.contains("unknown table"))
}

fn pending_migrations_from_applied(
    applied_migrations: &[sqlx::migrate::AppliedMigration],
) -> Vec<PendingMigrationInfo> {
    let applied_versions = applied_migrations
        .iter()
        .map(|migration| migration.version)
        .collect::<std::collections::HashSet<_>>();
    MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| !applied_versions.contains(&migration.version))
        .map(|migration| PendingMigrationInfo {
            version: migration.version,
            description: migration.description.to_string(),
        })
        .collect()
}

fn validate_applied_migrations(
    applied_migrations: &[AppliedMigration],
) -> Result<(), MigrateError> {
    if MIGRATOR.ignore_missing {
        return Ok(());
    }
    let known_versions = MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .collect::<std::collections::HashSet<_>>();
    if let Some(migration) = applied_migrations
        .iter()
        .find(|migration| !known_versions.contains(&migration.version))
    {
        return Err(MigrateError::VersionMissing(migration.version));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{
        pending_migrations, prepare_database_for_startup, validate_applied_migrations, MIGRATOR,
    };
    use sqlx::migrate::{AppliedMigration, MigrateError};

    #[test]
    fn embeds_mysql_migration_sources() {
        let versions = MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>();
        assert!(!versions.is_empty());
        assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn embeds_aggregate_id_width_migration() {
        assert!(MIGRATOR
            .iter()
            .any(|migration| migration.version == 20260825000000));
    }

    #[test]
    fn embeds_scoped_codex_live_permission_migration() {
        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260821000000)
            .expect("Codex Live permission migration should be embedded");
        let sql = migration.sql.as_ref();

        for required_fragment in [
            "UPDATE users",
            "UPDATE user_groups",
            "UPDATE api_keys",
            "UPDATE provider_api_keys",
            "provider.provider_type",
            "openai:responses",
            "codex:live",
        ] {
            assert!(
                sql.contains(required_fragment),
                "Codex Live permission migration is missing {required_fragment}"
            );
        }
    }

    #[test]
    fn embeds_cross_driver_schema_parity_migration() {
        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260725010000)
            .expect("cross-driver schema parity migration should be embedded");
        let sql = migration.sql.as_ref();

        for required_fragment in [
            "CREATE TABLE IF NOT EXISTS usage_body_blobs",
            "CREATE TABLE IF NOT EXISTS usage_http_audits",
            "CREATE TABLE IF NOT EXISTS stats_summary",
            "CREATE TABLE IF NOT EXISTS user_model_usage_counts",
            "CREATE TABLE IF NOT EXISTS api_key_provider_mappings",
            "CREATE TABLE IF NOT EXISTS provider_usage_tracking",
            "ADD COLUMN `settlement_snapshot_schema_version`",
            "ADD COLUMN `billing_effective_input_tokens`",
            "ADD COLUMN `converted_request_body`",
            "ADD COLUMN `p99_first_byte_time_ms`",
            "idx_usage_stale_pending_created_request",
        ] {
            assert!(
                sql.contains(required_fragment),
                "parity migration is missing {required_fragment}"
            );
        }
    }

    #[test]
    fn embeds_advanced_stats_parity_migration() {
        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260725020000)
            .expect("advanced stats parity migration should be embedded");
        let sql = migration.sql.as_ref();

        for required_fragment in [
            "CREATE TABLE stats_user_summary",
            "CREATE TABLE stats_user_daily_api_format",
            "CREATE TABLE stats_user_daily_model_provider",
            "CREATE TABLE stats_daily_model_provider",
            "CREATE TABLE stats_daily_cost_savings",
            "CREATE TABLE stats_user_daily_cost_savings_model_provider",
            "ADD COLUMN completed_total_input_context",
            "ADD COLUMN settled_total_cost",
            "ADD COLUMN response_time_samples",
            "UPDATE stats_hourly SET is_complete = 0",
            "UPDATE stats_daily SET is_complete = 0",
        ] {
            assert!(
                sql.contains(required_fragment),
                "advanced stats migration is missing {required_fragment}"
            );
        }
    }

    #[test]
    fn rejects_applied_migration_versions_unknown_to_this_binary() {
        let version = MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .max()
            .expect("mysql migrations should not be empty")
            + 1;
        let error = validate_applied_migrations(&[AppliedMigration {
            version,
            checksum: Cow::Borrowed(&[]),
        }])
        .expect_err("unknown applied migration should block startup");

        assert!(matches!(error, MigrateError::VersionMissing(found) if found == version));
    }

    #[tokio::test]
    async fn pending_and_startup_preparation_reject_dirty_mysql_migration_state_when_url_is_set() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!("skipping mysql dirty migration test because AETHER_TEST_MYSQL_URL is unset");
            return;
        };

        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("mysql test pool should connect");
        let dirty_version = MIGRATOR
            .iter()
            .next()
            .expect("mysql migrations should not be empty")
            .version;

        let mut conn = pool
            .acquire()
            .await
            .expect("mysql connection should acquire");
        sqlx::query(
            r#"
CREATE TEMPORARY TABLE _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
)
"#,
        )
        .execute(&mut *conn)
        .await
        .expect("temporary mysql migrations table should create");
        sqlx::query(
            r#"
INSERT INTO _sqlx_migrations (
    version,
    description,
    success,
    checksum,
    execution_time
) VALUES (?, 'dirty test migration', FALSE, ?, 0)
"#,
        )
        .bind(dirty_version)
        .bind(Vec::<u8>::new())
        .execute(&mut *conn)
        .await
        .expect("dirty mysql migration should insert");
        drop(conn);

        let pending_error = pending_migrations(&pool)
            .await
            .expect_err("dirty mysql migration should fail pending inspection");
        assert!(
            matches!(&pending_error, MigrateError::Dirty(version) if *version == dirty_version),
            "unexpected pending migration error: {pending_error}"
        );

        let preparation_error = prepare_database_for_startup(&pool)
            .await
            .expect_err("dirty mysql migration should fail startup preparation");
        assert!(
            matches!(&preparation_error, MigrateError::Dirty(version) if *version == dirty_version),
            "unexpected startup preparation error: {preparation_error}"
        );
    }
}
