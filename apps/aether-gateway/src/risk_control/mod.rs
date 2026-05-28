mod config;
mod input;
mod provider;

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_data::repository::risk_control::{
    InsertRiskControlLogRecord, InsertRiskControlNotificationOutboxRecord,
    StoredRiskControlNotificationOutbox, UpsertRiskControlFlaggedHashRecord,
};
use axum::body::Bytes;
use axum::http;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub(crate) use self::config::{
    read_risk_control_runtime_config, RiskControlKeywordMatchMode, RiskControlKeywordMode,
    RiskControlMode, RiskControlModelFilterMode, RiskControlRuntimeConfig,
    RiskControlScopeMatchContext, RiskControlScopeMode, RISK_CONTROL_CONFIG_KEY,
    RISK_CONTROL_ENABLED_CONFIG_KEY,
};
pub(crate) use self::input::risk_control_requires_buffered_body;
use self::input::{extract_inspection_input, RiskControlInspectionInput};
use self::provider::ModerationOutcome;
pub(crate) use self::provider::{provider_key_statuses, RiskControlProviderKeyStatus};

use crate::control::{
    GatewayControlAuthContext, GatewayControlDecision, GatewayPublicRequestContext,
};
use crate::important_notification::{
    important_notification_dispatch_ready_for_item, send_important_notification_for_item,
    ImportantNotification, ImportantNotificationDeliveryReport, RISK_CONTROL_AUTO_ACTION_ITEM_KEY,
    RISK_CONTROL_FLAGGED_ITEM_KEY, RISK_CONTROL_USER_ACTION_NOTICE_ITEM_KEY,
};
use crate::{AppState, GatewayError};

const MAX_REQUEST_ID_CHARS: usize = 100;
const OBSERVE_QUEUE_HARD_CAPACITY: usize = 65_536;
const OBSERVE_QUEUE_DEFAULT_SOFT_CAPACITY: usize = 1024;
const NOTIFICATION_OUTBOX_MAX_ATTEMPTS: u64 = 10;
const NOTIFICATION_OUTBOX_BATCH_SIZE: usize = 20;
const NOTIFICATION_OUTBOX_POLL_INTERVAL: Duration = Duration::from_secs(30);
const NOTIFICATION_OUTBOX_LEASE_SECS: u64 = 5 * 60;
const NOTIFICATION_OUTBOX_READINESS_RETRY_SECS: u64 = 5 * 60;
const NOTIFICATION_OUTBOX_BACKOFF_SECS: [u64; NOTIFICATION_OUTBOX_MAX_ATTEMPTS as usize] =
    [0, 30, 120, 300, 900, 1_800, 3_600, 7_200, 14_400, 28_800];
const MAX_NOTIFICATION_ERROR_CHARS: usize = 500;
const MAX_REGEX_KEYWORD_ITEMS: usize = 100;
const MAX_REGEX_PATTERN_CHARS: usize = 256;
const MAX_REGEX_COMPLEXITY_SCORE: usize = 600;
const MAX_REGEX_PROGRAM_SIZE_BYTES: usize = 256 * 1024;
const MAX_REGEX_MATCH_RANGES: usize = 256;
const MAX_REGEX_SCAN_CHARS: usize = 64 * 1024;
const MAX_REGEX_TOTAL_SCAN_CHARS: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct RiskControlBlockDecision {
    pub(crate) status_code: u16,
    pub(crate) message: String,
    pub(crate) action: String,
    pub(crate) decision_source: String,
    pub(crate) input_hash: String,
}

#[derive(Clone, Debug)]
struct InspectionDecision {
    action: &'static str,
    decision_source: &'static str,
    flagged: bool,
    highest_category: Option<String>,
    highest_score: f64,
    category_scores: Option<serde_json::Value>,
    matched_keywords: Vec<String>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct KeywordMatchOutcome {
    matched_keywords: Vec<String>,
    regex_scan_limited: bool,
    regex_pattern_limited: bool,
    regex_invalid_pattern_count: usize,
    regex_scan_chars: usize,
    regex_pattern_count: usize,
    regex_total_scan_budget_chars: usize,
}

#[derive(Debug, Clone, Default)]
struct NotificationDispatchOutcome {
    attempted: bool,
    sent: bool,
    attempts: u64,
    last_error: Option<String>,
    last_attempt_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone)]
struct NotificationOutboxPlan {
    item_key: &'static str,
    title: String,
    text: String,
}

#[derive(Debug, Clone)]
struct AutoActionApplyOutcome {
    applied: bool,
    target_type: &'static str,
    target_id: String,
}

struct RiskControlObserveJob {
    request_context: GatewayPublicRequestContext,
    config: RiskControlRuntimeConfig,
    input: RiskControlInspectionInput,
    started_at: Instant,
    enqueued_at: Instant,
    request_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum RiskControlObserveEnqueueError {
    Full,
    Closed,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RiskControlObserveQueueSnapshot {
    pub(crate) capacity: usize,
    pub(crate) configured_capacity: usize,
    pub(crate) queued: u64,
    pub(crate) enqueued_total: u64,
    pub(crate) dropped_total: u64,
    pub(crate) processed_total: u64,
    pub(crate) failed_total: u64,
}

pub(crate) struct RiskControlObserveQueue {
    hard_capacity: usize,
    soft_capacity: AtomicUsize,
    sender: mpsc::Sender<RiskControlObserveJob>,
    receiver: StdMutex<Option<mpsc::Receiver<RiskControlObserveJob>>>,
    queued: AtomicU64,
    enqueued_total: AtomicU64,
    dropped_total: AtomicU64,
    processed_total: AtomicU64,
    failed_total: AtomicU64,
}

impl fmt::Debug for RiskControlObserveQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RiskControlObserveQueue")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl Default for RiskControlObserveQueue {
    fn default() -> Self {
        Self::new(
            OBSERVE_QUEUE_HARD_CAPACITY,
            OBSERVE_QUEUE_DEFAULT_SOFT_CAPACITY,
        )
    }
}

impl RiskControlObserveQueue {
    pub(crate) fn new(hard_capacity: usize, soft_capacity: usize) -> Self {
        let hard_capacity = hard_capacity.max(1);
        let soft_capacity = soft_capacity.clamp(1, hard_capacity);
        let (sender, receiver) = mpsc::channel(hard_capacity);
        Self {
            hard_capacity,
            soft_capacity: AtomicUsize::new(soft_capacity),
            sender,
            receiver: StdMutex::new(Some(receiver)),
            queued: AtomicU64::new(0),
            enqueued_total: AtomicU64::new(0),
            dropped_total: AtomicU64::new(0),
            processed_total: AtomicU64::new(0),
            failed_total: AtomicU64::new(0),
        }
    }

    fn enqueue(&self, job: RiskControlObserveJob) -> Result<(), RiskControlObserveEnqueueError> {
        if !self.try_mark_queued() {
            self.dropped_total.fetch_add(1, Ordering::Relaxed);
            return Err(RiskControlObserveEnqueueError::Full);
        }
        match self.sender.try_send(job) {
            Ok(()) => {
                self.enqueued_total.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.mark_dequeued();
                self.dropped_total.fetch_add(1, Ordering::Relaxed);
                Err(RiskControlObserveEnqueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.mark_dequeued();
                self.dropped_total.fetch_add(1, Ordering::Relaxed);
                Err(RiskControlObserveEnqueueError::Closed)
            }
        }
    }

    pub(crate) fn set_soft_capacity(&self, capacity: usize) {
        self.soft_capacity
            .store(capacity.clamp(1, self.hard_capacity), Ordering::Relaxed);
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<RiskControlObserveJob>> {
        self.receiver
            .lock()
            .expect("risk control observe queue receiver lock")
            .take()
    }

    fn try_mark_queued(&self) -> bool {
        let soft_capacity = self.soft_capacity.load(Ordering::Relaxed) as u64;
        let mut current = self.queued.load(Ordering::Relaxed);
        loop {
            if current >= soft_capacity {
                return false;
            }
            match self.queued.compare_exchange(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(next) => current = next,
            }
        }
    }

    fn mark_dequeued(&self) {
        let mut current = self.queued.load(Ordering::Relaxed);
        while current > 0 {
            match self.queued.compare_exchange(
                current,
                current - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(next) => current = next,
            }
        }
    }

    fn mark_processed(&self) {
        self.processed_total.fetch_add(1, Ordering::Relaxed);
    }

    fn mark_failed(&self) {
        self.failed_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> RiskControlObserveQueueSnapshot {
        RiskControlObserveQueueSnapshot {
            capacity: self.hard_capacity,
            configured_capacity: self.soft_capacity.load(Ordering::Relaxed),
            queued: self.queued.load(Ordering::Relaxed),
            enqueued_total: self.enqueued_total.load(Ordering::Relaxed),
            dropped_total: self.dropped_total.load(Ordering::Relaxed),
            processed_total: self.processed_total.load(Ordering::Relaxed),
            failed_total: self.failed_total.load(Ordering::Relaxed),
        }
    }
}

pub(crate) fn spawn_risk_control_observe_worker(state: AppState) -> Option<JoinHandle<()>> {
    let mut receiver = state.risk_control_observe_queue.take_receiver()?;
    Some(tokio::spawn(async move {
        while let Some(job) = receiver.recv().await {
            state.risk_control_observe_queue.mark_dequeued();
            let queue_delay_ms = job.enqueued_at.elapsed().as_millis() as u64;
            let auth_context = job
                .request_context
                .control_decision
                .as_ref()
                .and_then(|decision| decision.auth_context.as_ref());
            let result = inspect_and_record(
                &state,
                &job.request_context,
                auth_context,
                &job.config,
                &job.input,
                job.started_at,
                queue_delay_ms,
                job.request_id,
            )
            .await;
            match result {
                Ok(_) => state.risk_control_observe_queue.mark_processed(),
                Err(err) => {
                    state.risk_control_observe_queue.mark_failed();
                    warn!(
                        event_name = "risk_control_observe_worker_failed",
                        log_type = "ops",
                        trace_id = %job.request_context.trace_id,
                        error = ?err,
                        "risk control observe worker failed"
                    );
                }
            }
        }
    }))
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RiskControlNotificationOutboxRunSummary {
    pub(crate) claimed: u64,
    pub(crate) sent: u64,
    pub(crate) failed: u64,
    pub(crate) deferred: u64,
    pub(crate) dead: u64,
}

pub(crate) fn spawn_risk_control_notification_worker(state: AppState) -> Option<JoinHandle<()>> {
    if !state.data.has_risk_control_writer() {
        return None;
    }

    Some(tokio::spawn(async move {
        loop {
            match run_risk_control_notification_outbox_once(&state).await {
                Ok(summary) if summary.claimed > 0 => {
                    info!(
                        event_name = "risk_control_notification_outbox_run",
                        log_type = "ops",
                        claimed = summary.claimed,
                        sent = summary.sent,
                        failed = summary.failed,
                        deferred = summary.deferred,
                        dead = summary.dead,
                        "risk control notification outbox run completed"
                    );
                }
                Ok(_) => {}
                Err(err) => warn!(
                    event_name = "risk_control_notification_outbox_run_failed",
                    log_type = "ops",
                    error = ?err,
                    "risk control notification outbox run failed"
                ),
            }
            tokio::time::sleep(NOTIFICATION_OUTBOX_POLL_INTERVAL).await;
        }
    }))
}

pub(crate) async fn run_risk_control_notification_outbox_once(
    state: &AppState,
) -> Result<RiskControlNotificationOutboxRunSummary, GatewayError> {
    let now = current_unix_secs();
    let lease_until = now.saturating_add(NOTIFICATION_OUTBOX_LEASE_SECS);
    let items = state
        .data
        .claim_due_risk_control_notification_outbox(
            now,
            lease_until,
            NOTIFICATION_OUTBOX_BATCH_SIZE,
        )
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let mut summary = RiskControlNotificationOutboxRunSummary {
        claimed: items.len() as u64,
        ..RiskControlNotificationOutboxRunSummary::default()
    };
    for item in items {
        let outcome = deliver_notification_outbox_item(state, &item).await;
        if outcome.sent {
            summary.sent = summary.sent.saturating_add(1);
        } else if outcome.deferred {
            summary.deferred = summary.deferred.saturating_add(1);
        } else {
            summary.failed = summary.failed.saturating_add(1);
            if outcome.dead {
                summary.dead = summary.dead.saturating_add(1);
            }
        }
    }
    Ok(summary)
}

pub(crate) async fn maybe_inspect_gateway_request(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    body: &Bytes,
) -> Result<Option<RiskControlBlockDecision>, GatewayError> {
    let config = read_risk_control_runtime_config(state).await?;
    if !config.enabled || config.mode == RiskControlMode::Off {
        return Ok(None);
    }
    if !risk_control_requires_buffered_body(request_context) {
        return Ok(None);
    }
    let Some(input) = extract_inspection_input(
        request_context.control_decision.as_ref(),
        headers,
        body,
        config.max_text_chars,
        config.excerpt_chars,
    )?
    else {
        return Ok(None);
    };
    let control_decision = request_context.control_decision.as_ref();
    let auth_context = control_decision.and_then(|decision| decision.auth_context.as_ref());
    let user_group_ids = if config.scope.user_groups.mode != RiskControlScopeMode::All {
        match auth_context {
            Some(context) => state
                .list_user_groups_for_user(&context.user_id)
                .await?
                .into_iter()
                .map(|group| group.id)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let scope_context = RiskControlScopeMatchContext {
        user_id: auth_context.map(|context| context.user_id.as_str()),
        user_group_ids: &user_group_ids,
        api_key_id: auth_context.map(|context| context.api_key_id.as_str()),
        route_family: control_decision.and_then(|decision| decision.route_family.as_deref()),
        route_kind: control_decision.and_then(|decision| decision.route_kind.as_deref()),
        endpoint: Some(input.endpoint.as_str()),
        model: input.model.as_deref(),
    };
    if !config.includes_scope(&scope_context) {
        return Ok(None);
    }
    if !sample_allows(&input.input_hash, config.sample_rate) {
        return Ok(None);
    }

    let started_at = std::time::Instant::now();
    let request_id = extract_request_id_from_headers(headers);

    if let Some(hash_hit) = if config.hash_block.enabled {
        state
            .data
            .get_risk_control_flagged_hash(&input.input_hash)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
    } else {
        None
    } {
        let decision = InspectionDecision {
            action: if config.mode == RiskControlMode::PreBlock {
                "block"
            } else {
                "observe"
            },
            decision_source: "hash",
            flagged: true,
            highest_category: hash_hit.highest_category,
            highest_score: hash_hit.highest_score,
            category_scores: None,
            matched_keywords: Vec::new(),
            error_message: None,
        };
        write_decision_log(
            state,
            request_context,
            auth_context,
            &config,
            &input,
            decision.clone(),
            started_at.elapsed().as_millis() as u64,
            0,
            request_id.clone(),
        )
        .await?;
        state
            .data
            .upsert_risk_control_flagged_hash(UpsertRiskControlFlaggedHashRecord {
                input_hash: input.input_hash.clone(),
                source_log_id: None,
                reason: Some("hash_hit".to_string()),
                highest_category: decision.highest_category.clone(),
                highest_score: decision.highest_score,
                excerpt: Some(input.excerpt.clone()),
                now_unix_secs: None,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if config.mode == RiskControlMode::PreBlock {
            return Ok(Some(block_decision(&config, &decision, &input)));
        }
        return Ok(None);
    }

    if config.mode == RiskControlMode::Observe
        && should_call_moderation_api(&config)
        && config.keyword_mode != RiskControlKeywordMode::KeywordOnly
    {
        state
            .risk_control_observe_queue
            .set_soft_capacity(config.observe.queue_capacity);
        match state
            .risk_control_observe_queue
            .enqueue(RiskControlObserveJob {
                request_context: request_context.clone(),
                config: config.clone(),
                input,
                started_at,
                enqueued_at: Instant::now(),
                request_id,
            }) {
            Ok(()) => {}
            Err(RiskControlObserveEnqueueError::Full) => warn!(
                event_name = "risk_control_observe_queue_full",
                log_type = "ops",
                trace_id = %request_context.trace_id,
                "risk control observe queue is full; background inspection dropped"
            ),
            Err(RiskControlObserveEnqueueError::Closed) => warn!(
                event_name = "risk_control_observe_queue_closed",
                log_type = "ops",
                trace_id = %request_context.trace_id,
                "risk control observe queue is closed; background inspection dropped"
            ),
        }
        return Ok(None);
    }

    let decision = inspect_and_record(
        state,
        request_context,
        auth_context,
        &config,
        &input,
        started_at,
        0,
        request_id,
    )
    .await?;
    if config.mode == RiskControlMode::PreBlock && decision.flagged {
        return Ok(Some(block_decision(&config, &decision, &input)));
    }
    Ok(None)
}

pub(crate) async fn risk_control_should_buffer_body(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<bool, GatewayError> {
    if !risk_control_requires_buffered_body(request_context) {
        return Ok(false);
    }

    let enabled = state
        .read_system_config_json_value(RISK_CONTROL_ENABLED_CONFIG_KEY)
        .await?
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if !enabled {
        return Ok(false);
    }

    let config = read_risk_control_runtime_config(state).await?;
    Ok(config.mode != RiskControlMode::Off)
}

pub(crate) async fn test_risk_control_text(
    state: &AppState,
    config: &RiskControlRuntimeConfig,
    text: &str,
) -> Result<serde_json::Value, GatewayError> {
    let text = text.trim();
    let keyword_outcome = keyword_outcome_for_config(config, text);
    let keyword_decision_source = keyword_decision_source(config, &keyword_outcome);
    let moderation = if should_call_moderation_api(config)
        && !matches!(config.keyword_mode, RiskControlKeywordMode::KeywordOnly)
    {
        Some(provider::moderate_text(state, &config.provider, &config.thresholds, text).await)
    } else {
        None
    };
    let decision = build_inspection_decision(
        config,
        keyword_decision_source,
        keyword_outcome.matched_keywords,
        moderation,
    );
    Ok(json!({
        "action": decision.action,
        "decision_source": decision.decision_source,
        "flagged": decision.flagged,
        "highest_category": decision.highest_category,
        "highest_score": decision.highest_score,
        "category_scores": decision.category_scores,
        "matched_keywords": decision.matched_keywords,
        "regex_scan_limited": keyword_outcome.regex_scan_limited,
        "regex_pattern_limited": keyword_outcome.regex_pattern_limited,
        "regex_invalid_pattern_count": keyword_outcome.regex_invalid_pattern_count,
        "regex_scan_chars": keyword_outcome.regex_scan_chars,
        "regex_pattern_count": keyword_outcome.regex_pattern_count,
        "regex_total_scan_budget_chars": keyword_outcome.regex_total_scan_budget_chars,
        "error_message": decision.error_message,
    }))
}

async fn inspect_and_record(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    auth_context: Option<&GatewayControlAuthContext>,
    config: &RiskControlRuntimeConfig,
    input: &RiskControlInspectionInput,
    started_at: std::time::Instant,
    queue_delay_ms: u64,
    request_id: Option<String>,
) -> Result<InspectionDecision, GatewayError> {
    let keyword_outcome = keyword_outcome_for_config(config, &input.keyword_text);
    emit_regex_keyword_runtime_warnings(request_context, &keyword_outcome);
    let keyword_decision_source = keyword_decision_source(config, &keyword_outcome);

    let moderation = if should_call_moderation_api(config)
        && !matches!(config.keyword_mode, RiskControlKeywordMode::KeywordOnly)
    {
        match provider::moderate_text(state, &config.provider, &config.thresholds, &input.text)
            .await
        {
            Ok(outcome) => Some(Ok(outcome)),
            Err(err) => Some(Err(err)),
        }
    } else {
        None
    };

    let decision = build_inspection_decision(
        config,
        keyword_decision_source,
        keyword_outcome.matched_keywords,
        moderation,
    );
    if decision.flagged || decision.error_message.is_some() || config.log_all {
        let log = write_decision_log(
            state,
            request_context,
            auth_context,
            config,
            input,
            decision.clone(),
            started_at.elapsed().as_millis() as u64,
            queue_delay_ms,
            request_id,
        )
        .await?;
        if should_learn_flagged_hash(config, &decision) {
            state
                .data
                .upsert_risk_control_flagged_hash(UpsertRiskControlFlaggedHashRecord {
                    input_hash: input.input_hash.clone(),
                    source_log_id: log.map(|log| log.id),
                    reason: Some(decision.decision_source.to_string()),
                    highest_category: decision.highest_category.clone(),
                    highest_score: decision.highest_score,
                    excerpt: Some(input.excerpt.clone()),
                    now_unix_secs: None,
                })
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
        }
    }
    Ok(decision)
}

fn build_inspection_decision(
    config: &RiskControlRuntimeConfig,
    keyword_decision_source: Option<&'static str>,
    matched_keywords: Vec<String>,
    moderation: Option<Result<ModerationOutcome, GatewayError>>,
) -> InspectionDecision {
    let keyword_flagged = keyword_decision_source.is_some();
    match moderation {
        Some(Ok(outcome)) => {
            let flagged = keyword_flagged || outcome.flagged;
            InspectionDecision {
                action: action_for(config, flagged),
                decision_source: keyword_decision_source.unwrap_or("api"),
                flagged,
                highest_category: outcome.highest_category,
                highest_score: outcome.highest_score,
                category_scores: Some(outcome.category_scores),
                matched_keywords,
                error_message: None,
            }
        }
        Some(Err(err)) => {
            let flagged = keyword_flagged || config.provider.fail_closed;
            InspectionDecision {
                action: action_for(config, flagged),
                decision_source: keyword_decision_source.unwrap_or("api_error"),
                flagged,
                highest_category: None,
                highest_score: 0.0,
                category_scores: None,
                matched_keywords,
                error_message: Some(format!("{err:?}")),
            }
        }
        None => {
            let flagged = keyword_flagged;
            InspectionDecision {
                action: action_for(config, flagged),
                decision_source: keyword_decision_source.unwrap_or("none"),
                flagged,
                highest_category: None,
                highest_score: 0.0,
                category_scores: None,
                matched_keywords,
                error_message: None,
            }
        }
    }
}

async fn write_decision_log(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    auth_context: Option<&GatewayControlAuthContext>,
    config: &RiskControlRuntimeConfig,
    input: &RiskControlInspectionInput,
    decision: InspectionDecision,
    latency_ms: u64,
    queue_delay_ms: u64,
    request_id: Option<String>,
) -> Result<Option<aether_data::repository::risk_control::StoredRiskControlLog>, GatewayError> {
    let (violation_count, auto_action) =
        resolve_auto_action(state, auth_context, config, decision.flagged).await?;
    let enforce_auto_action = should_enforce_auto_action(config.mode, auto_action.as_deref());
    let user_email = if config.notification.enabled {
        fetch_user_email(state, auth_context).await
    } else {
        None
    };
    let control_decision = request_context.control_decision.as_ref();
    let matched_keywords = (!decision.matched_keywords.is_empty())
        .then(|| serde_json::to_value(&decision.matched_keywords).unwrap_or_else(|_| json!([])));
    let thresholds = (!config.thresholds.is_empty()).then(|| json!(config.thresholds));
    let mut log = state
        .data
        .insert_risk_control_log(InsertRiskControlLogRecord {
            id: None,
            trace_id: request_context.trace_id.clone(),
            request_id,
            user_id: auth_context.map(|context| context.user_id.clone()),
            username: auth_context.and_then(|context| context.username.clone()),
            user_email: user_email.clone(),
            api_key_id: auth_context.map(|context| context.api_key_id.clone()),
            api_key_name: auth_context.and_then(|context| context.api_key_name.clone()),
            route_family: control_decision.and_then(|decision| decision.route_family.clone()),
            route_kind: control_decision.and_then(|decision| decision.route_kind.clone()),
            api_format: control_decision
                .and_then(|decision| decision.auth_endpoint_signature.clone()),
            endpoint: input.endpoint.clone(),
            model: input.model.clone(),
            mode: config.mode.as_str().to_string(),
            action: decision.action.to_string(),
            decision_source: decision.decision_source.to_string(),
            flagged: decision.flagged,
            highest_category: decision.highest_category.clone(),
            highest_score: decision.highest_score,
            category_scores: decision.category_scores.clone(),
            thresholds,
            matched_keywords,
            input_hash: Some(input.input_hash.clone()),
            excerpt: Some(input.excerpt.clone()),
            latency_ms: Some(latency_ms),
            queue_delay_ms: (queue_delay_ms > 0).then_some(queue_delay_ms),
            violation_count,
            auto_action: auto_action.clone(),
            notification_sent: false,
            notification_attempts: 0,
            notification_last_error: None,
            notification_last_attempt_at_unix_secs: None,
            error_message: decision.error_message.clone(),
            created_at_unix_secs: None,
        })
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;

    let auto_action_outcome = if enforce_auto_action {
        if let Some(action) = auto_action.as_deref() {
            Some(apply_auto_action(state, auth_context, action).await?)
        } else {
            None
        }
    } else {
        None
    };
    if let Some(outcome) = auto_action_outcome.as_ref() {
        info!(
            event_name = "risk_control_auto_action_applied",
            log_type = "audit",
            trace_id = %request_context.trace_id,
            log_id = log.as_ref().map(|item| item.id.as_str()).unwrap_or("-"),
            action = auto_action.as_deref().unwrap_or("none"),
            applied = outcome.applied,
            target_type = outcome.target_type,
            target_id = %outcome.target_id,
            violation_count,
            "risk control auto action audit"
        );
    }

    let notification_outcome = maybe_dispatch_notification(
        state,
        log.as_ref().map(|item| item.id.as_str()),
        config,
        request_context,
        auth_context,
        user_email.as_deref(),
        &decision,
        input,
        violation_count,
        enforce_auto_action
            .then_some(auto_action.as_deref())
            .flatten(),
    )
    .await
    .unwrap_or_default();
    if let Some(item) = log.as_mut() {
        item.notification_sent = notification_outcome.sent;
        item.notification_attempts = notification_outcome.attempts;
        item.notification_last_error = notification_outcome.last_error.clone();
        item.notification_last_attempt_at_unix_secs =
            notification_outcome.last_attempt_at_unix_secs;
    }

    let notification_sent = notification_outcome.sent;
    info!(
        event_name = "risk_control_decision",
        log_type = "security",
        trace_id = %request_context.trace_id,
        action = decision.action,
        decision_source = decision.decision_source,
        flagged = decision.flagged,
        input_hash = %input.input_hash,
        violation_count,
        queue_delay_ms,
        auto_action = auto_action.as_deref().unwrap_or("none"),
        auto_action_enforced = enforce_auto_action,
        notification_sent,
        notification_attempted = notification_outcome.attempted,
        notification_attempts = notification_outcome.attempts,
        "risk control inspected ai request"
    );
    Ok(log)
}

async fn fetch_user_email(
    state: &AppState,
    auth_context: Option<&GatewayControlAuthContext>,
) -> Option<String> {
    let auth_context = auth_context?;
    state
        .data
        .find_user_auth_by_id(&auth_context.user_id)
        .await
        .ok()
        .flatten()
        .and_then(|user| user.email)
}

#[allow(clippy::too_many_arguments)]
async fn maybe_dispatch_notification(
    state: &AppState,
    log_id: Option<&str>,
    config: &RiskControlRuntimeConfig,
    request_context: &GatewayPublicRequestContext,
    auth_context: Option<&GatewayControlAuthContext>,
    user_email: Option<&str>,
    decision: &InspectionDecision,
    input: &RiskControlInspectionInput,
    violation_count: u64,
    enforced_auto_action: Option<&str>,
) -> Option<NotificationDispatchOutcome> {
    if !config.notification.enabled {
        return None;
    }

    let mut plans = Vec::new();
    if let Some(action) = enforced_auto_action {
        if config.notification.notify_on_auto_action {
            plans.push(NotificationOutboxPlan {
                item_key: RISK_CONTROL_AUTO_ACTION_ITEM_KEY,
                title: format!("风控自动处置：{action}"),
                text: format!("已对用户执行 {action}，累计违规 {violation_count} 次。"),
            });
        }
        if config.notification.notify_on_user_action_notice {
            plans.push(NotificationOutboxPlan {
                item_key: RISK_CONTROL_USER_ACTION_NOTICE_ITEM_KEY,
                title: format!("风控用户处置通知：{action}"),
                text: format!("风控中心已触发用户处置：{action}，累计违规 {violation_count} 次。"),
            });
        }
    } else if decision.flagged && config.notification.notify_on_flagged {
        plans.push(NotificationOutboxPlan {
            item_key: RISK_CONTROL_FLAGGED_ITEM_KEY,
            title: format!("风控命中：{}", decision.decision_source),
            text: format!(
                "风控命中：source={} action={} keywords={}",
                decision.decision_source,
                decision.action,
                decision.matched_keywords.join(",")
            ),
        });
    }

    if plans.is_empty() {
        return None;
    }

    let user_label = auth_context
        .and_then(|context| context.username.clone())
        .or_else(|| user_email.map(ToOwned::to_owned))
        .or_else(|| auth_context.map(|context| context.user_id.clone()))
        .unwrap_or_else(|| "-".to_string());
    let matched_keywords_display = if decision.matched_keywords.is_empty() {
        "-".to_string()
    } else {
        decision.matched_keywords.join(", ")
    };
    let variables: Vec<(&str, String)> = vec![
        ("trace_id", request_context.trace_id.clone()),
        ("user_label", user_label.clone()),
        (
            "user_id",
            auth_context
                .map(|context| context.user_id.clone())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("user_email", user_email.unwrap_or("-").to_string()),
        ("decision_source", decision.decision_source.to_string()),
        ("action", decision.action.to_string()),
        (
            "model",
            input.model.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("endpoint", input.endpoint.clone()),
        ("matched_keywords", matched_keywords_display),
        ("excerpt", notification_excerpt_value(config, input)),
        ("violation_count", violation_count.to_string()),
        (
            "auto_action",
            enforced_auto_action.unwrap_or("-").to_string(),
        ),
    ];

    let Some(log_id) = log_id else {
        return Some(NotificationDispatchOutcome::default());
    };
    let now = current_unix_secs();
    let variables_json = notification_variables_json(&variables);
    let mut queued = 0u64;
    let mut errors = Vec::new();
    for plan in plans {
        match state
            .data
            .insert_risk_control_notification_outbox(InsertRiskControlNotificationOutboxRecord {
                id: None,
                log_id: log_id.to_string(),
                item_key: plan.item_key.to_string(),
                title: plan.title,
                markdown_body: plan.text.clone(),
                text_body: plan.text,
                variables_json: variables_json.clone(),
                max_attempts: NOTIFICATION_OUTBOX_MAX_ATTEMPTS,
                next_attempt_at_unix_secs: Some(now),
                created_at_unix_secs: Some(now),
            })
            .await
        {
            Ok(Some(outbox)) => {
                queued = queued.saturating_add(1);
                info!(
                    event_name = "risk_control_notification_queued",
                    log_type = "ops",
                    trace_id = %request_context.trace_id,
                    log_id,
                    outbox_id = %outbox.id,
                    item_key = plan.item_key,
                    "risk control notification queued"
                );
            }
            Ok(None) => {}
            Err(err) => {
                let error =
                    truncate_notification_error(format!("{} 通知入队失败：{err:?}", plan.item_key));
                errors.push(error.clone());
                warn!(
                    event_name = "risk_control_notification_enqueue_failed",
                    log_type = "ops",
                    trace_id = %request_context.trace_id,
                    log_id,
                    item_key = plan.item_key,
                    error = ?err,
                    "risk control notification enqueue failed"
                );
            }
        }
    }

    if errors.is_empty() {
        Some(NotificationDispatchOutcome {
            attempted: queued > 0,
            sent: false,
            attempts: 0,
            last_error: None,
            last_attempt_at_unix_secs: None,
        })
    } else {
        let error = truncate_notification_error(errors.join("; "));
        record_notification_attempt(state, Some(log_id), false, Some(error.clone())).await;
        Some(NotificationDispatchOutcome {
            attempted: true,
            sent: false,
            attempts: 1,
            last_error: Some(error),
            last_attempt_at_unix_secs: Some(current_unix_secs()),
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct NotificationOutboxDeliveryOutcome {
    sent: bool,
    deferred: bool,
    dead: bool,
}

#[derive(Debug, Clone)]
enum NotificationOutboxDeliveryResult {
    Sent,
    Failed(String),
    Deferred(String),
}

async fn deliver_notification_outbox_item(
    state: &AppState,
    item: &StoredRiskControlNotificationOutbox,
) -> NotificationOutboxDeliveryOutcome {
    let attempted_at = current_unix_secs();
    let result = match important_notification_dispatch_ready_for_item(state, &item.item_key).await {
        Ok(true) => {
            let variables = notification_variables_from_json(&item.variables_json);
            let variable_refs = variables
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect::<Vec<_>>();
            match send_important_notification_for_item(
                state,
                &item.item_key,
                ImportantNotification {
                    title: item.title.clone(),
                    markdown_body: item.markdown_body.clone(),
                    text_body: item.text_body.clone(),
                },
                &variable_refs,
            )
            .await
            {
                Ok(report) if report.success => NotificationOutboxDeliveryResult::Sent,
                Ok(report) => NotificationOutboxDeliveryResult::Failed(
                    truncate_notification_error(notification_report_error(&report)),
                ),
                Err(err) => NotificationOutboxDeliveryResult::Failed(truncate_notification_error(
                    format!("{err:?}"),
                )),
            }
        }
        Ok(false) => {
            NotificationOutboxDeliveryResult::Deferred("通知中心事件或渠道未就绪".to_string())
        }
        Err(err) => NotificationOutboxDeliveryResult::Deferred(truncate_notification_error(
            format!("通知中心状态读取失败：{err:?}"),
        )),
    };

    let (sent, error) = match result {
        NotificationOutboxDeliveryResult::Sent => (true, None),
        NotificationOutboxDeliveryResult::Failed(error) => (false, Some(error)),
        NotificationOutboxDeliveryResult::Deferred(error) => {
            let next_attempt_at =
                attempted_at.saturating_add(NOTIFICATION_OUTBOX_READINESS_RETRY_SECS);
            if let Err(err) = state
                .data
                .defer_risk_control_notification_outbox_attempt(
                    &item.id,
                    Some(error.clone()),
                    attempted_at,
                    next_attempt_at,
                )
                .await
            {
                warn!(
                    event_name = "risk_control_notification_outbox_defer_failed",
                    log_type = "ops",
                    outbox_id = %item.id,
                    log_id = %item.log_id,
                    error = ?err,
                    "failed to defer risk control notification outbox attempt"
                );
            }
            warn!(
                event_name = "risk_control_notification_deferred",
                log_type = "ops",
                outbox_id = %item.id,
                log_id = %item.log_id,
                item_key = %item.item_key,
                next_attempt_at,
                error = %error,
                "risk control notification deferred until notification center is ready"
            );
            return NotificationOutboxDeliveryOutcome {
                sent: false,
                deferred: true,
                dead: false,
            };
        }
    };
    let attempts_after = item.attempt_count.saturating_add(1);
    let next_attempt_at = if sent || attempts_after >= item.max_attempts {
        None
    } else {
        Some(attempted_at.saturating_add(notification_retry_delay_secs(attempts_after)))
    };
    let dead = !sent && next_attempt_at.is_none();

    if let Err(err) = state
        .data
        .record_risk_control_notification_outbox_attempt(
            &item.id,
            sent,
            error.clone(),
            attempted_at,
            next_attempt_at,
        )
        .await
    {
        warn!(
            event_name = "risk_control_notification_outbox_record_failed",
            log_type = "ops",
            outbox_id = %item.id,
            log_id = %item.log_id,
            error = ?err,
            "failed to record risk control notification outbox attempt"
        );
    }
    record_notification_attempt(state, Some(&item.log_id), sent, error.clone()).await;

    if !sent {
        warn!(
            event_name = "risk_control_notification_attempt_failed",
            log_type = "ops",
            outbox_id = %item.id,
            log_id = %item.log_id,
            item_key = %item.item_key,
            attempt = attempts_after,
            max_attempts = item.max_attempts,
            dead,
            next_attempt_at = next_attempt_at.unwrap_or_default(),
            error = %error.unwrap_or_else(|| "unknown".to_string()),
            "risk control notification attempt failed"
        );
    }

    NotificationOutboxDeliveryOutcome {
        sent,
        deferred: false,
        dead,
    }
}

fn notification_retry_delay_secs(attempts_after_failure: u64) -> u64 {
    NOTIFICATION_OUTBOX_BACKOFF_SECS
        .get(attempts_after_failure as usize)
        .copied()
        .unwrap_or_else(|| *NOTIFICATION_OUTBOX_BACKOFF_SECS.last().unwrap_or(&28_800))
}

fn notification_variables_json(variables: &[(&str, String)]) -> serde_json::Value {
    serde_json::Value::Object(
        variables
            .iter()
            .map(|(key, value)| (key.to_string(), serde_json::Value::String(value.clone())))
            .collect(),
    )
}

fn notification_variables_from_json(value: &serde_json::Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| value.to_string()),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn record_notification_attempt(
    state: &AppState,
    log_id: Option<&str>,
    sent: bool,
    error_message: Option<String>,
) {
    let Some(log_id) = log_id else {
        return;
    };
    let attempted_at = current_unix_secs();
    match state
        .data
        .record_risk_control_log_notification_result(log_id, sent, error_message, attempted_at)
        .await
    {
        Ok(true) => {}
        Ok(false) => warn!(
            event_name = "risk_control_notification_log_missing",
            log_type = "ops",
            log_id,
            "risk control notification result could not be recorded because log row was not found"
        ),
        Err(err) => warn!(
            event_name = "risk_control_notification_result_record_failed",
            log_type = "ops",
            log_id,
            error = ?err,
            "failed to record risk control notification result"
        ),
    }
}

fn notification_report_error(report: &ImportantNotificationDeliveryReport) -> String {
    let messages = report
        .channels
        .iter()
        .filter(|channel| !channel.success)
        .map(|channel| format!("{}: {}", channel.channel, channel.message))
        .collect::<Vec<_>>();
    if messages.is_empty() {
        "通知发送失败".to_string()
    } else {
        messages.join("; ")
    }
}

fn truncate_notification_error(value: String) -> String {
    value.chars().take(MAX_NOTIFICATION_ERROR_CHARS).collect()
}

fn notification_excerpt_value(
    config: &RiskControlRuntimeConfig,
    input: &RiskControlInspectionInput,
) -> String {
    if config.notification.include_excerpt {
        input.excerpt.clone()
    } else {
        "(已隐藏)".to_string()
    }
}

fn extract_request_id_from_headers(headers: &http::HeaderMap) -> Option<String> {
    for name in ["x-request-id", "request-id", "x-amzn-trace-id"] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.chars().take(MAX_REQUEST_ID_CHARS).collect());
            }
        }
    }
    None
}

async fn resolve_auto_action(
    state: &AppState,
    auth_context: Option<&GatewayControlAuthContext>,
    config: &RiskControlRuntimeConfig,
    flagged: bool,
) -> Result<(u64, Option<String>), GatewayError> {
    if !flagged {
        return Ok((0, None));
    }
    let Some(auth_context) = auth_context else {
        return Ok((1, None));
    };
    let since = current_unix_secs().saturating_sub(config.auto_action.window_seconds);
    let previous = state
        .data
        .count_risk_control_flagged_logs_since(&auth_context.user_id, since)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let violation_count = previous.saturating_add(1);
    if !config.auto_action.enabled || violation_count < config.auto_action.violation_threshold {
        return Ok((violation_count, None));
    }
    if config.auto_action.disable_user {
        return Ok((violation_count, Some("disable_user".to_string())));
    }
    if config.auto_action.lock_api_key {
        return Ok((violation_count, Some("lock_api_key".to_string())));
    }
    Ok((violation_count, None))
}

async fn apply_auto_action(
    state: &AppState,
    auth_context: Option<&GatewayControlAuthContext>,
    action: &str,
) -> Result<AutoActionApplyOutcome, GatewayError> {
    let Some(auth_context) = auth_context else {
        return Ok(AutoActionApplyOutcome {
            applied: false,
            target_type: "unknown",
            target_id: "-".to_string(),
        });
    };
    match action {
        "disable_user" => {
            let updated = state
                .update_local_auth_user_admin_fields(
                    &auth_context.user_id,
                    None,
                    false,
                    None,
                    false,
                    None,
                    false,
                    None,
                    false,
                    None,
                    Some(false),
                )
                .await?;
            Ok(AutoActionApplyOutcome {
                applied: updated.is_some(),
                target_type: "user",
                target_id: auth_context.user_id.clone(),
            })
        }
        "lock_api_key" => {
            let updated = state
                .set_user_api_key_locked(&auth_context.user_id, &auth_context.api_key_id, true)
                .await?;
            Ok(AutoActionApplyOutcome {
                applied: updated,
                target_type: "user_api_key",
                target_id: auth_context.api_key_id.clone(),
            })
        }
        _ => Ok(AutoActionApplyOutcome {
            applied: false,
            target_type: "unknown",
            target_id: action.to_string(),
        }),
    }
}

#[derive(Clone, Copy, Debug)]
struct TextRange {
    start: usize,
    end: usize,
}

pub(crate) fn validate_keyword_patterns(config: &RiskControlRuntimeConfig) -> Result<(), String> {
    if config.keyword_match_mode != RiskControlKeywordMatchMode::Regex {
        return Ok(());
    }
    let mut pattern_count = 0usize;
    for keyword in &config.keywords {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            continue;
        }
        if pattern_count >= MAX_REGEX_KEYWORD_ITEMS {
            return Err(format!(
                "regex 模式最多允许 {MAX_REGEX_KEYWORD_ITEMS} 条关键词"
            ));
        }
        pattern_count += 1;
        validate_keyword_regex_pattern(keyword)?;
    }
    Ok(())
}

fn keyword_matches(
    text: &str,
    keywords: &[String],
    exemptions: &[String],
    match_mode: RiskControlKeywordMatchMode,
) -> Vec<String> {
    keyword_match_outcome(text, keywords, exemptions, match_mode).matched_keywords
}

fn keyword_outcome_for_config(
    config: &RiskControlRuntimeConfig,
    text: &str,
) -> KeywordMatchOutcome {
    if config.keyword_mode == RiskControlKeywordMode::ApiOnly {
        return KeywordMatchOutcome::default();
    }
    keyword_match_outcome(
        text,
        &config.keywords,
        &config.keyword_exemptions,
        config.keyword_match_mode,
    )
}

fn keyword_decision_source(
    config: &RiskControlRuntimeConfig,
    outcome: &KeywordMatchOutcome,
) -> Option<&'static str> {
    if !matches!(
        config.keyword_mode,
        RiskControlKeywordMode::KeywordOnly | RiskControlKeywordMode::KeywordAndApi
    ) {
        return None;
    }
    if !outcome.matched_keywords.is_empty() {
        return Some("keyword");
    }
    if outcome.regex_scan_limited || outcome.regex_pattern_limited {
        return Some("regex_budget_limited");
    }
    if outcome.regex_invalid_pattern_count > 0 {
        return Some("regex_config_invalid");
    }
    None
}

fn emit_regex_keyword_runtime_warnings(
    request_context: &GatewayPublicRequestContext,
    outcome: &KeywordMatchOutcome,
) {
    if outcome.regex_scan_limited {
        warn!(
            event_name = "risk_control_regex_scan_budget_limited",
            log_type = "security",
            trace_id = %request_context.trace_id,
            regex_scan_chars = outcome.regex_scan_chars,
            regex_pattern_count = outcome.regex_pattern_count,
            regex_total_scan_budget_chars = outcome.regex_total_scan_budget_chars,
            "risk control regex keyword scan was limited by runtime budget"
        );
    }
    if outcome.regex_pattern_limited {
        warn!(
            event_name = "risk_control_regex_pattern_budget_limited",
            log_type = "security",
            trace_id = %request_context.trace_id,
            regex_pattern_count = outcome.regex_pattern_count,
            max_regex_pattern_count = MAX_REGEX_KEYWORD_ITEMS,
            "risk control regex keyword pattern count was limited at runtime"
        );
    }
    if outcome.regex_invalid_pattern_count > 0 {
        warn!(
            event_name = "risk_control_regex_runtime_config_invalid",
            log_type = "security",
            trace_id = %request_context.trace_id,
            regex_invalid_pattern_count = outcome.regex_invalid_pattern_count,
            "risk control regex keyword config contains invalid patterns at runtime"
        );
    }
}

fn keyword_match_outcome(
    text: &str,
    keywords: &[String],
    exemptions: &[String],
    match_mode: RiskControlKeywordMatchMode,
) -> KeywordMatchOutcome {
    if match_mode == RiskControlKeywordMatchMode::Regex {
        return regex_keyword_match_outcome(text, keywords, exemptions);
    }
    let haystack = text.to_ascii_lowercase();
    let exemption_ranges = term_ranges(&haystack, exemptions);
    let matched_keywords = keywords
        .iter()
        .filter_map(|keyword| {
            let keyword = keyword.trim();
            if keyword.is_empty() {
                return None;
            }
            let has_uncovered_match = ranges_for_keyword(text, &haystack, keyword, match_mode)
                .into_iter()
                .any(|range| !range_is_covered(range, &exemption_ranges));
            has_uncovered_match.then(|| keyword.to_string())
        })
        .collect();
    KeywordMatchOutcome {
        matched_keywords,
        ..KeywordMatchOutcome::default()
    }
}

fn regex_keyword_match_outcome(
    text: &str,
    keywords: &[String],
    exemptions: &[String],
) -> KeywordMatchOutcome {
    let regex_keywords = keywords
        .iter()
        .map(|keyword| keyword.trim())
        .filter(|keyword| !keyword.is_empty())
        .collect::<Vec<_>>();
    let regex_pattern_limited = regex_keywords.len() > MAX_REGEX_KEYWORD_ITEMS;
    let regex_keywords = regex_keywords
        .into_iter()
        .take(MAX_REGEX_KEYWORD_ITEMS)
        .collect::<Vec<_>>();
    let regex_pattern_count = regex_keywords.len();
    if regex_pattern_count == 0 {
        return KeywordMatchOutcome::default();
    }

    let regex_scan_chars = regex_scan_chars_for_pattern_count(regex_pattern_count);
    let (scan_text, text_limited) = prefix_by_char_budget(text, regex_scan_chars);
    let haystack = scan_text.to_ascii_lowercase();
    let exemption_ranges = term_ranges(&haystack, exemptions);
    let mut invalid_pattern_count = 0;
    let matched_keywords = regex_keywords
        .into_iter()
        .filter_map(|keyword| {
            let Some(regex) = compile_valid_keyword_regex(keyword) else {
                invalid_pattern_count += 1;
                return None;
            };
            let has_uncovered_match = regex_ranges_with_compiled(scan_text, &regex)
                .into_iter()
                .any(|range| !range_is_covered(range, &exemption_ranges));
            has_uncovered_match.then(|| keyword.to_string())
        })
        .collect();

    KeywordMatchOutcome {
        matched_keywords,
        regex_scan_limited: text_limited,
        regex_pattern_limited,
        regex_invalid_pattern_count: invalid_pattern_count,
        regex_scan_chars,
        regex_pattern_count,
        regex_total_scan_budget_chars: MAX_REGEX_TOTAL_SCAN_CHARS,
    }
}

fn regex_scan_chars_for_pattern_count(pattern_count: usize) -> usize {
    (MAX_REGEX_TOTAL_SCAN_CHARS / pattern_count.max(1)).clamp(1, MAX_REGEX_SCAN_CHARS)
}

fn prefix_by_char_budget(text: &str, max_chars: usize) -> (&str, bool) {
    if max_chars == 0 {
        return ("", !text.is_empty());
    }
    match text.char_indices().nth(max_chars) {
        Some((index, _)) => (&text[..index], true),
        None => (text, false),
    }
}

fn ranges_for_keyword(
    text: &str,
    haystack: &str,
    keyword: &str,
    match_mode: RiskControlKeywordMatchMode,
) -> Vec<TextRange> {
    match match_mode {
        RiskControlKeywordMatchMode::Contains => {
            ranges_for_needle(haystack, &keyword.to_ascii_lowercase())
        }
        RiskControlKeywordMatchMode::Exact => exact_text_range(text, keyword),
        RiskControlKeywordMatchMode::Regex => {
            debug_assert!(
                false,
                "regex keyword ranges must go through regex_keyword_match_outcome"
            );
            Vec::new()
        }
    }
}

fn exact_text_range(text: &str, keyword: &str) -> Vec<TextRange> {
    let keyword = keyword.trim();
    let trimmed = text.trim();
    if keyword.is_empty() || trimmed.is_empty() || !trimmed.eq_ignore_ascii_case(keyword) {
        return Vec::new();
    }
    let start = text.find(trimmed).unwrap_or(0);
    vec![TextRange {
        start,
        end: start + trimmed.len(),
    }]
}

fn regex_ranges(text: &str, pattern: &str) -> Vec<TextRange> {
    let Some(regex) = compile_valid_keyword_regex(pattern) else {
        return Vec::new();
    };
    regex_ranges_with_compiled(text, &regex)
}

fn regex_ranges_with_compiled(text: &str, regex: &Regex) -> Vec<TextRange> {
    regex
        .find_iter(text)
        .filter_map(|item| {
            let start = item.start();
            let end = item.end();
            (start < end).then_some(TextRange { start, end })
        })
        .take(MAX_REGEX_MATCH_RANGES)
        .collect()
}

fn compile_valid_keyword_regex(pattern: &str) -> Option<Regex> {
    let pattern = pattern.trim();
    if pattern.is_empty() || keyword_regex_static_limits(pattern).is_err() {
        return None;
    }
    let regex = compile_keyword_regex(pattern).ok()?;
    if regex.is_match("") {
        return None;
    }
    Some(regex)
}

fn validate_keyword_regex_pattern(pattern: &str) -> Result<(), String> {
    keyword_regex_static_limits(pattern)
        .map_err(|detail| format!("正则限制词「{pattern}」{detail}"))?;
    let regex = compile_keyword_regex(pattern)
        .map_err(|err| format!("正则限制词「{pattern}」无效: {err}"))?;
    if regex.is_match("") {
        return Err(format!("正则限制词「{pattern}」不能匹配空字符串"));
    }
    Ok(())
}

fn keyword_regex_static_limits(pattern: &str) -> Result<(), &'static str> {
    if pattern.chars().count() > MAX_REGEX_PATTERN_CHARS {
        return Err("长度超过限制");
    }
    if regex_complexity_score(pattern) > MAX_REGEX_COMPLEXITY_SCORE {
        return Err("复杂度超过限制");
    }
    Ok(())
}

fn regex_complexity_score(pattern: &str) -> usize {
    let mut score = 0usize;
    let mut escaped = false;
    let mut in_class = false;
    for ch in pattern.chars() {
        score = score.saturating_add(1);
        if escaped {
            score = score.saturating_add(1);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            score = score.saturating_add(1);
            escaped = true;
            continue;
        }
        if in_class {
            if ch == ']' {
                in_class = false;
            }
            score = score.saturating_add(1);
            continue;
        }
        match ch {
            '[' => {
                in_class = true;
                score = score.saturating_add(4);
            }
            '(' | ')' | '.' => score = score.saturating_add(3),
            '|' => score = score.saturating_add(6),
            '*' | '+' | '?' => score = score.saturating_add(10),
            '{' => score = score.saturating_add(12),
            _ => {}
        }
    }
    score
}

fn compile_keyword_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(MAX_REGEX_PROGRAM_SIZE_BYTES)
        .build()
}

fn term_ranges(haystack: &str, terms: &[String]) -> Vec<TextRange> {
    terms
        .iter()
        .flat_map(|term| ranges_for_needle(haystack, &term.trim().to_ascii_lowercase()))
        .collect()
}

fn ranges_for_needle(haystack: &str, needle: &str) -> Vec<TextRange> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut search_start = 0;
    while search_start < haystack.len() {
        let Some(index) = haystack[search_start..].find(needle) else {
            break;
        };
        let start = search_start + index;
        let end = start + needle.len();
        ranges.push(TextRange { start, end });
        search_start = haystack[start..]
            .chars()
            .next()
            .map(|ch| start + ch.len_utf8())
            .unwrap_or(haystack.len());
    }
    ranges
}

fn range_is_covered(range: TextRange, exemptions: &[TextRange]) -> bool {
    exemptions
        .iter()
        .any(|exemption| exemption.start <= range.start && exemption.end >= range.end)
}

fn should_call_moderation_api(config: &RiskControlRuntimeConfig) -> bool {
    !config.provider.api_keys.is_empty()
        && !matches!(config.keyword_mode, RiskControlKeywordMode::KeywordOnly)
}

fn action_for(config: &RiskControlRuntimeConfig, flagged: bool) -> &'static str {
    if flagged && config.mode == RiskControlMode::PreBlock {
        "block"
    } else if flagged {
        "observe"
    } else {
        "allow"
    }
}

fn block_decision(
    config: &RiskControlRuntimeConfig,
    decision: &InspectionDecision,
    input: &RiskControlInspectionInput,
) -> RiskControlBlockDecision {
    RiskControlBlockDecision {
        status_code: config.block_status,
        message: config.block_message.clone(),
        action: decision.action.to_string(),
        decision_source: decision.decision_source.to_string(),
        input_hash: input.input_hash.clone(),
    }
}

fn should_enforce_auto_action(mode: RiskControlMode, auto_action: Option<&str>) -> bool {
    mode == RiskControlMode::PreBlock && auto_action.is_some()
}

fn should_learn_flagged_hash(
    config: &RiskControlRuntimeConfig,
    decision: &InspectionDecision,
) -> bool {
    decision.flagged
        && config.hash_block.learn_from_flagged
        && decision.decision_source != "api_error"
}

fn sample_allows(input_hash: &str, sample_rate: f64) -> bool {
    if sample_rate >= 1.0 {
        return true;
    }
    if sample_rate <= 0.0 {
        return false;
    }
    let prefix = input_hash.get(..8).unwrap_or(input_hash);
    let bucket = u32::from_str_radix(prefix, 16).unwrap_or(0) as f64 / u32::MAX as f64;
    bucket <= sample_rate
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_ai_public_request_context() -> GatewayPublicRequestContext {
        GatewayPublicRequestContext {
            trace_id: "trace-risk-control-test".to_string(),
            request_method: http::Method::POST,
            request_path: "/v1/chat/completions".to_string(),
            request_query_string: None,
            request_content_type: Some("application/json".to_string()),
            host_header: None,
            control_decision: Some(GatewayControlDecision::synthetic(
                "/v1/chat/completions",
                Some("ai_public".to_string()),
                Some("openai".to_string()),
                Some("chat".to_string()),
                Some("openai:chat".to_string()),
            )),
        }
    }

    fn sample_auth_context(user_id: &str, api_key_id: &str) -> GatewayControlAuthContext {
        GatewayControlAuthContext {
            user_id: user_id.to_string(),
            api_key_id: api_key_id.to_string(),
            username: Some(format!("{user_id}-name")),
            api_key_name: Some(format!("{api_key_id}-name")),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
        }
    }

    fn risk_control_state(enabled: bool, config: serde_json::Value) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::disabled().with_system_config_values_for_tests([
                    (RISK_CONTROL_ENABLED_CONFIG_KEY.to_string(), json!(enabled)),
                    (RISK_CONTROL_CONFIG_KEY.to_string(), config),
                ]),
            )
    }

    #[tokio::test]
    async fn risk_control_disabled_does_not_request_body_buffering() {
        let state = risk_control_state(false, json!({ "mode": "pre_block" }));
        let context = sample_ai_public_request_context();

        assert!(!risk_control_should_buffer_body(&state, &context)
            .await
            .expect("buffer decision should succeed"));
    }

    #[tokio::test]
    async fn risk_control_off_mode_does_not_request_body_buffering() {
        let state = risk_control_state(true, json!({ "mode": "off" }));
        let context = sample_ai_public_request_context();

        assert!(!risk_control_should_buffer_body(&state, &context)
            .await
            .expect("buffer decision should succeed"));
    }

    #[tokio::test]
    async fn risk_control_observe_mode_requests_body_buffering() {
        let state = risk_control_state(true, json!({ "mode": "observe" }));
        let context = sample_ai_public_request_context();

        assert!(risk_control_should_buffer_body(&state, &context)
            .await
            .expect("buffer decision should succeed"));
    }

    #[tokio::test]
    async fn risk_control_scope_skips_unmatched_request() {
        let state = risk_control_state(
            true,
            json!({
                "mode": "pre_block",
                "keyword_mode": "keyword_only",
                "keywords": ["blocked"],
                "scope": {
                    "users": {
                        "mode": "include",
                        "values": ["user-allowed"]
                    }
                }
            }),
        );
        let mut context = sample_ai_public_request_context();
        context
            .control_decision
            .as_mut()
            .expect("decision should exist")
            .auth_context = Some(sample_auth_context("user-other", "key-1"));
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","messages":[{"role":"user","content":"blocked"}]}"#,
        );

        let decision =
            maybe_inspect_gateway_request(&state, &context, &http::HeaderMap::new(), &body)
                .await
                .expect("inspection should succeed");

        assert!(decision.is_none());
    }

    fn sample_inspection_input() -> RiskControlInspectionInput {
        RiskControlInspectionInput {
            endpoint: "openai:chat".to_string(),
            model: Some("gpt-5".to_string()),
            text: "sensitive prompt".to_string(),
            keyword_text: "sensitive prompt".to_string(),
            normalized_hash_input: "sensitive prompt".to_string(),
            input_hash: "hash".to_string(),
            excerpt: "sensitive prompt".to_string(),
        }
    }

    fn sample_observe_job() -> RiskControlObserveJob {
        RiskControlObserveJob {
            request_context: sample_ai_public_request_context(),
            config: RiskControlRuntimeConfig::default(),
            input: sample_inspection_input(),
            started_at: Instant::now(),
            enqueued_at: Instant::now(),
            request_id: Some("request-1".to_string()),
        }
    }

    #[test]
    fn observe_queue_rolls_back_queued_count_when_full() {
        let queue = RiskControlObserveQueue::new(1, 1);

        assert!(queue.enqueue(sample_observe_job()).is_ok());
        assert!(matches!(
            queue.enqueue(sample_observe_job()),
            Err(RiskControlObserveEnqueueError::Full)
        ));

        let snapshot = queue.snapshot();
        assert_eq!(snapshot.capacity, 1);
        assert_eq!(snapshot.configured_capacity, 1);
        assert_eq!(snapshot.queued, 1);
        assert_eq!(snapshot.enqueued_total, 1);
        assert_eq!(snapshot.dropped_total, 1);
    }

    #[test]
    fn observe_queue_soft_capacity_drops_before_channel_full() {
        let queue = RiskControlObserveQueue::new(16, 2);

        assert!(queue.enqueue(sample_observe_job()).is_ok());
        assert!(queue.enqueue(sample_observe_job()).is_ok());
        assert!(matches!(
            queue.enqueue(sample_observe_job()),
            Err(RiskControlObserveEnqueueError::Full)
        ));

        let snapshot = queue.snapshot();
        assert_eq!(snapshot.capacity, 16);
        assert_eq!(snapshot.configured_capacity, 2);
        assert_eq!(snapshot.queued, 2);
        assert_eq!(snapshot.dropped_total, 1);
    }

    #[test]
    fn observe_queue_set_soft_capacity_clamps_to_hard_limit() {
        let queue = RiskControlObserveQueue::new(8, 2);

        queue.set_soft_capacity(0);
        assert_eq!(queue.snapshot().configured_capacity, 1);

        queue.set_soft_capacity(999_999);
        assert_eq!(queue.snapshot().configured_capacity, 8);
    }

    #[test]
    fn notification_excerpt_is_hidden_by_default() {
        let config = RiskControlRuntimeConfig::default();
        let input = sample_inspection_input();

        assert_eq!(notification_excerpt_value(&config, &input), "(已隐藏)");
    }

    #[test]
    fn notification_excerpt_can_be_included_explicitly() {
        let config = RiskControlRuntimeConfig {
            notification: crate::risk_control::config::RiskControlNotificationConfig {
                include_excerpt: true,
                ..crate::risk_control::config::RiskControlNotificationConfig::default()
            },
            ..RiskControlRuntimeConfig::default()
        };
        let input = sample_inspection_input();

        assert_eq!(
            notification_excerpt_value(&config, &input),
            "sensitive prompt"
        );
    }

    #[test]
    fn request_id_from_headers_is_trimmed_and_bounded_to_storage_width() {
        let mut headers = http::HeaderMap::new();
        let long_request_id = format!("  {}  ", "x".repeat(MAX_REQUEST_ID_CHARS + 20));
        headers.insert(
            "x-request-id",
            http::HeaderValue::from_str(&long_request_id).expect("request id header should parse"),
        );

        let request_id =
            extract_request_id_from_headers(&headers).expect("request id should exist");

        assert_eq!(request_id.len(), MAX_REQUEST_ID_CHARS);
        assert!(request_id.chars().all(|ch| ch == 'x'));
    }

    #[test]
    fn keyword_matching_is_case_insensitive() {
        assert_eq!(
            keyword_matches(
                "Please Refund",
                &["refund".to_string()],
                &[],
                RiskControlKeywordMatchMode::Contains
            ),
            vec!["refund".to_string()]
        );
    }

    #[test]
    fn keyword_matching_exempts_covering_phrase() {
        assert_eq!(
            keyword_matches(
                "safe phrase",
                &["blocked".to_string()],
                &["safe phrase".to_string()],
                RiskControlKeywordMatchMode::Contains
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            keyword_matches(
                "allow this blocked phrase",
                &["blocked".to_string()],
                &["allow this blocked".to_string()],
                RiskControlKeywordMatchMode::Contains
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn keyword_matching_keeps_uncovered_occurrence() {
        assert_eq!(
            keyword_matches(
                "safe phrase then blocked",
                &["blocked".to_string()],
                &["safe phrase".to_string()],
                RiskControlKeywordMatchMode::Contains
            ),
            vec!["blocked".to_string()]
        );
    }

    #[test]
    fn keyword_matching_supports_exact_mode() {
        assert_eq!(
            keyword_matches(
                "safe phrase",
                &["blocked".to_string()],
                &[],
                RiskControlKeywordMatchMode::Exact
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            keyword_matches(
                "  Refund  ",
                &["refund".to_string()],
                &[],
                RiskControlKeywordMatchMode::Exact
            ),
            vec!["refund".to_string()]
        );
    }

    #[test]
    fn keyword_matching_supports_regex_mode() {
        assert_eq!(
            keyword_matches(
                "pattern item-test-123 appeared",
                &[r"item-[a-z]+-\d+".to_string()],
                &[],
                RiskControlKeywordMatchMode::Regex
            ),
            vec![r"item-[a-z]+-\d+".to_string()]
        );
        assert_eq!(
            keyword_matches(
                "pattern item-test-123 appeared",
                &[r"item-[a-z]+-\d+".to_string()],
                &["item-test-123".to_string()],
                RiskControlKeywordMatchMode::Regex
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn keyword_regex_runtime_budget_limits_scan_window() {
        let text = format!(
            "{}needle-after-budget",
            "a".repeat(MAX_REGEX_SCAN_CHARS + 8)
        );

        let outcome = keyword_match_outcome(
            &text,
            &["needle-after-budget".to_string()],
            &[],
            RiskControlKeywordMatchMode::Regex,
        );

        assert!(outcome.regex_scan_limited);
        assert_eq!(outcome.regex_scan_chars, MAX_REGEX_SCAN_CHARS);
        assert!(outcome.matched_keywords.is_empty());
    }

    #[test]
    fn keyword_regex_runtime_budget_scales_scan_window_by_pattern_count() {
        let pattern_count = MAX_REGEX_KEYWORD_ITEMS;
        let scan_chars = regex_scan_chars_for_pattern_count(pattern_count);
        assert!(scan_chars < MAX_REGEX_SCAN_CHARS);
        let mut keywords = (0..pattern_count - 1)
            .map(|index| format!("risk-{index}"))
            .collect::<Vec<_>>();
        keywords.push("needle-after-total-budget".to_string());
        let text = format!("{}needle-after-total-budget", "a".repeat(scan_chars + 8));

        let outcome =
            keyword_match_outcome(&text, &keywords, &[], RiskControlKeywordMatchMode::Regex);

        assert!(outcome.regex_scan_limited);
        assert_eq!(outcome.regex_pattern_count, pattern_count);
        assert_eq!(outcome.regex_scan_chars, scan_chars);
        assert!(outcome.matched_keywords.is_empty());
    }

    #[test]
    fn keyword_regex_runtime_budget_does_not_mark_limited_when_text_fits() {
        let pattern_count = MAX_REGEX_KEYWORD_ITEMS;
        let mut keywords = (0..pattern_count - 1)
            .map(|index| format!("risk-{index}"))
            .collect::<Vec<_>>();
        keywords.push(r"needle-\d+".to_string());

        let outcome = keyword_match_outcome(
            "short text has needle-123 inside",
            &keywords,
            &[],
            RiskControlKeywordMatchMode::Regex,
        );

        assert_eq!(outcome.regex_pattern_count, pattern_count);
        assert!(outcome.regex_scan_chars < MAX_REGEX_SCAN_CHARS);
        assert!(!outcome.regex_scan_limited);
        assert_eq!(outcome.matched_keywords, vec![r"needle-\d+".to_string()]);
    }

    #[test]
    fn keyword_regex_runtime_budget_caps_pattern_count_at_runtime() {
        let keywords = (0..MAX_REGEX_KEYWORD_ITEMS)
            .map(|index| format!("risk-{index}"))
            .chain(std::iter::once("needle-after-cap".to_string()))
            .collect::<Vec<_>>();

        let outcome = keyword_match_outcome(
            "needle-after-cap",
            &keywords,
            &[],
            RiskControlKeywordMatchMode::Regex,
        );

        assert_eq!(outcome.regex_pattern_count, MAX_REGEX_KEYWORD_ITEMS);
        assert!(outcome.regex_pattern_limited);
        assert!(outcome.matched_keywords.is_empty());
    }

    #[test]
    fn keyword_regex_runtime_budget_still_matches_inside_window() {
        let outcome = keyword_match_outcome(
            "prefix needle-123 suffix",
            &[r"needle-\d+".to_string()],
            &[],
            RiskControlKeywordMatchMode::Regex,
        );

        assert!(!outcome.regex_scan_limited);
        assert_eq!(outcome.matched_keywords, vec![r"needle-\d+".to_string()]);
    }

    #[test]
    fn keyword_regex_runtime_rejects_invalid_stale_patterns() {
        let outcome = keyword_match_outcome(
            "anything",
            &[".*".to_string()],
            &[],
            RiskControlKeywordMatchMode::Regex,
        );

        assert_eq!(outcome.regex_invalid_pattern_count, 1);
        assert!(outcome.matched_keywords.is_empty());
    }

    #[test]
    fn keyword_outcome_for_api_only_does_not_scan_regex_keywords() {
        let config = RiskControlRuntimeConfig {
            keyword_mode: RiskControlKeywordMode::ApiOnly,
            keyword_match_mode: RiskControlKeywordMatchMode::Regex,
            keywords: vec![r"needle-\d+".to_string()],
            ..RiskControlRuntimeConfig::default()
        };

        let outcome = keyword_outcome_for_config(&config, "needle-123");

        assert_eq!(outcome, KeywordMatchOutcome::default());
    }

    #[tokio::test]
    async fn test_risk_control_text_api_only_does_not_scan_regex_keywords() {
        let state = risk_control_state(false, json!({}));
        let config = RiskControlRuntimeConfig {
            keyword_mode: RiskControlKeywordMode::ApiOnly,
            keyword_match_mode: RiskControlKeywordMatchMode::Regex,
            keywords: vec![r"needle-\d+".to_string()],
            ..RiskControlRuntimeConfig::default()
        };

        let result = test_risk_control_text(&state, &config, "needle-123")
            .await
            .expect("test should succeed");

        assert_eq!(result["matched_keywords"], json!([]));
        assert_eq!(result["regex_scan_limited"], json!(false));
        assert_eq!(result["regex_scan_chars"], json!(0));
        assert_eq!(result["regex_pattern_count"], json!(0));
        assert_eq!(result["regex_total_scan_budget_chars"], json!(0));
    }

    #[tokio::test]
    async fn test_risk_control_text_reports_regex_budget_metadata() {
        let state = risk_control_state(false, json!({}));
        let config = RiskControlRuntimeConfig {
            keyword_mode: RiskControlKeywordMode::KeywordOnly,
            keyword_match_mode: RiskControlKeywordMatchMode::Regex,
            keywords: vec!["needle-after-budget".to_string()],
            ..RiskControlRuntimeConfig::default()
        };
        let text = format!(
            "{}needle-after-budget",
            "a".repeat(MAX_REGEX_SCAN_CHARS + 8)
        );

        let result = test_risk_control_text(&state, &config, &text)
            .await
            .expect("test should succeed");

        assert_eq!(result["regex_scan_limited"], json!(true));
        assert_eq!(result["regex_scan_chars"], json!(MAX_REGEX_SCAN_CHARS));
        assert_eq!(result["regex_pattern_count"], json!(1));
        assert_eq!(
            result["regex_total_scan_budget_chars"],
            json!(MAX_REGEX_TOTAL_SCAN_CHARS)
        );
        assert_eq!(result["decision_source"], json!("regex_budget_limited"));
    }

    #[test]
    fn keyword_regex_validation_reports_invalid_pattern() {
        let config = RiskControlRuntimeConfig {
            keyword_match_mode: RiskControlKeywordMatchMode::Regex,
            keywords: vec!["(".to_string()],
            ..RiskControlRuntimeConfig::default()
        };
        assert!(validate_keyword_patterns(&config).is_err());
    }

    #[test]
    fn keyword_regex_validation_rejects_empty_match_patterns() {
        let config = RiskControlRuntimeConfig {
            keyword_match_mode: RiskControlKeywordMatchMode::Regex,
            keywords: vec![".*".to_string()],
            ..RiskControlRuntimeConfig::default()
        };

        assert!(validate_keyword_patterns(&config).is_err());
    }

    #[test]
    fn keyword_regex_validation_rejects_too_many_patterns() {
        let config = RiskControlRuntimeConfig {
            keyword_match_mode: RiskControlKeywordMatchMode::Regex,
            keywords: (0..=MAX_REGEX_KEYWORD_ITEMS)
                .map(|index| format!("risk-{index}"))
                .collect(),
            ..RiskControlRuntimeConfig::default()
        };

        assert!(validate_keyword_patterns(&config).is_err());
    }

    #[test]
    fn keyword_regex_validation_rejects_complex_patterns() {
        let config = RiskControlRuntimeConfig {
            keyword_match_mode: RiskControlKeywordMatchMode::Regex,
            keywords: vec!["(ab|cd)+".repeat(80)],
            ..RiskControlRuntimeConfig::default()
        };

        assert!(validate_keyword_patterns(&config).is_err());
    }

    #[test]
    fn sampling_is_stable_for_hash() {
        assert!(sample_allows(
            "00000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            0.1
        ));
        assert!(!sample_allows(
            "ffffffff00000000ffffffffffffffffffffffffffffffffffffffffffffffff",
            0.1
        ));
    }

    #[test]
    fn auto_action_is_only_enforced_in_pre_block_mode() {
        assert!(should_enforce_auto_action(
            RiskControlMode::PreBlock,
            Some("disable_user")
        ));
        assert!(!should_enforce_auto_action(
            RiskControlMode::Observe,
            Some("disable_user")
        ));
        assert!(!should_enforce_auto_action(
            RiskControlMode::Off,
            Some("disable_user")
        ));
        assert!(!should_enforce_auto_action(RiskControlMode::PreBlock, None));
    }

    #[test]
    fn fail_closed_api_error_does_not_learn_flagged_hash() {
        let config = RiskControlRuntimeConfig {
            hash_block: crate::risk_control::config::RiskControlHashBlockConfig {
                enabled: true,
                learn_from_flagged: true,
            },
            ..RiskControlRuntimeConfig::default()
        };
        let decision = InspectionDecision {
            action: "block",
            decision_source: "api_error",
            flagged: true,
            highest_category: None,
            highest_score: 0.0,
            category_scores: None,
            matched_keywords: Vec::new(),
            error_message: Some("provider timeout".to_string()),
        };

        assert!(!should_learn_flagged_hash(&config, &decision));
    }

    #[test]
    fn keyword_decision_can_learn_flagged_hash_even_with_provider_error_detail() {
        let config = RiskControlRuntimeConfig {
            hash_block: crate::risk_control::config::RiskControlHashBlockConfig {
                enabled: true,
                learn_from_flagged: true,
            },
            ..RiskControlRuntimeConfig::default()
        };
        let decision = InspectionDecision {
            action: "block",
            decision_source: "keyword",
            flagged: true,
            highest_category: None,
            highest_score: 0.0,
            category_scores: None,
            matched_keywords: vec!["blocked".to_string()],
            error_message: Some("provider timeout".to_string()),
        };

        assert!(should_learn_flagged_hash(&config, &decision));
    }
}
