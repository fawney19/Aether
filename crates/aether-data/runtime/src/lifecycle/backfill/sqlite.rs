use sqlx::migrate::MigrateError;
use tracing::info;

use super::types::PendingBackfillInfo;
use crate::driver::sqlite::SqlitePool;

pub async fn run_backfills(_pool: &SqlitePool) -> Result<(), MigrateError> {
    info!("sqlite database backfills are up to date");
    Ok(())
}

pub async fn pending_backfills(
    _pool: &SqlitePool,
) -> Result<Vec<PendingBackfillInfo>, MigrateError> {
    Ok(Vec::new())
}
