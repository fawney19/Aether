//! Database lifecycle contracts shared by driver adapters and the data facade.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMigrationInfo {
    pub version: i64,
    pub description: String,
}
