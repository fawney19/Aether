use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use super::types::{
    InsertRiskControlLogRecord, InsertRiskControlNotificationOutboxRecord,
    RiskControlHashListQuery, RiskControlLogListQuery, RiskControlLogRetentionSummary,
    RiskControlNotificationOutboxSummary, RiskControlReadRepository, RiskControlWriteRepository,
    StoredRiskControlFlaggedHash, StoredRiskControlHashPage, StoredRiskControlLog,
    StoredRiskControlLogPage, StoredRiskControlNotificationOutbox,
    UpsertRiskControlFlaggedHashRecord,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryRiskControlRepository {
    logs: RwLock<Vec<StoredRiskControlLog>>,
    hashes: RwLock<Vec<StoredRiskControlFlaggedHash>>,
    notification_outbox: RwLock<Vec<StoredRiskControlNotificationOutbox>>,
}

impl InMemoryRiskControlRepository {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_logs<I>(logs: I) -> Self
    where
        I: IntoIterator<Item = StoredRiskControlLog>,
    {
        Self {
            logs: RwLock::new(logs.into_iter().collect()),
            hashes: RwLock::new(Vec::new()),
            notification_outbox: RwLock::new(Vec::new()),
        }
    }

    fn now_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

#[async_trait]
impl RiskControlReadRepository for InMemoryRiskControlRepository {
    async fn list_logs(
        &self,
        query: &RiskControlLogListQuery,
    ) -> Result<StoredRiskControlLogPage, DataLayerError> {
        let mut items = self
            .logs
            .read()
            .expect("risk control logs lock")
            .iter()
            .filter(|item| log_matches_query(item, query))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .created_at_unix_secs
                .cmp(&left.created_at_unix_secs)
                .then_with(|| right.id.cmp(&left.id))
        });
        let total = items.len() as u64;
        let limit = query.limit.clamp(1, 500);
        let items = items.into_iter().skip(query.offset).take(limit).collect();
        Ok(StoredRiskControlLogPage { items, total })
    }

    async fn count_flagged_logs_since(
        &self,
        user_id: &str,
        since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        Ok(self
            .logs
            .read()
            .expect("risk control logs lock")
            .iter()
            .filter(|item| {
                item.flagged
                    && item.user_id.as_deref() == Some(user_id)
                    && item.created_at_unix_secs >= since_unix_secs
            })
            .count() as u64)
    }

    async fn get_flagged_hash(
        &self,
        input_hash: &str,
    ) -> Result<Option<StoredRiskControlFlaggedHash>, DataLayerError> {
        Ok(self
            .hashes
            .read()
            .expect("risk control hashes lock")
            .iter()
            .find(|item| item.input_hash == input_hash)
            .cloned())
    }

    async fn list_flagged_hashes(
        &self,
        query: &RiskControlHashListQuery,
    ) -> Result<StoredRiskControlHashPage, DataLayerError> {
        let mut items = self
            .hashes
            .read()
            .expect("risk control hashes lock")
            .clone();
        items.sort_by(|left, right| {
            right
                .last_seen_at_unix_secs
                .cmp(&left.last_seen_at_unix_secs)
                .then_with(|| right.hit_count.cmp(&left.hit_count))
                .then_with(|| left.input_hash.cmp(&right.input_hash))
        });
        let total = items.len() as u64;
        let limit = query.limit.clamp(1, 500);
        let items = items.into_iter().skip(query.offset).take(limit).collect();
        Ok(StoredRiskControlHashPage { items, total })
    }

    async fn count_flagged_hashes(&self) -> Result<u64, DataLayerError> {
        Ok(self.hashes.read().expect("risk control hashes lock").len() as u64)
    }

    async fn list_notification_outbox_by_log_ids(
        &self,
        log_ids: &[String],
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        if log_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = log_ids.iter().collect::<std::collections::HashSet<_>>();
        Ok(self
            .notification_outbox
            .read()
            .expect("risk control notification outbox lock")
            .iter()
            .filter(|item| ids.contains(&item.log_id))
            .cloned()
            .collect())
    }

    async fn notification_outbox_summary(
        &self,
    ) -> Result<RiskControlNotificationOutboxSummary, DataLayerError> {
        let outbox = self
            .notification_outbox
            .read()
            .expect("risk control notification outbox lock");
        let mut summary = RiskControlNotificationOutboxSummary::default();
        let mut newest_error_updated_at = 0;
        for item in outbox.iter() {
            match item.status.as_str() {
                "pending" => {
                    summary.pending = summary.pending.saturating_add(1);
                    summary.oldest_pending_at_unix_secs = Some(
                        summary
                            .oldest_pending_at_unix_secs
                            .map_or(item.created_at_unix_secs, |value| {
                                value.min(item.created_at_unix_secs)
                            }),
                    );
                    if let Some(next) = item.next_attempt_at_unix_secs {
                        summary.next_attempt_at_unix_secs = Some(
                            summary
                                .next_attempt_at_unix_secs
                                .map_or(next, |value| value.min(next)),
                        );
                    }
                }
                "processing" => summary.processing = summary.processing.saturating_add(1),
                "sent" => summary.sent = summary.sent.saturating_add(1),
                "dead" => {
                    summary.dead = summary.dead.saturating_add(1);
                }
                _ => {}
            }
            if item.status != "sent"
                && item.last_error.is_some()
                && item.updated_at_unix_secs >= newest_error_updated_at
            {
                newest_error_updated_at = item.updated_at_unix_secs;
                summary.last_error = item.last_error.clone();
            }
        }
        Ok(summary)
    }
}

#[async_trait]
impl RiskControlWriteRepository for InMemoryRiskControlRepository {
    async fn insert_log(
        &self,
        record: InsertRiskControlLogRecord,
    ) -> Result<StoredRiskControlLog, DataLayerError> {
        record.validate()?;
        let log = StoredRiskControlLog {
            id: record
                .id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            trace_id: record.trace_id,
            request_id: record.request_id,
            user_id: record.user_id,
            username: record.username,
            user_email: record.user_email,
            api_key_id: record.api_key_id,
            api_key_name: record.api_key_name,
            route_family: record.route_family,
            route_kind: record.route_kind,
            api_format: record.api_format,
            endpoint: record.endpoint,
            model: record.model,
            mode: record.mode,
            action: record.action,
            decision_source: record.decision_source,
            flagged: record.flagged,
            highest_category: record.highest_category,
            highest_score: record.highest_score,
            category_scores: record.category_scores,
            thresholds: record.thresholds,
            matched_keywords: record.matched_keywords,
            input_hash: record.input_hash,
            excerpt: record.excerpt,
            latency_ms: record.latency_ms,
            queue_delay_ms: record.queue_delay_ms,
            violation_count: record.violation_count,
            auto_action: record.auto_action,
            notification_sent: record.notification_sent,
            notification_attempts: record.notification_attempts,
            notification_last_error: record.notification_last_error,
            notification_last_attempt_at_unix_secs: record.notification_last_attempt_at_unix_secs,
            error_message: record.error_message,
            created_at_unix_secs: record
                .created_at_unix_secs
                .unwrap_or_else(Self::now_unix_secs),
        };
        self.logs
            .write()
            .expect("risk control logs lock")
            .push(log.clone());
        Ok(log)
    }

    async fn mark_log_notification_sent(&self, id: &str) -> Result<bool, DataLayerError> {
        let mut logs = self.logs.write().expect("risk control logs lock");
        let Some(log) = logs.iter_mut().find(|item| item.id == id) else {
            return Ok(false);
        };
        log.notification_sent = true;
        log.notification_last_error = None;
        Ok(true)
    }

    async fn record_log_notification_result(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let mut logs = self.logs.write().expect("risk control logs lock");
        let Some(log) = logs.iter_mut().find(|item| item.id == id) else {
            return Ok(false);
        };
        if sent {
            log.notification_sent = true;
            log.notification_last_error = None;
        } else {
            log.notification_last_error = error_message;
        }
        log.notification_attempts = log.notification_attempts.saturating_add(1);
        log.notification_last_attempt_at_unix_secs = Some(attempted_at_unix_secs);
        Ok(true)
    }

    async fn upsert_flagged_hash(
        &self,
        record: UpsertRiskControlFlaggedHashRecord,
    ) -> Result<StoredRiskControlFlaggedHash, DataLayerError> {
        record.validate()?;
        let now = record.now_unix_secs.unwrap_or_else(Self::now_unix_secs);
        let mut hashes = self.hashes.write().expect("risk control hashes lock");
        if let Some(existing) = hashes
            .iter_mut()
            .find(|item| item.input_hash == record.input_hash)
        {
            existing.source_log_id = record
                .source_log_id
                .or_else(|| existing.source_log_id.clone());
            existing.reason = record.reason.or_else(|| existing.reason.clone());
            existing.highest_category = record
                .highest_category
                .or_else(|| existing.highest_category.clone());
            existing.highest_score = record.highest_score.max(existing.highest_score);
            existing.excerpt = record.excerpt.or_else(|| existing.excerpt.clone());
            existing.last_seen_at_unix_secs = now;
            existing.hit_count = existing.hit_count.saturating_add(1);
            return Ok(existing.clone());
        }
        let hash = StoredRiskControlFlaggedHash {
            input_hash: record.input_hash,
            source_log_id: record.source_log_id,
            reason: record.reason,
            highest_category: record.highest_category,
            highest_score: record.highest_score,
            excerpt: record.excerpt,
            first_seen_at_unix_secs: now,
            last_seen_at_unix_secs: now,
            hit_count: 1,
        };
        hashes.push(hash.clone());
        Ok(hash)
    }

    async fn delete_flagged_hash(&self, input_hash: &str) -> Result<bool, DataLayerError> {
        let mut hashes = self.hashes.write().expect("risk control hashes lock");
        let before = hashes.len();
        hashes.retain(|item| item.input_hash != input_hash);
        Ok(hashes.len() != before)
    }

    async fn clear_flagged_hashes(&self) -> Result<u64, DataLayerError> {
        let mut hashes = self.hashes.write().expect("risk control hashes lock");
        let deleted = hashes.len() as u64;
        hashes.clear();
        Ok(deleted)
    }

    async fn delete_expired_logs(
        &self,
        hit_before_unix_secs: Option<u64>,
        non_hit_before_unix_secs: Option<u64>,
    ) -> Result<RiskControlLogRetentionSummary, DataLayerError> {
        let mut logs = self.logs.write().expect("risk control logs lock");
        let mut hit_deleted = 0;
        let mut non_hit_deleted = 0;
        logs.retain(|item| {
            if item.flagged
                && hit_before_unix_secs.is_some_and(|cutoff| item.created_at_unix_secs < cutoff)
            {
                hit_deleted += 1;
                return false;
            }
            if !item.flagged
                && non_hit_before_unix_secs.is_some_and(|cutoff| item.created_at_unix_secs < cutoff)
            {
                non_hit_deleted += 1;
                return false;
            }
            true
        });
        Ok(RiskControlLogRetentionSummary {
            hit_deleted,
            non_hit_deleted,
        })
    }

    async fn insert_notification_outbox(
        &self,
        record: InsertRiskControlNotificationOutboxRecord,
    ) -> Result<StoredRiskControlNotificationOutbox, DataLayerError> {
        record.validate()?;
        let now = record
            .created_at_unix_secs
            .unwrap_or_else(Self::now_unix_secs);
        let item = StoredRiskControlNotificationOutbox {
            id: record
                .id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            log_id: record.log_id,
            item_key: record.item_key,
            title: record.title,
            markdown_body: record.markdown_body,
            text_body: record.text_body,
            variables_json: record.variables_json,
            status: "pending".to_string(),
            attempt_count: 0,
            max_attempts: record.max_attempts,
            next_attempt_at_unix_secs: record.next_attempt_at_unix_secs.or(Some(now)),
            lease_until_unix_secs: None,
            last_error: None,
            created_at_unix_secs: now,
            updated_at_unix_secs: now,
            sent_at_unix_secs: None,
        };
        let mut outbox = self
            .notification_outbox
            .write()
            .expect("risk control notification outbox lock");
        if let Some(existing) = outbox
            .iter_mut()
            .find(|row| row.log_id == item.log_id && row.item_key == item.item_key)
        {
            *existing = item.clone();
            return Ok(item);
        }
        outbox.push(item.clone());
        Ok(item)
    }

    async fn claim_due_notification_outbox(
        &self,
        now_unix_secs: u64,
        lease_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        let mut outbox = self
            .notification_outbox
            .write()
            .expect("risk control notification outbox lock");
        let mut due = outbox
            .iter_mut()
            .filter(|item| notification_outbox_due(item, now_unix_secs))
            .collect::<Vec<_>>();
        due.sort_by(|left, right| {
            left.next_attempt_at_unix_secs
                .unwrap_or(left.created_at_unix_secs)
                .cmp(
                    &right
                        .next_attempt_at_unix_secs
                        .unwrap_or(right.created_at_unix_secs),
                )
                .then_with(|| left.created_at_unix_secs.cmp(&right.created_at_unix_secs))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut claimed = Vec::new();
        for item in due.into_iter().take(limit.clamp(1, 500)) {
            item.status = "processing".to_string();
            item.lease_until_unix_secs = Some(lease_until_unix_secs);
            item.updated_at_unix_secs = now_unix_secs;
            claimed.push(item.clone());
        }
        Ok(claimed)
    }

    async fn record_notification_outbox_attempt(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
        next_attempt_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        let mut outbox = self
            .notification_outbox
            .write()
            .expect("risk control notification outbox lock");
        let Some(item) = outbox.iter_mut().find(|item| item.id == id) else {
            return Ok(false);
        };
        item.attempt_count = item.attempt_count.saturating_add(1);
        item.updated_at_unix_secs = attempted_at_unix_secs;
        item.lease_until_unix_secs = None;
        if sent {
            item.status = "sent".to_string();
            item.last_error = None;
            item.next_attempt_at_unix_secs = None;
            item.sent_at_unix_secs = Some(attempted_at_unix_secs);
        } else {
            item.last_error = error_message;
            item.next_attempt_at_unix_secs = next_attempt_at_unix_secs;
            item.status = if next_attempt_at_unix_secs.is_some() {
                "pending".to_string()
            } else {
                "dead".to_string()
            };
        }
        Ok(true)
    }

    async fn defer_notification_outbox_attempt(
        &self,
        id: &str,
        error_message: Option<String>,
        deferred_at_unix_secs: u64,
        next_attempt_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let mut outbox = self
            .notification_outbox
            .write()
            .expect("risk control notification outbox lock");
        let Some(item) = outbox.iter_mut().find(|item| item.id == id) else {
            return Ok(false);
        };
        item.status = "pending".to_string();
        item.next_attempt_at_unix_secs = Some(next_attempt_at_unix_secs);
        item.lease_until_unix_secs = None;
        item.last_error = error_message;
        item.updated_at_unix_secs = deferred_at_unix_secs;
        item.sent_at_unix_secs = None;
        Ok(true)
    }

    async fn reset_notification_outbox_for_retry(
        &self,
        log_id: &str,
        next_attempt_at_unix_secs: u64,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        let mut outbox = self
            .notification_outbox
            .write()
            .expect("risk control notification outbox lock");
        let mut reset_any = false;
        for item in outbox
            .iter_mut()
            .filter(|item| item.log_id == log_id && item.status != "sent")
        {
            item.status = "pending".to_string();
            item.attempt_count = 0;
            item.next_attempt_at_unix_secs = Some(next_attempt_at_unix_secs);
            item.lease_until_unix_secs = None;
            item.last_error = None;
            item.updated_at_unix_secs = next_attempt_at_unix_secs;
            item.sent_at_unix_secs = None;
            reset_any = true;
        }
        if !reset_any {
            return Ok(Vec::new());
        }
        Ok(outbox
            .iter()
            .filter(|item| item.log_id == log_id)
            .cloned()
            .collect())
    }
}

fn notification_outbox_due(item: &StoredRiskControlNotificationOutbox, now_unix_secs: u64) -> bool {
    match item.status.as_str() {
        "pending" => item
            .next_attempt_at_unix_secs
            .is_some_and(|next| next <= now_unix_secs),
        "processing" => item
            .lease_until_unix_secs
            .is_some_and(|lease_until| lease_until <= now_unix_secs),
        _ => false,
    }
}

fn log_matches_query(item: &StoredRiskControlLog, query: &RiskControlLogListQuery) -> bool {
    if query
        .user_id
        .as_deref()
        .is_some_and(|value| item.user_id.as_deref() != Some(value))
    {
        return false;
    }
    if query
        .api_key_id
        .as_deref()
        .is_some_and(|value| item.api_key_id.as_deref() != Some(value))
    {
        return false;
    }
    if query.flagged.is_some_and(|value| item.flagged != value) {
        return false;
    }
    if query
        .action
        .as_deref()
        .is_some_and(|value| item.action != value)
    {
        return false;
    }
    if query
        .decision_source
        .as_deref()
        .is_some_and(|value| item.decision_source != value)
    {
        return false;
    }
    if query
        .endpoint
        .as_deref()
        .is_some_and(|value| item.endpoint != value)
    {
        return false;
    }
    if query
        .model
        .as_deref()
        .is_some_and(|value| item.model.as_deref() != Some(value))
    {
        return false;
    }
    if query
        .from_unix_secs
        .is_some_and(|value| item.created_at_unix_secs < value)
    {
        return false;
    }
    if query
        .to_unix_secs
        .is_some_and(|value| item.created_at_unix_secs > value)
    {
        return false;
    }
    if query
        .q
        .as_deref()
        .is_some_and(|value| !log_matches_search(item, value))
    {
        return false;
    }
    true
}

fn log_matches_search(item: &StoredRiskControlLog, raw: &str) -> bool {
    let needle = raw.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }

    [
        item.username.as_deref(),
        item.user_email.as_deref(),
        item.user_id.as_deref(),
        item.api_key_name.as_deref(),
        item.api_key_id.as_deref(),
        item.route_family.as_deref(),
        item.route_kind.as_deref(),
        item.api_format.as_deref(),
        Some(item.endpoint.as_str()),
        item.model.as_deref(),
        Some(item.action.as_str()),
        Some(item.decision_source.as_str()),
        item.highest_category.as_deref(),
        item.input_hash.as_deref(),
        item.excerpt.as_deref(),
        item.error_message.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.to_lowercase().contains(&needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn outbox_record(log_id: &str) -> InsertRiskControlNotificationOutboxRecord {
        InsertRiskControlNotificationOutboxRecord {
            id: Some(format!("outbox-{log_id}")),
            log_id: log_id.to_string(),
            item_key: "risk_control.flagged".to_string(),
            title: "风险通知".to_string(),
            markdown_body: "body".to_string(),
            text_body: "body".to_string(),
            variables_json: json!({ "trace_id": "trace-1" }),
            max_attempts: 2,
            next_attempt_at_unix_secs: Some(100),
            created_at_unix_secs: Some(90),
        }
    }

    fn outbox_record_with_item(
        log_id: &str,
        item_key: &str,
    ) -> InsertRiskControlNotificationOutboxRecord {
        InsertRiskControlNotificationOutboxRecord {
            id: Some(format!("outbox-{log_id}-{item_key}")),
            item_key: item_key.to_string(),
            ..outbox_record(log_id)
        }
    }

    #[tokio::test]
    async fn notification_outbox_claims_dead_letters_and_resets() {
        let repository = InMemoryRiskControlRepository::new();
        repository
            .insert_notification_outbox(outbox_record("log-1"))
            .await
            .expect("outbox insert should succeed");

        let summary = repository
            .notification_outbox_summary()
            .await
            .expect("summary should load");
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.next_attempt_at_unix_secs, Some(100));
        assert!(repository
            .claim_due_notification_outbox(99, 200, 10)
            .await
            .expect("early claim should succeed")
            .is_empty());

        let claimed = repository
            .claim_due_notification_outbox(100, 200, 10)
            .await
            .expect("due claim should succeed");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].status, "processing");

        repository
            .defer_notification_outbox_attempt(
                &claimed[0].id,
                Some("not ready".to_string()),
                101,
                130,
            )
            .await
            .expect("deferred attempt should record");
        let deferred = repository
            .list_notification_outbox_by_log_ids(&["log-1".to_string()])
            .await
            .expect("deferred outbox should load");
        assert_eq!(deferred[0].status, "pending");
        assert_eq!(deferred[0].attempt_count, 0);
        assert_eq!(deferred[0].last_error.as_deref(), Some("not ready"));
        let summary = repository
            .notification_outbox_summary()
            .await
            .expect("summary should load");
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.last_error.as_deref(), Some("not ready"));

        let claimed = repository
            .claim_due_notification_outbox(130, 230, 10)
            .await
            .expect("second claim should succeed");
        repository
            .record_notification_outbox_attempt(
                &claimed[0].id,
                false,
                Some("send failed".to_string()),
                131,
                Some(140),
            )
            .await
            .expect("failed attempt should record");
        let claimed = repository
            .claim_due_notification_outbox(140, 240, 10)
            .await
            .expect("third claim should succeed");
        repository
            .record_notification_outbox_attempt(
                &claimed[0].id,
                false,
                Some("still down".to_string()),
                141,
                None,
            )
            .await
            .expect("dead letter should record");

        let summary = repository
            .notification_outbox_summary()
            .await
            .expect("summary should load");
        assert_eq!(summary.dead, 1);
        assert_eq!(summary.last_error.as_deref(), Some("still down"));

        let reset = repository
            .reset_notification_outbox_for_retry("log-1", 150)
            .await
            .expect("reset should succeed");
        assert_eq!(reset.len(), 1);
        assert_eq!(reset[0].status, "pending");
        assert_eq!(reset[0].attempt_count, 0);
        assert_eq!(reset[0].next_attempt_at_unix_secs, Some(150));
    }

    #[tokio::test]
    async fn notification_outbox_allows_multiple_items_per_log() {
        let repository = InMemoryRiskControlRepository::new();
        repository
            .insert_notification_outbox(outbox_record_with_item(
                "log-1",
                "risk_control_auto_action",
            ))
            .await
            .expect("admin outbox insert should succeed");
        repository
            .insert_notification_outbox(outbox_record_with_item(
                "log-1",
                "risk_control_user_action_notice",
            ))
            .await
            .expect("user action outbox insert should succeed");

        let items = repository
            .list_notification_outbox_by_log_ids(&["log-1".to_string()])
            .await
            .expect("outboxes should load");

        assert_eq!(items.len(), 2);
        assert!(items
            .iter()
            .any(|item| item.item_key == "risk_control_auto_action"));
        assert!(items
            .iter()
            .any(|item| item.item_key == "risk_control_user_action_notice"));
    }

    #[tokio::test]
    async fn notification_outbox_retry_resets_all_non_sent_items_for_log() {
        let repository = InMemoryRiskControlRepository::new();
        repository
            .insert_notification_outbox(outbox_record_with_item("log-1", "risk_control_flagged"))
            .await
            .expect("flagged outbox insert should succeed");
        repository
            .insert_notification_outbox(outbox_record_with_item(
                "log-1",
                "risk_control_auto_action",
            ))
            .await
            .expect("auto action outbox insert should succeed");
        repository
            .insert_notification_outbox(outbox_record_with_item("log-2", "risk_control_flagged"))
            .await
            .expect("other log outbox insert should succeed");

        let claimed = repository
            .claim_due_notification_outbox(100, 200, 10)
            .await
            .expect("claim should succeed");
        assert_eq!(claimed.len(), 3);
        for item in claimed.iter().filter(|item| item.log_id == "log-1") {
            repository
                .record_notification_outbox_attempt(
                    &item.id,
                    false,
                    Some("send failed".to_string()),
                    101,
                    None,
                )
                .await
                .expect("dead letter should record");
        }

        let reset = repository
            .reset_notification_outbox_for_retry("log-1", 150)
            .await
            .expect("reset should succeed");

        assert_eq!(reset.len(), 2);
        assert!(reset.iter().all(|item| item.log_id == "log-1"));
        assert!(reset.iter().all(|item| item.status == "pending"));
        assert!(reset.iter().all(|item| item.attempt_count == 0));
        assert!(reset
            .iter()
            .all(|item| item.next_attempt_at_unix_secs == Some(150)));

        let other = repository
            .list_notification_outbox_by_log_ids(&["log-2".to_string()])
            .await
            .expect("other log should load");
        assert_eq!(other[0].status, "processing");
    }
}
