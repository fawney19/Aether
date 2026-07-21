#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredSystemConfigEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub description: Option<String>,
    pub updated_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminSecurityBlacklistEntry {
    pub ip_address: String,
    pub reason: String,
    pub ttl_seconds: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct AdminSystemStats {
    pub total_users: u64,
    pub active_users: u64,
    pub total_api_keys: u64,
    pub total_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminSystemUsageAggregateImportMode {
    Skip,
    Overwrite,
    Error,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminSystemStatsDailyAggregate {
    pub date_unix_secs: u64,
    pub total_requests: u64,
    pub success_requests: u64,
    pub error_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost: f64,
    pub actual_total_cost: f64,
    pub is_complete: bool,
    pub aggregated_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminSystemStatsUserDailyAggregate {
    pub user_id: String,
    pub username: Option<String>,
    pub date_unix_secs: u64,
    pub total_requests: u64,
    pub success_requests: u64,
    pub error_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminSystemStatsDailyApiKeyAggregate {
    pub api_key_id: String,
    pub api_key_name: Option<String>,
    pub date_unix_secs: u64,
    pub total_requests: u64,
    pub success_requests: u64,
    pub error_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_cost: f64,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminSystemUsageAggregateSnapshot {
    #[serde(default)]
    pub stats_daily: Vec<AdminSystemStatsDailyAggregate>,
    #[serde(default)]
    pub stats_user_daily: Vec<AdminSystemStatsUserDailyAggregate>,
    #[serde(default)]
    pub stats_daily_api_key: Vec<AdminSystemStatsDailyApiKeyAggregate>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdminSystemUsageAggregateImportCounter {
    pub created: u64,
    pub updated: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdminSystemUsageAggregateImportSummary {
    pub stats_daily: AdminSystemUsageAggregateImportCounter,
    pub stats_user_daily: AdminSystemUsageAggregateImportCounter,
    pub stats_daily_api_key: AdminSystemUsageAggregateImportCounter,
    pub skipped_unmapped_user_daily: u64,
    pub skipped_unmapped_api_key_daily: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminSystemPurgeTarget {
    Config,
    Users,
    Usage,
    AuditLogs,
    RequestBodies,
    Stats,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminSystemPurgeSummary {
    pub affected: std::collections::BTreeMap<String, u64>,
}

impl AdminSystemPurgeSummary {
    pub fn add(&mut self, key: impl Into<String>, count: u64) {
        *self.affected.entry(key.into()).or_insert(0) += count;
    }

    pub fn merge(&mut self, other: &Self) {
        for (key, count) in &other.affected {
            self.add(key.clone(), *count);
        }
    }

    pub fn total(&self) -> u64 {
        self.affected.values().copied().sum()
    }
}
