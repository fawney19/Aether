use super::{
    GatewayDataState, InsertRiskControlLogRecord, InsertRiskControlNotificationOutboxRecord,
    RiskControlHashListQuery, RiskControlLogListQuery, RiskControlNotificationOutboxSummary,
    StoredRiskControlFlaggedHash, StoredRiskControlHashPage, StoredRiskControlLog,
    StoredRiskControlLogPage, StoredRiskControlNotificationOutbox,
    UpsertRiskControlFlaggedHashRecord,
};
use aether_data::repository::risk_control::RiskControlLogRetentionSummary;
use aether_data::DataLayerError;

impl GatewayDataState {
    pub(crate) async fn list_risk_control_logs(
        &self,
        query: &RiskControlLogListQuery,
    ) -> Result<StoredRiskControlLogPage, DataLayerError> {
        match &self.risk_control_reader {
            Some(repository) => repository.list_logs(query).await,
            None => Ok(StoredRiskControlLogPage::default()),
        }
    }

    pub(crate) async fn insert_risk_control_log(
        &self,
        record: InsertRiskControlLogRecord,
    ) -> Result<Option<StoredRiskControlLog>, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => repository.insert_log(record).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn mark_risk_control_log_notification_sent(
        &self,
        id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => repository.mark_log_notification_sent(id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn record_risk_control_log_notification_result(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => {
                repository
                    .record_log_notification_result(id, sent, error_message, attempted_at_unix_secs)
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn count_risk_control_flagged_logs_since(
        &self,
        user_id: &str,
        since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        match &self.risk_control_reader {
            Some(repository) => {
                repository
                    .count_flagged_logs_since(user_id, since_unix_secs)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn get_risk_control_flagged_hash(
        &self,
        input_hash: &str,
    ) -> Result<Option<StoredRiskControlFlaggedHash>, DataLayerError> {
        match &self.risk_control_reader {
            Some(repository) => repository.get_flagged_hash(input_hash).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_risk_control_flagged_hashes(
        &self,
        query: &RiskControlHashListQuery,
    ) -> Result<StoredRiskControlHashPage, DataLayerError> {
        match &self.risk_control_reader {
            Some(repository) => repository.list_flagged_hashes(query).await,
            None => Ok(StoredRiskControlHashPage::default()),
        }
    }

    pub(crate) async fn count_risk_control_flagged_hashes(&self) -> Result<u64, DataLayerError> {
        match &self.risk_control_reader {
            Some(repository) => repository.count_flagged_hashes().await,
            None => Ok(0),
        }
    }

    pub(crate) async fn upsert_risk_control_flagged_hash(
        &self,
        record: UpsertRiskControlFlaggedHashRecord,
    ) -> Result<Option<StoredRiskControlFlaggedHash>, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => repository.upsert_flagged_hash(record).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_risk_control_flagged_hash(
        &self,
        input_hash: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => repository.delete_flagged_hash(input_hash).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn clear_risk_control_flagged_hashes(&self) -> Result<u64, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => repository.clear_flagged_hashes().await,
            None => Ok(0),
        }
    }

    pub(crate) async fn delete_expired_risk_control_logs(
        &self,
        hit_before_unix_secs: Option<u64>,
        non_hit_before_unix_secs: Option<u64>,
    ) -> Result<RiskControlLogRetentionSummary, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => {
                repository
                    .delete_expired_logs(hit_before_unix_secs, non_hit_before_unix_secs)
                    .await
            }
            None => Ok(RiskControlLogRetentionSummary::default()),
        }
    }

    pub(crate) async fn list_risk_control_notification_outbox_by_log_ids(
        &self,
        log_ids: &[String],
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        match &self.risk_control_reader {
            Some(repository) => {
                repository
                    .list_notification_outbox_by_log_ids(log_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn risk_control_notification_outbox_summary(
        &self,
    ) -> Result<RiskControlNotificationOutboxSummary, DataLayerError> {
        match &self.risk_control_reader {
            Some(repository) => repository.notification_outbox_summary().await,
            None => Ok(RiskControlNotificationOutboxSummary::default()),
        }
    }

    pub(crate) async fn insert_risk_control_notification_outbox(
        &self,
        record: InsertRiskControlNotificationOutboxRecord,
    ) -> Result<Option<StoredRiskControlNotificationOutbox>, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => repository
                .insert_notification_outbox(record)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn claim_due_risk_control_notification_outbox(
        &self,
        now_unix_secs: u64,
        lease_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => {
                repository
                    .claim_due_notification_outbox(now_unix_secs, lease_until_unix_secs, limit)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn record_risk_control_notification_outbox_attempt(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
        next_attempt_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => {
                repository
                    .record_notification_outbox_attempt(
                        id,
                        sent,
                        error_message,
                        attempted_at_unix_secs,
                        next_attempt_at_unix_secs,
                    )
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn defer_risk_control_notification_outbox_attempt(
        &self,
        id: &str,
        error_message: Option<String>,
        deferred_at_unix_secs: u64,
        next_attempt_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => {
                repository
                    .defer_notification_outbox_attempt(
                        id,
                        error_message,
                        deferred_at_unix_secs,
                        next_attempt_at_unix_secs,
                    )
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn reset_risk_control_notification_outbox_for_retry(
        &self,
        log_id: &str,
        next_attempt_at_unix_secs: u64,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        match &self.risk_control_writer {
            Some(repository) => {
                repository
                    .reset_notification_outbox_for_retry(log_id, next_attempt_at_unix_secs)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }
}
