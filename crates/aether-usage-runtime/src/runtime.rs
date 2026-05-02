use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use aether_contracts::ExecutionTelemetry;
use aether_data::redis::RedisStreamRunner;
use aether_data_contracts::repository::usage::UpsertUsageRecord;
use aether_data_contracts::DataLayerError;
use async_trait::async_trait;
use tracing::warn;

use crate::executor::spawn_on_usage_background_runtime;
use crate::{
    apply_usage_body_capture_policy_to_event, apply_usage_body_capture_policy_to_record,
    build_stream_terminal_usage_seed, build_sync_terminal_usage_seed,
    build_terminal_usage_event_from_seed, build_upsert_usage_record_from_event,
    build_usage_queue_worker, settle_usage_if_needed, LifecycleUsageSeed,
    StreamTerminalUsagePayloadSeed, SyncTerminalUsagePayloadSeed, TerminalUsageContextSeed,
    UsageEvent, UsageQueue, UsageRecordWriter, UsageRuntimeConfig, UsageSettlementWriter,
};

#[async_trait]
pub trait UsageBillingEventEnricher: Send + Sync {
    async fn enrich_usage_event(&self, event: &mut UsageEvent) -> Result<(), DataLayerError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageRequestRecordLevel {
    Basic,
    #[default]
    Full,
}

pub const DEFAULT_USAGE_REQUEST_BODY_CAPTURE_LIMIT_BYTES: usize = 5 * 1024 * 1024;
pub const DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageBodyCapturePolicy {
    pub record_level: UsageRequestRecordLevel,
    pub max_request_body_bytes: Option<usize>,
    pub max_response_body_bytes: Option<usize>,
}

impl Default for UsageBodyCapturePolicy {
    fn default() -> Self {
        Self {
            record_level: UsageRequestRecordLevel::Full,
            max_request_body_bytes: Some(DEFAULT_USAGE_REQUEST_BODY_CAPTURE_LIMIT_BYTES),
            max_response_body_bytes: Some(DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES),
        }
    }
}

#[async_trait]
pub trait UsageRuntimeAccess:
    UsageRecordWriter
    + UsageSettlementWriter
    + UsageBillingEventEnricher
    + crate::worker::ManualProxyNodeCounter
    + Send
    + Sync
{
    fn has_usage_writer(&self) -> bool;
    fn has_usage_worker_runner(&self) -> bool;
    fn usage_worker_runner(&self) -> Option<RedisStreamRunner>;

    async fn body_capture_policy(&self) -> Result<UsageBodyCapturePolicy, DataLayerError> {
        Ok(UsageBodyCapturePolicy::default())
    }

    async fn request_record_level(&self) -> Result<UsageRequestRecordLevel, DataLayerError> {
        Ok(self.body_capture_policy().await?.record_level)
    }
}

#[derive(Debug, Clone)]
pub struct UsageRuntime {
    config: UsageRuntimeConfig,
}

impl Default for UsageRuntime {
    fn default() -> Self {
        Self::disabled()
    }
}

impl UsageRuntime {
    pub fn disabled() -> Self {
        Self {
            config: UsageRuntimeConfig::disabled(),
        }
    }

    pub fn new(config: UsageRuntimeConfig) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn can_spawn_worker<T>(&self, data: &T) -> bool
    where
        T: UsageRuntimeAccess,
    {
        self.is_enabled() && data.has_usage_writer() && data.has_usage_worker_runner()
    }

    pub fn spawn_worker<T>(&self, data: Arc<T>) -> Option<tokio::task::JoinHandle<()>>
    where
        T: UsageRuntimeAccess + 'static,
    {
        if !self.can_spawn_worker(data.as_ref()) {
            return None;
        }
        let runner = data.usage_worker_runner()?;
        let worker = build_usage_queue_worker(runner, data, self.config.clone()).ok()?;
        Some(worker.spawn())
    }

    pub fn record_pending<T>(&self, data: &T, seed: LifecycleUsageSeed)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let data = T::clone(data);
        let request_id = seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            let now_unix_secs = now_unix_secs();
            match build_pending_usage_record_offthread(seed, now_unix_secs).await {
                Ok(mut record) => {
                    apply_body_capture_policy_to_record_from_data(&data, &mut record).await;
                    if let Err(err) = data.upsert_usage_record(record).await {
                        warn!(
                            event_name = "usage_pending_record_failed",
                            log_type = "event",
                            request_id = %request_id,
                            error = %err,
                            "usage runtime failed to record sync pending usage"
                        );
                    }
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_pending_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build sync pending usage"
                    )
                }
            }
        }));
    }

    pub fn record_stream_started<T>(
        &self,
        data: &T,
        seed: &LifecycleUsageSeed,
        status_code: u16,
        telemetry: Option<&ExecutionTelemetry>,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let data = T::clone(data);
        let seed = seed.clone();
        let telemetry = telemetry.cloned();
        let request_id = seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            let now_unix_secs = now_unix_secs();
            match build_streaming_usage_record_offthread(
                seed,
                status_code,
                telemetry,
                now_unix_secs,
            )
            .await
            {
                Ok(mut record) => {
                    apply_body_capture_policy_to_record_from_data(&data, &mut record).await;
                    if let Err(err) = data.upsert_usage_record(record).await {
                        warn!(
                            event_name = "usage_stream_record_failed",
                            log_type = "event",
                            request_id = %request_id,
                            error = %err,
                            "usage runtime failed to record stream usage"
                        );
                    }
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_stream_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build stream usage"
                    )
                }
            }
        }));
    }

    pub fn record_sync_terminal<T>(
        &self,
        data: &T,
        context_seed: TerminalUsageContextSeed,
        payload_seed: SyncTerminalUsagePayloadSeed,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let runtime = self.clone();
        let data = T::clone(data);
        let request_id = context_seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            match build_sync_terminal_usage_event_offthread(context_seed, payload_seed).await {
                Ok(mut event) => {
                    apply_body_capture_policy_from_data(&data, &mut event).await;
                    if let Err(err) = data.enrich_usage_event(&mut event).await {
                        warn!(
                            event_name = "usage_sync_terminal_billing_enrichment_failed",
                            log_type = "event",
                            request_id = %request_id,
                            error = %err,
                            "usage runtime failed to enrich sync usage event with billing"
                        );
                    }
                    runtime.enqueue_or_write_terminal(&data, event).await
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_sync_terminal_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build sync terminal usage event"
                    )
                }
            }
        }));
    }

    pub fn record_stream_terminal<T>(
        &self,
        data: &T,
        context_seed: TerminalUsageContextSeed,
        payload_seed: StreamTerminalUsagePayloadSeed,
        cancelled: bool,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let runtime = self.clone();
        let data = T::clone(data);
        let request_id = context_seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            match build_stream_terminal_usage_event_offthread(context_seed, payload_seed, cancelled)
                .await
            {
                Ok(mut event) => {
                    apply_body_capture_policy_from_data(&data, &mut event).await;
                    if let Err(err) = data.enrich_usage_event(&mut event).await {
                        warn!(
                            event_name = "usage_stream_terminal_billing_enrichment_failed",
                            log_type = "event",
                            request_id = %request_id,
                            error = %err,
                            "usage runtime failed to enrich stream usage event with billing"
                        );
                    }
                    runtime.enqueue_or_write_terminal(&data, event).await
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_stream_terminal_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build stream terminal usage event"
                    )
                }
            }
        }));
    }

    pub fn submit_terminal_event<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let runtime = self.clone();
        let data = T::clone(data);
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            runtime.record_terminal_event(&data, event).await;
        }));
    }

    pub async fn record_terminal_event<T>(&self, data: &T, mut event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        if !self.is_enabled() {
            return;
        }
        apply_body_capture_policy_from_data(data, &mut event).await;
        if let Err(err) = data.enrich_usage_event(&mut event).await {
            warn!(
                event_name = "usage_terminal_billing_enrichment_failed",
                log_type = "event",
                request_id = %event.request_id,
                error = %err,
                "usage runtime failed to enrich terminal usage event with billing"
            );
        }
        self.enqueue_or_write_terminal(data, event).await;
    }

    async fn enqueue_or_write_terminal<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        if let Some(runner) = data.usage_worker_runner() {
            match UsageQueue::new(runner, self.config.clone()) {
                Ok(queue) => match queue.enqueue(&event).await {
                    Ok(_) => return,
                    Err(err) => {
                        warn!(
                            event_name = "usage_terminal_enqueue_failed",
                            log_type = "event",
                            request_id = %event.request_id,
                            fallback = "direct_write",
                            error = %err,
                            "usage runtime failed to enqueue terminal usage event; falling back to direct write"
                        )
                    }
                },
                Err(err) => {
                    warn!(
                        event_name = "usage_terminal_queue_init_failed",
                        log_type = "event",
                        request_id = %event.request_id,
                        fallback = "direct_write",
                        error = %err,
                        "usage runtime failed to build queue; falling back to direct write"
                    )
                }
            }
        }

        match build_upsert_usage_record_from_event(&event) {
            Ok(record) => match data.upsert_usage_record(record).await {
                Ok(Some(stored)) => {
                    if let Err(err) = settle_usage_if_needed(data, &stored).await {
                        warn!(
                            event_name = "usage_terminal_settlement_failed",
                            log_type = "event",
                            request_id = %event.request_id,
                            error = %err,
                            "usage runtime failed to settle terminal usage directly"
                        );
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        event_name = "usage_terminal_upsert_failed",
                        log_type = "event",
                        request_id = %event.request_id,
                        error = %err,
                        "usage runtime failed to upsert terminal usage directly"
                    );
                }
            },
            Err(err) => {
                warn!(
                    event_name = "usage_terminal_upsert_build_failed",
                    log_type = "event",
                    request_id = %event.request_id,
                    error = %err,
                    "usage runtime failed to build terminal usage upsert"
                )
            }
        }
    }
}

async fn build_pending_usage_record_offthread(
    seed: LifecycleUsageSeed,
    now_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        crate::write::build_pending_usage_record_from_owned_seed(seed, now_unix_secs)
    })
    .await
    .map_err(join_error_to_data_layer)?
}

async fn build_streaming_usage_record_offthread(
    seed: LifecycleUsageSeed,
    status_code: u16,
    telemetry: Option<ExecutionTelemetry>,
    now_unix_secs: u64,
) -> Result<UpsertUsageRecord, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        crate::write::build_streaming_usage_record_from_owned_seed(
            seed,
            status_code,
            telemetry,
            now_unix_secs,
        )
    })
    .await
    .map_err(join_error_to_data_layer)?
}

async fn build_sync_terminal_usage_event_offthread(
    context_seed: TerminalUsageContextSeed,
    payload_seed: SyncTerminalUsagePayloadSeed,
) -> Result<UsageEvent, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        build_terminal_usage_event_from_seed(build_sync_terminal_usage_seed(
            context_seed,
            payload_seed,
        ))
    })
    .await
    .map_err(join_error_to_data_layer)?
}

async fn build_stream_terminal_usage_event_offthread(
    context_seed: TerminalUsageContextSeed,
    payload_seed: StreamTerminalUsagePayloadSeed,
    cancelled: bool,
) -> Result<UsageEvent, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        build_terminal_usage_event_from_seed(build_stream_terminal_usage_seed(
            context_seed,
            payload_seed,
            cancelled,
        ))
    })
    .await
    .map_err(join_error_to_data_layer)?
}

fn join_error_to_data_layer(err: tokio::task::JoinError) -> DataLayerError {
    DataLayerError::UnexpectedValue(format!("usage builder task join failed: {err}"))
}

async fn apply_body_capture_policy_from_data<T>(data: &T, event: &mut UsageEvent)
where
    T: UsageRuntimeAccess,
{
    match data.body_capture_policy().await {
        Ok(policy) => apply_usage_body_capture_policy_to_event(policy, event),
        Err(err) => {
            warn!(
                event_name = "usage_body_capture_policy_read_failed",
                log_type = "event",
                request_id = %event.request_id,
                fallback = "default",
                error = %err,
                "usage runtime failed to read body capture policy; keeping default capture"
            );
            apply_usage_body_capture_policy_to_event(UsageBodyCapturePolicy::default(), event);
        }
    }
}

async fn apply_body_capture_policy_to_record_from_data<T>(data: &T, record: &mut UpsertUsageRecord)
where
    T: UsageRuntimeAccess,
{
    match data.body_capture_policy().await {
        Ok(policy) => apply_usage_body_capture_policy_to_record(policy, record),
        Err(err) => {
            warn!(
                event_name = "usage_body_capture_policy_read_failed",
                log_type = "event",
                request_id = %record.request_id,
                fallback = "default",
                error = %err,
                "usage runtime failed to read body capture policy; keeping default capture"
            );
            apply_usage_body_capture_policy_to_record(UsageBodyCapturePolicy::default(), record);
        }
    }
}

fn boxed_usage_task<F>(task: F) -> Pin<Box<dyn Future<Output = ()> + Send>>
where
    F: Future<Output = ()> + Send + 'static,
{
    Box::pin(task)
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{UsageBodyCapturePolicy, UsageRequestRecordLevel};
    use crate::apply_usage_body_capture_policy_to_event;
    use crate::{UsageEvent, UsageEventData, UsageEventType};

    #[test]
    fn basic_request_record_level_strips_body_capture_but_preserves_derived_fields() {
        let mut event = UsageEvent::new(
            UsageEventType::Failed,
            "req-basic-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                total_tokens: Some(42),
                error_message: Some("upstream failed".to_string()),
                request_body: Some(json!({"messages":[{"role":"user","content":"hello"}]})),
                request_body_ref: Some("usage://request/req-basic-1/request_body".to_string()),
                provider_request_body: Some(json!({"model":"gpt-5"})),
                provider_request_body_ref: Some(
                    "usage://request/req-basic-1/provider_request_body".to_string(),
                ),
                response_body: Some(json!({"error":{"message":"bad gateway"}})),
                response_body_ref: Some("usage://request/req-basic-1/response_body".to_string()),
                client_response_body: Some(json!({"detail":"bad gateway"})),
                client_response_body_ref: Some(
                    "usage://request/req-basic-1/client_response_body".to_string(),
                ),
                ..UsageEventData::default()
            },
        );

        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Basic,
                ..UsageBodyCapturePolicy::default()
            },
            &mut event,
        );

        assert_eq!(event.data.total_tokens, Some(42));
        assert_eq!(event.data.error_message.as_deref(), Some("upstream failed"));
        assert!(event.data.request_body.is_none());
        assert!(event.data.request_body_ref.is_none());
        assert!(event.data.provider_request_body.is_none());
        assert!(event.data.provider_request_body_ref.is_none());
        assert!(event.data.response_body.is_none());
        assert!(event.data.response_body_ref.is_none());
        assert!(event.data.client_response_body.is_none());
        assert!(event.data.client_response_body_ref.is_none());
    }
}
