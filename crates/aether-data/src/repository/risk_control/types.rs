use async_trait::async_trait;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RiskControlLogListQuery {
    pub offset: usize,
    pub limit: usize,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub flagged: Option<bool>,
    pub action: Option<String>,
    pub decision_source: Option<String>,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub q: Option<String>,
    pub from_unix_secs: Option<u64>,
    pub to_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredRiskControlLog {
    pub id: String,
    pub trace_id: String,
    pub request_id: Option<String>,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub user_email: Option<String>,
    pub api_key_id: Option<String>,
    pub api_key_name: Option<String>,
    pub route_family: Option<String>,
    pub route_kind: Option<String>,
    pub api_format: Option<String>,
    pub endpoint: String,
    pub model: Option<String>,
    pub mode: String,
    pub action: String,
    pub decision_source: String,
    pub flagged: bool,
    pub highest_category: Option<String>,
    pub highest_score: f64,
    pub category_scores: Option<serde_json::Value>,
    pub thresholds: Option<serde_json::Value>,
    pub matched_keywords: Option<serde_json::Value>,
    pub input_hash: Option<String>,
    pub excerpt: Option<String>,
    pub latency_ms: Option<u64>,
    pub queue_delay_ms: Option<u64>,
    pub violation_count: u64,
    pub auto_action: Option<String>,
    pub notification_sent: bool,
    pub notification_attempts: u64,
    pub notification_last_error: Option<String>,
    pub notification_last_attempt_at_unix_secs: Option<u64>,
    pub error_message: Option<String>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredRiskControlLogPage {
    pub items: Vec<StoredRiskControlLog>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InsertRiskControlLogRecord {
    pub id: Option<String>,
    pub trace_id: String,
    pub request_id: Option<String>,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub user_email: Option<String>,
    pub api_key_id: Option<String>,
    pub api_key_name: Option<String>,
    pub route_family: Option<String>,
    pub route_kind: Option<String>,
    pub api_format: Option<String>,
    pub endpoint: String,
    pub model: Option<String>,
    pub mode: String,
    pub action: String,
    pub decision_source: String,
    pub flagged: bool,
    pub highest_category: Option<String>,
    pub highest_score: f64,
    pub category_scores: Option<serde_json::Value>,
    pub thresholds: Option<serde_json::Value>,
    pub matched_keywords: Option<serde_json::Value>,
    pub input_hash: Option<String>,
    pub excerpt: Option<String>,
    pub latency_ms: Option<u64>,
    pub queue_delay_ms: Option<u64>,
    pub violation_count: u64,
    pub auto_action: Option<String>,
    pub notification_sent: bool,
    pub notification_attempts: u64,
    pub notification_last_error: Option<String>,
    pub notification_last_attempt_at_unix_secs: Option<u64>,
    pub error_message: Option<String>,
    pub created_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RiskControlHashListQuery {
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredRiskControlFlaggedHash {
    pub input_hash: String,
    pub source_log_id: Option<String>,
    pub reason: Option<String>,
    pub highest_category: Option<String>,
    pub highest_score: f64,
    pub excerpt: Option<String>,
    pub first_seen_at_unix_secs: u64,
    pub last_seen_at_unix_secs: u64,
    pub hit_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredRiskControlHashPage {
    pub items: Vec<StoredRiskControlFlaggedHash>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertRiskControlFlaggedHashRecord {
    pub input_hash: String,
    pub source_log_id: Option<String>,
    pub reason: Option<String>,
    pub highest_category: Option<String>,
    pub highest_score: f64,
    pub excerpt: Option<String>,
    pub now_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RiskControlLogRetentionSummary {
    pub hit_deleted: u64,
    pub non_hit_deleted: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredRiskControlNotificationOutbox {
    pub id: String,
    pub log_id: String,
    pub item_key: String,
    pub title: String,
    pub markdown_body: String,
    pub text_body: String,
    pub variables_json: serde_json::Value,
    pub status: String,
    pub attempt_count: u64,
    pub max_attempts: u64,
    pub next_attempt_at_unix_secs: Option<u64>,
    pub lease_until_unix_secs: Option<u64>,
    pub last_error: Option<String>,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
    pub sent_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InsertRiskControlNotificationOutboxRecord {
    pub id: Option<String>,
    pub log_id: String,
    pub item_key: String,
    pub title: String,
    pub markdown_body: String,
    pub text_body: String,
    pub variables_json: serde_json::Value,
    pub max_attempts: u64,
    pub next_attempt_at_unix_secs: Option<u64>,
    pub created_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RiskControlNotificationOutboxSummary {
    pub pending: u64,
    pub processing: u64,
    pub sent: u64,
    pub dead: u64,
    pub oldest_pending_at_unix_secs: Option<u64>,
    pub next_attempt_at_unix_secs: Option<u64>,
    pub last_error: Option<String>,
}

#[async_trait]
pub trait RiskControlReadRepository: Send + Sync {
    async fn list_logs(
        &self,
        query: &RiskControlLogListQuery,
    ) -> Result<StoredRiskControlLogPage, crate::DataLayerError>;

    async fn count_flagged_logs_since(
        &self,
        user_id: &str,
        since_unix_secs: u64,
    ) -> Result<u64, crate::DataLayerError>;

    async fn get_flagged_hash(
        &self,
        input_hash: &str,
    ) -> Result<Option<StoredRiskControlFlaggedHash>, crate::DataLayerError>;

    async fn list_flagged_hashes(
        &self,
        query: &RiskControlHashListQuery,
    ) -> Result<StoredRiskControlHashPage, crate::DataLayerError>;

    async fn count_flagged_hashes(&self) -> Result<u64, crate::DataLayerError>;

    async fn list_notification_outbox_by_log_ids(
        &self,
        log_ids: &[String],
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, crate::DataLayerError>;

    async fn notification_outbox_summary(
        &self,
    ) -> Result<RiskControlNotificationOutboxSummary, crate::DataLayerError>;
}

#[async_trait]
pub trait RiskControlWriteRepository: Send + Sync {
    async fn insert_log(
        &self,
        record: InsertRiskControlLogRecord,
    ) -> Result<StoredRiskControlLog, crate::DataLayerError>;

    async fn mark_log_notification_sent(&self, id: &str) -> Result<bool, crate::DataLayerError>;

    async fn record_log_notification_result(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
    ) -> Result<bool, crate::DataLayerError>;

    async fn upsert_flagged_hash(
        &self,
        record: UpsertRiskControlFlaggedHashRecord,
    ) -> Result<StoredRiskControlFlaggedHash, crate::DataLayerError>;

    async fn delete_flagged_hash(&self, input_hash: &str) -> Result<bool, crate::DataLayerError>;

    async fn clear_flagged_hashes(&self) -> Result<u64, crate::DataLayerError>;

    async fn delete_expired_logs(
        &self,
        hit_before_unix_secs: Option<u64>,
        non_hit_before_unix_secs: Option<u64>,
    ) -> Result<RiskControlLogRetentionSummary, crate::DataLayerError>;

    async fn insert_notification_outbox(
        &self,
        record: InsertRiskControlNotificationOutboxRecord,
    ) -> Result<StoredRiskControlNotificationOutbox, crate::DataLayerError>;

    async fn claim_due_notification_outbox(
        &self,
        now_unix_secs: u64,
        lease_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, crate::DataLayerError>;

    async fn record_notification_outbox_attempt(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
        next_attempt_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn defer_notification_outbox_attempt(
        &self,
        id: &str,
        error_message: Option<String>,
        deferred_at_unix_secs: u64,
        next_attempt_at_unix_secs: u64,
    ) -> Result<bool, crate::DataLayerError>;

    async fn reset_notification_outbox_for_retry(
        &self,
        log_id: &str,
        next_attempt_at_unix_secs: u64,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, crate::DataLayerError>;
}

impl InsertRiskControlLogRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.trace_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_logs.trace_id is empty".to_string(),
            ));
        }
        if self.endpoint.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_logs.endpoint is empty".to_string(),
            ));
        }
        if self.mode.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_logs.mode is empty".to_string(),
            ));
        }
        if self.action.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_logs.action is empty".to_string(),
            ));
        }
        if self.decision_source.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_logs.decision_source is empty".to_string(),
            ));
        }
        Ok(())
    }
}

impl UpsertRiskControlFlaggedHashRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.input_hash.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_flagged_hashes.input_hash is empty".to_string(),
            ));
        }
        Ok(())
    }
}

impl InsertRiskControlNotificationOutboxRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.log_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_notification_outbox.log_id is empty".to_string(),
            ));
        }
        if self.item_key.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_notification_outbox.item_key is empty".to_string(),
            ));
        }
        if self.title.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_notification_outbox.title is empty".to_string(),
            ));
        }
        if self.max_attempts == 0 {
            return Err(crate::DataLayerError::UnexpectedValue(
                "risk_control_notification_outbox.max_attempts must be positive".to_string(),
            ));
        }
        Ok(())
    }
}
