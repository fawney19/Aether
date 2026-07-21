use sqlx::migrate::MigrateError;
use tracing::info;

use super::types::PendingBackfillInfo;
use crate::driver::mysql::MysqlPool;

pub async fn run_backfills(_pool: &MysqlPool) -> Result<(), MigrateError> {
    info!("mysql database backfills are up to date");
    Ok(())
}

pub async fn pending_backfills(
    _pool: &MysqlPool,
) -> Result<Vec<PendingBackfillInfo>, MigrateError> {
    Ok(Vec::new())
}
