use sqlx::{
    migrate::{MigrateError, Migrator},
    PgConnection, PgPool,
};

use super::types::PendingMigrationInfo;

pub use aether_data_postgres::pending_migrations;
#[cfg(all(test, feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub(super) use aether_data_postgres::{
    all_up_migrations, pending_migrations_from_applied, POSTGRES_MIGRATOR,
};

#[derive(Debug, Clone, Copy)]
struct SnapshotBootstrap;

impl aether_data_postgres::PostgresMigrationBootstrap for SnapshotBootstrap {
    fn apply_snapshot<'a>(
        &self,
        conn: &'a mut PgConnection,
        migrator: &'static Migrator,
    ) -> aether_data_postgres::BootstrapFuture<'a> {
        Box::pin(crate::lifecycle::bootstrap::postgres::apply_snapshot_if_empty(conn, migrator))
    }
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    aether_data_postgres::run_migrations_with_bootstrap(pool, &SnapshotBootstrap).await
}

pub async fn prepare_database_for_startup(
    pool: &PgPool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    aether_data_postgres::prepare_database_for_startup_with_bootstrap(pool, &SnapshotBootstrap)
        .await
}
