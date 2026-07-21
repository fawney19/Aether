//! Runtime database migration entry points.
//!
//! Each driver owns its migrator and startup preparation. The facade keeps
//! the established public entry points used by gateway bootstrap code.

#[cfg(feature = "mysql")]
mod mysql;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;
mod types;

#[cfg(all(test, feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod tests;

#[cfg(feature = "postgres")]
pub use postgres::{pending_migrations, prepare_database_for_startup, run_migrations};
pub use types::PendingMigrationInfo;

#[cfg(any(feature = "mysql", feature = "sqlite"))]
use sqlx::migrate::MigrateError;

#[cfg(feature = "mysql")]
pub async fn run_mysql_migrations(pool: &sqlx::MySqlPool) -> Result<(), MigrateError> {
    mysql::run_migrations(pool).await
}

#[cfg(feature = "mysql")]
pub async fn pending_mysql_migrations(
    pool: &sqlx::MySqlPool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    mysql::pending_migrations(pool).await
}

#[cfg(feature = "mysql")]
pub async fn prepare_mysql_database_for_startup(
    pool: &sqlx::MySqlPool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    mysql::prepare_database_for_startup(pool).await
}

#[cfg(feature = "sqlite")]
pub async fn run_sqlite_migrations(pool: &sqlx::SqlitePool) -> Result<(), MigrateError> {
    sqlite::run_migrations(pool).await
}

#[cfg(feature = "sqlite")]
pub async fn pending_sqlite_migrations(
    pool: &sqlx::SqlitePool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    sqlite::pending_migrations(pool).await
}

#[cfg(feature = "sqlite")]
pub async fn prepare_sqlite_database_for_startup(
    pool: &sqlx::SqlitePool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    sqlite::prepare_database_for_startup(pool).await
}
