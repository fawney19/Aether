use crate::backend::MysqlBackend;
use crate::error::SqlResultExt;
use crate::{DataLayerError, DatabaseMaintenanceSummary};

use super::maintenance_identifier;

impl MysqlBackend {
    pub async fn run_table_maintenance(
        &self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        let mut summary = DatabaseMaintenanceSummary::default();
        for table_name in table_names {
            let table_name = maintenance_identifier(table_name)?;
            summary.attempted += 1;
            let statement = format!("ANALYZE TABLE `{table_name}`");
            if sqlx::raw_sql(&statement)
                .execute(self.pool())
                .await
                .map_sql_err()
                .is_ok()
            {
                summary.succeeded += 1;
            }
        }
        Ok(summary)
    }
}
