#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingBackfillInfo {
    pub version: i64,
    pub description: String,
}
