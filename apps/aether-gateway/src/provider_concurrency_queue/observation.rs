use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use aether_usage_runtime::{build_usage_event_data_seed, UsageEvent, UsageEventType};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::{AppState, GatewayError};

static OWNER: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());
static WAITERS: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(Mutex::default);
const OBSERVATION_IO_TIMEOUT: Duration = Duration::from_millis(250);

tokio::task_local! {
    static CANCELLATION: Arc<QueueCancellation>;
}

struct QueueCancellation {
    state: AppState,
    plan: ExecutionPlan,
    context: Option<Value>,
    started: std::time::Instant,
    armed: AtomicBool,
}

pub(crate) struct QueueCancellationGuard(Arc<QueueCancellation>);

impl QueueCancellationGuard {
    pub(crate) fn new(state: &AppState, plan: &ExecutionPlan, context: Option<&Value>) -> Self {
        Self(Arc::new(QueueCancellation {
            state: state.clone(),
            plan: plan.clone(),
            context: context.cloned(),
            started: std::time::Instant::now(),
            armed: AtomicBool::new(false),
        }))
    }

    pub(crate) async fn scope<F: std::future::Future>(&self, future: F) -> F::Output {
        CANCELLATION.scope(Arc::clone(&self.0), future).await
    }

    pub(crate) fn disarm(&self) {
        self.0.armed.store(false, Ordering::Relaxed);
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.0.armed.load(Ordering::Relaxed)
    }

    pub(crate) async fn fail(&self, error: &GatewayError) {
        if !self.0.armed.swap(false, Ordering::Relaxed) {
            return;
        }
        let context = crate::usage::reporting::failure::gateway_error_analytics_context(
            self.0.context.as_ref(),
            error,
        );
        let mut data = build_usage_event_data_seed(&self.0.plan, context.as_ref());
        data.status_code = Some(500);
        data.error_category = Some("server_error".to_string());
        data.error_message =
            Some("Provider admission failed before request completion.".to_string());
        data.response_time_ms =
            Some(u64::try_from(self.0.started.elapsed().as_millis()).unwrap_or(u64::MAX));
        self.0
            .state
            .usage_runtime
            .record_terminal_event_direct(
                self.0.state.usage_lifecycle_data_state().as_ref(),
                UsageEvent::new(UsageEventType::Failed, self.0.plan.request_id.clone(), data),
            )
            .await;
    }
}

impl Drop for QueueCancellationGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl QueueCancellation {
    fn cancel(self: &Arc<Self>) {
        if !self.armed.swap(false, Ordering::Relaxed) {
            return;
        }
        let cancellation = Arc::clone(self);
        let producer = self.state.usage_runtime.track_producer();
        tokio::spawn(async move {
            let _producer = producer;
            let context = crate::usage::reporting::failure::with_analytics_failure(
                cancellation.context.as_ref(),
                "gateway",
                "admission",
                "request_task_cancelled",
            );
            let mut data = build_usage_event_data_seed(&cancellation.plan, context.as_ref());
            data.status_code = Some(499);
            data.error_category = Some("cancelled".to_string());
            data.error_message = Some("Request cancelled during provider admission.".to_string());
            data.response_time_ms =
                Some(u64::try_from(cancellation.started.elapsed().as_millis()).unwrap_or(u64::MAX));
            cancellation
                .state
                .usage_runtime
                .record_terminal_event_direct(
                    cancellation.state.usage_lifecycle_data_state().as_ref(),
                    UsageEvent::new(
                        UsageEventType::Cancelled,
                        cancellation.plan.request_id.clone(),
                        data,
                    ),
                )
                .await;
        });
    }
}

fn request_key(request_id: &str) -> String {
    format!("provider-queue:request:{request_id}")
}

fn attempt_key(token: &str) -> String {
    format!("provider-queue:attempt:{token}")
}

pub(crate) struct QueueObservation {
    token: String,
    cancellation: Option<Arc<QueueCancellation>>,
    scoped_cancellation: bool,
    stop: Option<oneshot::Sender<()>>,
    finished: Option<oneshot::Receiver<()>>,
}

impl QueueObservation {
    pub(crate) fn begin(
        state: &AppState,
        plan: &ExecutionPlan,
        context: Option<&Value>,
        timeout: Duration,
    ) -> Self {
        let token = format!("{}:{}", *OWNER, uuid::Uuid::new_v4());
        WAITERS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(token.clone());
        let (stop, stopped) = oneshot::channel();
        let (finished, finish) = oneshot::channel();
        let (cancellation, scoped_cancellation) = match CANCELLATION.try_with(Arc::clone) {
            Ok(cancellation) => (cancellation, true),
            Err(_) => {
                let guard = QueueCancellationGuard::new(state, plan, context);
                (Arc::clone(&guard.0), false)
            }
        };
        cancellation.armed.store(true, Ordering::Relaxed);
        let observation = Self {
            token: token.clone(),
            cancellation: Some(cancellation),
            scoped_cancellation,
            stop: Some(stop),
            finished: Some(finish),
        };
        state.usage_runtime.record_pending(
            state.usage_lifecycle_data_state().as_ref(),
            aether_usage_runtime::build_lifecycle_usage_seed(plan, context),
        );
        let runtime = state.runtime_state.clone();
        let key = request_key(&plan.request_id);
        tokio::spawn(async move {
            let attempt = attempt_key(&token);
            let ttl = Some(timeout.saturating_add(Duration::from_secs(5)));
            let publish = tokio::time::timeout(OBSERVATION_IO_TIMEOUT, async {
                runtime.kv_set(&attempt, "1", ttl).await?;
                runtime.kv_set(&key, &token, ttl).await
            })
            .await;
            match publish {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, "could not publish provider queue observation")
                }
                Err(error) => {
                    tracing::warn!(%error, "provider queue observation publication timed out")
                }
            }
            if tokio::time::timeout(timeout.saturating_add(Duration::from_secs(1)), stopped)
                .await
                .is_err()
            {
                tracing::debug!("provider queue observation expired");
            }
            match tokio::time::timeout(OBSERVATION_IO_TIMEOUT, runtime.kv_delete(&attempt)).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, "could not clear provider queue observation")
                }
                Err(error) => {
                    tracing::warn!(%error, "provider queue observation cleanup timed out")
                }
            }
            if finished.send(()).is_err() {
                tracing::debug!("provider queue observation owner dropped");
            }
        });
        observation
    }

    pub(crate) async fn finish(mut self) {
        WAITERS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.token);
        self.stop.take();
        // A rejected candidate may still fail over. The request executor owns
        // normal terminal reporting; only a dropped queue wait is cancellation.
        if let Some(finished) = self.finished.take() {
            if finished.await.is_err() {
                tracing::warn!("provider queue observation cleanup stopped");
            }
        }
        if let Some(cancellation) = self.cancellation.take() {
            if !self.scoped_cancellation {
                cancellation.armed.store(false, Ordering::Relaxed);
            }
        }
    }
}

impl Drop for QueueObservation {
    fn drop(&mut self) {
        WAITERS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.token);
        self.stop.take();
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
    }
}

pub(crate) async fn queued_request_ids(
    state: &AppState,
    items: &[StoredRequestUsageAudit],
) -> BTreeSet<String> {
    match tokio::time::timeout(
        OBSERVATION_IO_TIMEOUT,
        lookup_queued_request_ids(state, items),
    )
    .await
    {
        Ok(Ok(queued)) => queued,
        Ok(Err(error)) => {
            tracing::warn!(?error, "could not read provider queue observations");
            BTreeSet::new()
        }
        Err(error) => {
            tracing::warn!(%error, "provider queue observation lookup timed out");
            BTreeSet::new()
        }
    }
}

async fn lookup_queued_request_ids(
    state: &AppState,
    items: &[StoredRequestUsageAudit],
) -> Result<BTreeSet<String>, GatewayError> {
    let requests = items
        .iter()
        .filter(|item| matches!(item.status.as_str(), "pending" | "streaming"))
        .collect::<Vec<_>>();
    if requests.is_empty() {
        return Ok(BTreeSet::new());
    }
    let keys = requests
        .iter()
        .map(|item| request_key(&item.request_id))
        .collect::<Vec<_>>();
    let tokens = state
        .runtime_state
        .kv_get_many(&keys)
        .await
        .map_err(|error| GatewayError::Internal(error.to_string()))?;
    let attempts = tokens
        .iter()
        .flatten()
        .map(|token| attempt_key(token))
        .collect::<Vec<_>>();
    if attempts.is_empty() {
        return Ok(BTreeSet::new());
    }
    let active = state
        .runtime_state
        .kv_get_many(&attempts)
        .await
        .map_err(|error| GatewayError::Internal(error.to_string()))?;
    let mut active = active.into_iter();
    let local = WAITERS.lock().unwrap_or_else(|error| error.into_inner());
    Ok(requests
        .into_iter()
        .zip(tokens)
        .filter_map(|(item, token)| {
            let token = token?;
            let shared = active.next().flatten().is_some();
            let queued = if token.starts_with(OWNER.as_str()) {
                local.contains(&token)
            } else {
                shared
            };
            queued.then(|| item.request_id.clone())
        })
        .collect())
}
