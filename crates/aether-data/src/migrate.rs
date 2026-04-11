use std::collections::{HashMap, HashSet};

use sqlx::{
    migrate::{Migrate, MigrateError, Migrator},
    PgPool,
};
use tracing::{error, info, warn};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Run all pending migrations embedded at compile time from `migrations/`.
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    let mut conn = pool.acquire().await?;

    if MIGRATOR.locking {
        conn.lock().await?;
    }

    let result = run_migrations_locked(&mut *conn).await;

    if MIGRATOR.locking {
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

async fn run_migrations_locked<C>(conn: &mut C) -> Result<(), MigrateError>
where
    C: Migrate,
{
    conn.ensure_migrations_table().await?;

    if let Some(version) = conn.dirty_version().await? {
        error!(version, "database migration state is dirty");
        return Err(MigrateError::Dirty(version));
    }

    let applied_migrations = conn.list_applied_migrations().await?;
    validate_applied_migrations(&applied_migrations)?;

    let known_versions: HashSet<_> = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .collect();
    let applied_migrations_by_version: HashMap<_, _> = applied_migrations
        .into_iter()
        .map(|migration| (migration.version, migration))
        .collect();

    let pending_migrations: Vec<_> = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| !applied_migrations_by_version.contains_key(&migration.version))
        .collect();

    let total_migrations = known_versions.len();
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
        let total = pending_migrations.len();

        info!(
            current,
            total,
            version = migration.version,
            description = %migration.description,
            "applying database migration"
        );

        let elapsed = conn.apply(migration).await?;

        info!(
            current,
            total,
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

fn validate_applied_migrations(
    applied_migrations: &[sqlx::migrate::AppliedMigration],
) -> Result<(), MigrateError> {
    if MIGRATOR.ignore_missing {
        return Ok(());
    }

    let known_versions: HashSet<_> = MIGRATOR.iter().map(|migration| migration.version).collect();

    for applied_migration in applied_migrations {
        if !known_versions.contains(&applied_migration.version) {
            error!(
                version = applied_migration.version,
                "applied database migration is missing from embedded migrations"
            );
            return Err(MigrateError::VersionMissing(applied_migration.version));
        }
    }

    // Checksum drift is reported as a warning only. Strict enforcement makes
    // harmless edits (comment tweaks, whitespace, metadata fixes) impossible
    // without also touching every environment's migration history table. We
    // match by version alone — sqlx still skips already-applied migrations so
    // edited files will not re-run, they are merely allowed to exist.
    for migration in MIGRATOR
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
    use super::MIGRATOR;

    #[test]
    fn baseline_migration_keeps_empty_search_path_transaction_local() {
        let baseline = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260403000000)
            .expect("baseline migration should be embedded");

        assert!(
            baseline
                .sql
                .contains("SELECT pg_catalog.set_config('search_path', '', true);"),
            "baseline migration must keep empty search_path transaction-local so sqlx bookkeeping can still access _sqlx_migrations",
        );
        assert!(
            !baseline
                .sql
                .contains("SELECT pg_catalog.set_config('search_path', '', false);"),
            "baseline migration must not persist an empty search_path at session scope",
        );
    }
}
