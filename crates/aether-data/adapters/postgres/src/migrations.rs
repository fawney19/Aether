use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

use sqlx::{
    migrate::{AppliedMigration, Migrate, MigrateError, Migrator},
    query, query_scalar, PgConnection, PgPool,
};
use tracing::{error, info, warn};

use aether_data_contracts::PendingMigrationInfo;

pub static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const MIGRATIONS_TABLE_EXISTS_SQL: &str =
    "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL";
const USAGE_LEGACY_BODY_REF_CLEANUP_INDEX_MIGRATION_VERSION: i64 = 20260715000000;
const USAGE_SETTLEMENT_DASHBOARD_INDEX_MIGRATION_VERSION: i64 = 20260715130000;
const USAGE_STALE_PENDING_CLEANUP_INDEX_MIGRATION_VERSION: i64 = 20260720000000;
const INVALID_USAGE_LEGACY_BODY_REF_CLEANUP_INDEX_EXISTS_SQL: &str = r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_class AS index_relation
    JOIN pg_catalog.pg_namespace AS index_namespace
      ON index_namespace.oid = index_relation.relnamespace
    JOIN pg_catalog.pg_index AS index_state
      ON index_state.indexrelid = index_relation.oid
    WHERE index_namespace.nspname = 'public'
      AND index_relation.relname = 'idx_usage_legacy_body_ref_cleanup_created_at'
      AND NOT index_state.indisvalid
)
"#;
const DROP_USAGE_LEGACY_BODY_REF_CLEANUP_INDEX_SQL: &str =
    "DROP INDEX CONCURRENTLY IF EXISTS public.idx_usage_legacy_body_ref_cleanup_created_at";
const INVALID_USAGE_SETTLEMENT_DASHBOARD_INDEX_EXISTS_SQL: &str = r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_class AS index_relation
    JOIN pg_catalog.pg_namespace AS index_namespace
      ON index_namespace.oid = index_relation.relnamespace
    JOIN pg_catalog.pg_index AS index_state
      ON index_state.indexrelid = index_relation.oid
    WHERE index_namespace.nspname = 'public'
      AND index_relation.relname = 'idx_usage_settlement_dashboard_cover'
      AND NOT index_state.indisvalid
)
"#;
const DROP_USAGE_SETTLEMENT_DASHBOARD_INDEX_SQL: &str =
    "DROP INDEX CONCURRENTLY IF EXISTS public.idx_usage_settlement_dashboard_cover";
const INVALID_USAGE_STALE_PENDING_CLEANUP_INDEX_EXISTS_SQL: &str = r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_class AS index_relation
    JOIN pg_catalog.pg_namespace AS index_namespace
      ON index_namespace.oid = index_relation.relnamespace
    JOIN pg_catalog.pg_index AS index_state
      ON index_state.indexrelid = index_relation.oid
    WHERE index_namespace.nspname = 'public'
      AND index_relation.relname = 'idx_usage_stale_pending_created_request'
      AND NOT index_state.indisvalid
)
"#;
const DROP_USAGE_STALE_PENDING_CLEANUP_INDEX_SQL: &str =
    "DROP INDEX CONCURRENTLY IF EXISTS public.idx_usage_stale_pending_created_request";

pub type BootstrapFuture<'a> = Pin<Box<dyn Future<Output = Result<(), MigrateError>> + 'a>>;

pub trait PostgresMigrationBootstrap: Send + Sync {
    fn apply_snapshot<'a>(
        &self,
        conn: &'a mut PgConnection,
        migrator: &'static Migrator,
    ) -> BootstrapFuture<'a>;
}

#[derive(Debug, Clone, Copy, Default)]
struct NoopBootstrap;

impl PostgresMigrationBootstrap for NoopBootstrap {
    fn apply_snapshot<'a>(
        &self,
        _conn: &'a mut PgConnection,
        _migrator: &'static Migrator,
    ) -> BootstrapFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    run_migrations_with_bootstrap(pool, &NoopBootstrap).await
}

pub async fn run_migrations_with_bootstrap(
    pool: &PgPool,
    bootstrap: &dyn PostgresMigrationBootstrap,
) -> Result<(), MigrateError> {
    let mut conn = pool.acquire().await?;

    if POSTGRES_MIGRATOR.locking {
        conn.lock().await?;
    }

    let result = run_migrations_locked(&mut conn, bootstrap).await;

    if POSTGRES_MIGRATOR.locking {
        match conn.unlock().await {
            Ok(()) => {}
            Err(unlock_error) if result.is_ok() => return Err(unlock_error),
            Err(unlock_error) => {
                warn!(
                    error = %unlock_error,
                    "database migration lock release failed after migration error"
                );
            }
        }
    }

    result
}

pub async fn pending_migrations(pool: &PgPool) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    let mut conn = pool.acquire().await?;
    pending_migrations_locked(&mut conn).await
}

pub async fn prepare_database_for_startup(
    pool: &PgPool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    prepare_database_for_startup_with_bootstrap(pool, &NoopBootstrap).await
}

pub async fn prepare_database_for_startup_with_bootstrap(
    pool: &PgPool,
    bootstrap: &dyn PostgresMigrationBootstrap,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    let mut conn = pool.acquire().await?;

    if POSTGRES_MIGRATOR.locking {
        conn.lock().await?;
    }

    let result = prepare_database_for_startup_locked(&mut conn, bootstrap).await;

    if POSTGRES_MIGRATOR.locking {
        match conn.unlock().await {
            Ok(()) => {}
            Err(unlock_error) if result.is_ok() => return Err(unlock_error),
            Err(unlock_error) => {
                warn!(
                    error = %unlock_error,
                    "database migration lock release failed after startup preparation error"
                );
            }
        }
    }

    result
}

async fn run_migrations_locked(
    conn: &mut PgConnection,
    bootstrap: &dyn PostgresMigrationBootstrap,
) -> Result<(), MigrateError> {
    conn.ensure_migrations_table().await?;
    bootstrap.apply_snapshot(conn, &POSTGRES_MIGRATOR).await?;

    if let Some(version) = conn.dirty_version().await? {
        error!(version, "database migration state is dirty");
        return Err(MigrateError::Dirty(version));
    }

    let applied_migrations = conn.list_applied_migrations().await?;
    validate_applied_migrations(&applied_migrations)?;
    let applied_migrations_by_version = applied_migrations
        .into_iter()
        .map(|migration| (migration.version, migration))
        .collect::<HashMap<_, _>>();
    let pending_migrations = POSTGRES_MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| !applied_migrations_by_version.contains_key(&migration.version))
        .collect::<Vec<_>>();
    let total_migrations = POSTGRES_MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .count();
    let applied_count = total_migrations.saturating_sub(pending_migrations.len());

    if pending_migrations.is_empty() {
        info!(
            total_migrations,
            applied_migrations = applied_count,
            pending_migrations = 0,
            "database migrations already up to date"
        );
        return Ok(());
    }

    info!(
        total_migrations,
        applied_migrations = applied_count,
        pending_migrations = pending_migrations.len(),
        "database migrations pending"
    );
    for (index, migration) in pending_migrations.iter().enumerate() {
        let current = index + 1;
        info!(
            current,
            total = pending_migrations.len(),
            version = migration.version,
            description = %migration.description,
            "applying database migration"
        );
        repair_invalid_concurrent_index(conn, migration.version).await?;
        let elapsed = conn.apply(migration).await?;
        info!(
            current,
            total = pending_migrations.len(),
            version = migration.version,
            description = %migration.description,
            elapsed_ms = elapsed.as_millis() as u64,
            "applied database migration"
        );
    }
    info!(
        total_migrations,
        applied_migrations = total_migrations,
        pending_migrations = 0,
        "database migrations complete"
    );
    Ok(())
}

async fn repair_invalid_concurrent_index(
    conn: &mut PgConnection,
    migration_version: i64,
) -> Result<(), MigrateError> {
    let (index_name, invalid_index_exists_sql, drop_index_sql) = match migration_version {
        USAGE_LEGACY_BODY_REF_CLEANUP_INDEX_MIGRATION_VERSION => (
            "idx_usage_legacy_body_ref_cleanup_created_at",
            INVALID_USAGE_LEGACY_BODY_REF_CLEANUP_INDEX_EXISTS_SQL,
            DROP_USAGE_LEGACY_BODY_REF_CLEANUP_INDEX_SQL,
        ),
        USAGE_SETTLEMENT_DASHBOARD_INDEX_MIGRATION_VERSION => (
            "idx_usage_settlement_dashboard_cover",
            INVALID_USAGE_SETTLEMENT_DASHBOARD_INDEX_EXISTS_SQL,
            DROP_USAGE_SETTLEMENT_DASHBOARD_INDEX_SQL,
        ),
        USAGE_STALE_PENDING_CLEANUP_INDEX_MIGRATION_VERSION => (
            "idx_usage_stale_pending_created_request",
            INVALID_USAGE_STALE_PENDING_CLEANUP_INDEX_EXISTS_SQL,
            DROP_USAGE_STALE_PENDING_CLEANUP_INDEX_SQL,
        ),
        _ => return Ok(()),
    };

    let invalid_index_exists: bool = query_scalar(invalid_index_exists_sql)
        .fetch_one(&mut *conn)
        .await?;
    if !invalid_index_exists {
        return Ok(());
    }

    warn!(
        migration_version,
        index = index_name,
        "dropping invalid index left by an interrupted concurrent migration"
    );
    query(drop_index_sql).execute(&mut *conn).await?;
    Ok(())
}

async fn prepare_database_for_startup_locked(
    conn: &mut PgConnection,
    bootstrap: &dyn PostgresMigrationBootstrap,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    conn.ensure_migrations_table().await?;
    bootstrap.apply_snapshot(conn, &POSTGRES_MIGRATOR).await?;
    pending_migrations_locked(conn).await
}

async fn pending_migrations_locked(
    conn: &mut PgConnection,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    if !migrations_table_exists(conn).await? {
        return Ok(all_up_migrations());
    }
    if let Some(version) = conn.dirty_version().await? {
        error!(version, "database migration state is dirty");
        return Err(MigrateError::Dirty(version));
    }
    let applied_migrations = conn.list_applied_migrations().await?;
    validate_applied_migrations(&applied_migrations)?;
    Ok(pending_migrations_from_applied(&applied_migrations))
}

async fn migrations_table_exists(conn: &mut PgConnection) -> Result<bool, MigrateError> {
    Ok(query_scalar(MIGRATIONS_TABLE_EXISTS_SQL)
        .fetch_one(&mut *conn)
        .await?)
}

pub fn all_up_migrations() -> Vec<PendingMigrationInfo> {
    POSTGRES_MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| PendingMigrationInfo {
            version: migration.version,
            description: migration.description.to_string(),
        })
        .collect()
}

pub fn pending_migrations_from_applied(
    applied_migrations: &[AppliedMigration],
) -> Vec<PendingMigrationInfo> {
    let applied_versions = applied_migrations
        .iter()
        .map(|migration| migration.version)
        .collect::<HashSet<_>>();
    POSTGRES_MIGRATOR
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
    if POSTGRES_MIGRATOR.ignore_missing {
        return Ok(());
    }
    let known_versions = POSTGRES_MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .collect::<HashSet<_>>();
    for applied_migration in applied_migrations {
        if !known_versions.contains(&applied_migration.version) {
            error!(
                version = applied_migration.version,
                "applied database migration is missing from embedded migrations"
            );
            return Err(MigrateError::VersionMissing(applied_migration.version));
        }
    }
    for migration in POSTGRES_MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
    {
        if let Some(applied_migration) = applied_migrations
            .iter()
            .find(|applied_migration| applied_migration.version == migration.version)
        {
            if migration.checksum != applied_migration.checksum {
                warn!(
                    version = migration.version,
                    description = %migration.description,
                    "database migration checksum mismatch (ignored: version-only validation)"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{all_up_migrations, pending_migrations_from_applied, POSTGRES_MIGRATOR};

    #[test]
    fn embeds_ordered_postgres_migration_sources() {
        let versions = POSTGRES_MIGRATOR
            .iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>();
        assert!(!versions.is_empty());
        assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(pending_migrations_from_applied(&[]), all_up_migrations());
    }

    #[test]
    fn concurrent_index_migrations_opt_out_of_transactions() {
        for version in [20260715000000, 20260715130000, 20260720000000] {
            let migration = POSTGRES_MIGRATOR
                .iter()
                .find(|migration| migration.version == version)
                .expect("concurrent index migration should be embedded");
            assert!(
                migration.no_tx,
                "migration {version} must run without a transaction"
            );
        }

        let analyze_tuning = POSTGRES_MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260715130100)
            .expect("analyze tuning migration should be embedded");
        assert!(!analyze_tuning.no_tx);
    }
}
