use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use aether_runtime::task::spawn_named;
pub use aether_task_core::{RetryPolicy, TaskDefinition, TaskKind};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;
use tracing::warn;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum TaskStatus {
    Queued,
    Running,
    Retrying,
    Succeeded,
    Failed,
    Cancelled,
    Skipped,
}

impl TaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskSupervisorTaskSnapshot {
    pub task_name: &'static str,
    pub active_tasks: u64,
    pub supervised_total: u64,
    pub completed_total: u64,
    pub panicked_total: u64,
    pub aborted_total: u64,
    pub cancelled_total: u64,
    pub singleton_lease_contention_total: u64,
    pub singleton_lease_lost_total: u64,
    pub singleton_lease_error_total: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskSupervisorMetricsSnapshot {
    pub active_tasks: u64,
    pub supervised_total: u64,
    pub completed_total: u64,
    pub panicked_total: u64,
    pub aborted_total: u64,
    pub cancelled_total: u64,
    pub singleton_lease_contention_total: u64,
    pub singleton_lease_lost_total: u64,
    pub singleton_lease_error_total: u64,
    pub tasks: Vec<TaskSupervisorTaskSnapshot>,
}

#[derive(Debug, Clone, Default)]
pub struct TaskSupervisorMetrics {
    inner: Arc<Mutex<BTreeMap<&'static str, TaskSupervisorTaskCounters>>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct TaskSupervisorTaskCounters {
    active_tasks: u64,
    supervised_total: u64,
    completed_total: u64,
    panicked_total: u64,
    aborted_total: u64,
    cancelled_total: u64,
    singleton_lease_contention_total: u64,
    singleton_lease_lost_total: u64,
    singleton_lease_error_total: u64,
}

impl TaskSupervisorMetrics {
    pub fn record_supervised(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.supervised_total = counters.supervised_total.saturating_add(1);
            counters.active_tasks = counters.active_tasks.saturating_add(1);
        });
    }

    pub fn record_completed(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.active_tasks = counters.active_tasks.saturating_sub(1);
            counters.completed_total = counters.completed_total.saturating_add(1);
        });
    }

    pub fn record_panicked(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.active_tasks = counters.active_tasks.saturating_sub(1);
            counters.panicked_total = counters.panicked_total.saturating_add(1);
        });
    }

    pub fn record_aborted(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.active_tasks = counters.active_tasks.saturating_sub(1);
            counters.aborted_total = counters.aborted_total.saturating_add(1);
        });
    }

    pub fn record_cancelled(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.active_tasks = counters.active_tasks.saturating_sub(1);
            counters.cancelled_total = counters.cancelled_total.saturating_add(1);
        });
    }

    pub fn record_singleton_lease_contention(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.singleton_lease_contention_total =
                counters.singleton_lease_contention_total.saturating_add(1);
        });
    }

    pub fn record_singleton_lease_lost(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.singleton_lease_lost_total =
                counters.singleton_lease_lost_total.saturating_add(1);
        });
    }

    pub fn record_singleton_lease_error(&self, task_name: &'static str) {
        self.with_task_counters(task_name, |counters| {
            counters.singleton_lease_error_total =
                counters.singleton_lease_error_total.saturating_add(1);
        });
    }

    pub fn snapshot(&self) -> TaskSupervisorMetricsSnapshot {
        let Ok(guard) = self.inner.lock() else {
            return TaskSupervisorMetricsSnapshot::default();
        };
        let mut snapshot = TaskSupervisorMetricsSnapshot::default();
        for (task_name, counters) in guard.iter() {
            snapshot.active_tasks = snapshot.active_tasks.saturating_add(counters.active_tasks);
            snapshot.supervised_total = snapshot
                .supervised_total
                .saturating_add(counters.supervised_total);
            snapshot.completed_total = snapshot
                .completed_total
                .saturating_add(counters.completed_total);
            snapshot.panicked_total = snapshot
                .panicked_total
                .saturating_add(counters.panicked_total);
            snapshot.aborted_total = snapshot
                .aborted_total
                .saturating_add(counters.aborted_total);
            snapshot.cancelled_total = snapshot
                .cancelled_total
                .saturating_add(counters.cancelled_total);
            snapshot.singleton_lease_contention_total = snapshot
                .singleton_lease_contention_total
                .saturating_add(counters.singleton_lease_contention_total);
            snapshot.singleton_lease_lost_total = snapshot
                .singleton_lease_lost_total
                .saturating_add(counters.singleton_lease_lost_total);
            snapshot.singleton_lease_error_total = snapshot
                .singleton_lease_error_total
                .saturating_add(counters.singleton_lease_error_total);
            snapshot.tasks.push(TaskSupervisorTaskSnapshot {
                task_name,
                active_tasks: counters.active_tasks,
                supervised_total: counters.supervised_total,
                completed_total: counters.completed_total,
                panicked_total: counters.panicked_total,
                aborted_total: counters.aborted_total,
                cancelled_total: counters.cancelled_total,
                singleton_lease_contention_total: counters.singleton_lease_contention_total,
                singleton_lease_lost_total: counters.singleton_lease_lost_total,
                singleton_lease_error_total: counters.singleton_lease_error_total,
            });
        }
        snapshot
    }

    fn with_task_counters(
        &self,
        task_name: &'static str,
        update: impl FnOnce(&mut TaskSupervisorTaskCounters),
    ) {
        if let Ok(mut guard) = self.inner.lock() {
            update(guard.entry(task_name).or_default());
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskContext<TPayload = serde_json::Value> {
    run_id: String,
    task_key: String,
    payload: Option<TPayload>,
    cancellation_token: CancellationToken,
}

impl<TPayload> TaskContext<TPayload> {
    pub fn new(
        run_id: impl Into<String>,
        task_key: impl Into<String>,
        payload: Option<TPayload>,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            task_key: task_key.into(),
            payload,
            cancellation_token,
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn task_key(&self) -> &str {
        &self.task_key
    }

    pub fn payload(&self) -> Option<&TPayload> {
        self.payload.as_ref()
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    pub async fn cancelled(&self) {
        self.cancellation_token.cancelled().await;
    }
}

#[derive(Debug)]
pub struct TaskSupervisor {
    cancellation_token: CancellationToken,
    join_set: JoinSet<()>,
    metrics: TaskSupervisorMetrics,
    supervised_task_count: usize,
}

impl TaskSupervisor {
    pub fn new() -> Self {
        Self::with_metrics(TaskSupervisorMetrics::default())
    }

    pub fn with_metrics(metrics: TaskSupervisorMetrics) -> Self {
        Self {
            cancellation_token: CancellationToken::new(),
            join_set: JoinSet::new(),
            metrics,
            supervised_task_count: 0,
        }
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    pub fn metrics(&self) -> TaskSupervisorMetrics {
        self.metrics.clone()
    }

    pub fn spawn_named<F>(&mut self, task_name: &'static str, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.supervised_task_count = self.supervised_task_count.saturating_add(1);
        let cancellation_token = self.cancellation_token.clone();
        let metrics = self.metrics.clone();
        metrics.record_supervised(task_name);
        self.join_set.spawn(async move {
            let mut handle = spawn_named(task_name, future);
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    handle.abort();
                    let _ = handle.await;
                    metrics.record_cancelled(task_name);
                }
                result = &mut handle => {
                    match result {
                        Ok(()) => metrics.record_completed(task_name),
                        Err(error) => {
                            if error.is_panic() {
                                metrics.record_panicked(task_name);
                            } else {
                                metrics.record_aborted(task_name);
                            }
                            warn!(task = task_name, error = ?error, "supervised task failed");
                        }
                    }
                }
            }
        });
    }

    pub fn supervise_handle(&mut self, task_name: &'static str, mut handle: JoinHandle<()>) {
        self.supervised_task_count = self.supervised_task_count.saturating_add(1);
        let cancellation_token = self.cancellation_token.clone();
        let metrics = self.metrics.clone();
        metrics.record_supervised(task_name);
        self.join_set.spawn(async move {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    handle.abort();
                    let _ = handle.await;
                    metrics.record_cancelled(task_name);
                }
                result = &mut handle => {
                    match result {
                        Ok(()) => metrics.record_completed(task_name),
                        Err(error) => {
                            if error.is_panic() {
                                metrics.record_panicked(task_name);
                            } else {
                                metrics.record_aborted(task_name);
                            }
                            warn!(task = task_name, error = ?error, "supervised task failed");
                        }
                    }
                }
            }
        });
    }

    pub fn is_empty(&self) -> bool {
        self.supervised_task_count == 0
    }

    pub fn task_count(&self) -> usize {
        self.supervised_task_count
    }

    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    pub async fn shutdown(mut self) {
        self.cancel();
        while self.join_set.join_next().await.is_some() {}
    }
}

impl Default for TaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskSupervisor, TaskSupervisorMetrics};
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn supervisor_metrics_record_completion_and_cancellation() {
        let metrics = TaskSupervisorMetrics::default();
        let (tx, rx) = oneshot::channel::<()>();
        let mut supervisor = TaskSupervisor::with_metrics(metrics.clone());

        supervisor.spawn_named("test.completed", async {});
        supervisor.spawn_named("test.cancelled", async move {
            let _ = rx.await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.supervised_total, 2);
        assert_eq!(snapshot.completed_total, 1);
        assert_eq!(snapshot.active_tasks, 1);

        supervisor.shutdown().await;
        drop(tx);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.supervised_total, 2);
        assert_eq!(snapshot.completed_total, 1);
        assert_eq!(snapshot.cancelled_total, 1);
        assert_eq!(snapshot.active_tasks, 0);
    }

    #[tokio::test]
    async fn supervisor_metrics_record_panics_and_external_aborts() {
        let metrics = TaskSupervisorMetrics::default();
        let mut supervisor = TaskSupervisor::with_metrics(metrics.clone());
        let handle = tokio::spawn(async {});
        handle.abort();

        supervisor.supervise_handle("test.aborted", handle);
        supervisor.spawn_named("test.panicked", async {
            panic!("intentional task panic");
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.supervised_total, 2);
        assert_eq!(snapshot.aborted_total, 1);
        assert_eq!(snapshot.panicked_total, 1);
        assert_eq!(snapshot.active_tasks, 0);

        supervisor.shutdown().await;
    }
}
