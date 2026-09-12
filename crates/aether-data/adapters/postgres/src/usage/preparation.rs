use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aether_data_contracts::DataLayerError;
use tokio::sync::Semaphore;

static USAGE_PREPARATION_EXECUTOR: OnceLock<UsagePreparationExecutor> = OnceLock::new();

pub(super) async fn prepare_usage_in_background<T: Send + 'static>(
    prepare: impl FnOnce() -> Result<T, DataLayerError> + Send + 'static,
) -> Result<T, DataLayerError> {
    USAGE_PREPARATION_EXECUTOR
        .get_or_init(|| {
            UsagePreparationExecutor::new(4, 32, Duration::from_secs(1), Duration::from_secs(30))
        })
        .run(prepare)
        .await
}

struct UsagePreparationExecutor {
    workers: Arc<Semaphore>,
    admitted: Arc<Semaphore>,
    queue_timeout: Duration,
    execution_timeout: Duration,
}

impl UsagePreparationExecutor {
    fn new(
        workers: usize,
        admitted: usize,
        queue_timeout: Duration,
        execution_timeout: Duration,
    ) -> Self {
        Self {
            workers: Arc::new(Semaphore::new(workers)),
            admitted: Arc::new(Semaphore::new(admitted)),
            queue_timeout,
            execution_timeout,
        }
    }

    async fn run<T: Send + 'static>(
        &self,
        prepare: impl FnOnce() -> Result<T, DataLayerError> + Send + 'static,
    ) -> Result<T, DataLayerError> {
        // Bound both running work and callers retaining input while waiting for a worker.
        // These limits count tasks, not bytes in the caller's original usage records.
        let admitted = self.admitted.clone().try_acquire_owned().map_err(|_| {
            DataLayerError::TimedOut("usage preparation capacity exhausted".to_string())
        })?;
        let worker = tokio::time::timeout(self.queue_timeout, self.workers.clone().acquire_owned())
            .await
            .map_err(|_| {
                DataLayerError::TimedOut(
                    "timed out waiting for usage preparation worker".to_string(),
                )
            })?
            .map_err(|_| {
                DataLayerError::TimedOut("usage preparation workers unavailable".to_string())
            })?;

        // The closure owns both permits even if its caller times out or is cancelled. It
        // prepares input only; detached completion must never begin a database transaction.
        let mut task = tokio::task::spawn_blocking(move || {
            let _admitted = admitted;
            let _worker = worker;
            prepare()
        });
        match tokio::time::timeout(self.execution_timeout, &mut task).await {
            Ok(result) => result.map_err(|error| {
                DataLayerError::TimedOut(format!("usage preparation worker failed: {error}"))
            })?,
            Err(_) => {
                // This cancels work still queued in Tokio; running blocking work keeps its
                // permits until it actually exits, since abort cannot stop a blocking thread.
                task.abort();
                Err(DataLayerError::TimedOut(
                    "timed out preparing usage storage".to_string(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    use super::*;

    fn executor(
        admitted: usize,
        queue_timeout: Duration,
        execution_timeout: Duration,
    ) -> Arc<UsagePreparationExecutor> {
        Arc::new(UsagePreparationExecutor::new(
            1,
            admitted,
            queue_timeout,
            execution_timeout,
        ))
    }

    async fn wait_for_worker_release(executor: &UsagePreparationExecutor) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while executor.workers.available_permits() != 1
                || executor.admitted.available_permits() == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("finished blocking work should release its permits");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn preparation_runs_off_the_runtime_thread_and_preserves_errors() {
        let executor = executor(1, Duration::from_secs(1), Duration::from_secs(2));
        let runtime_thread = std::thread::current().id();
        executor
            .run(move || {
                assert_ne!(std::thread::current().id(), runtime_thread);
                Ok(())
            })
            .await
            .expect("preparation should succeed");

        let error = executor
            .run(|| Err::<(), _>(DataLayerError::InvalidInput("bad usage".to_string())))
            .await
            .expect_err("input errors must reach the caller");
        assert!(matches!(error, DataLayerError::InvalidInput(message) if message == "bad usage"));
    }

    #[tokio::test]
    async fn saturated_admission_rejects_work_without_running_it() {
        let executor = executor(1, Duration::from_secs(1), Duration::from_secs(2));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_executor = executor.clone();
        let first = tokio::spawn(async move {
            first_executor
                .run(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    Ok(())
                })
                .await
        });
        started_rx.await.expect("first job should start");

        let error = executor
            .run(|| -> Result<(), DataLayerError> { panic!("rejected work must not execute") })
            .await
            .expect_err("admission should fail immediately");
        assert!(matches!(error, DataLayerError::TimedOut(message) if message.contains("capacity")));
        release_tx
            .send(())
            .expect("first job should still be alive");
        first.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn blocking_worker_failure_is_retryable_and_releases_capacity() {
        let executor = executor(1, Duration::from_secs(1), Duration::from_secs(2));
        let error = executor
            .run(|| -> Result<(), DataLayerError> { panic!("simulated worker failure") })
            .await
            .expect_err("worker failure must reach the caller");
        assert!(
            matches!(error, DataLayerError::TimedOut(message) if message.contains("worker failed"))
        );
        executor
            .run(|| Ok(()))
            .await
            .expect("failed workers should release capacity");
    }

    #[tokio::test]
    async fn waiting_for_a_worker_has_a_deadline_and_never_starts_expired_work() {
        let executor = executor(2, Duration::from_millis(20), Duration::from_secs(2));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_executor = executor.clone();
        let first = tokio::spawn(async move {
            first_executor
                .run(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    Ok(())
                })
                .await
        });
        started_rx.await.expect("first job should start");
        let ran = Arc::new(AtomicBool::new(false));
        let work_ran = ran.clone();
        let error = executor
            .run(move || {
                work_ran.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await
            .expect_err("the queued job should time out");
        assert!(matches!(error, DataLayerError::TimedOut(message) if message.contains("waiting")));
        assert_eq!(executor.admitted.available_permits(), 1);
        release_tx
            .send(())
            .expect("first job should still be alive");
        first.await.unwrap().unwrap();
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancellation_keeps_permits_until_running_blocking_work_exits() {
        let executor = executor(1, Duration::from_secs(1), Duration::from_secs(2));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_executor = executor.clone();
        let first = tokio::spawn(async move {
            first_executor
                .run(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    Ok(())
                })
                .await
        });
        started_rx.await.expect("first job should start");
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_eq!(executor.workers.available_permits(), 0);
        assert!(matches!(
            executor.run(|| Ok(())).await,
            Err(DataLayerError::TimedOut(_))
        ));
        release_tx
            .send(())
            .expect("blocking work should outlive cancellation");
        wait_for_worker_release(&executor).await;
        executor
            .run(|| Ok(()))
            .await
            .expect("the executor should recover");
    }

    #[tokio::test]
    async fn cancellation_keeps_capture_budget_until_blocking_input_is_dropped() {
        use aether_data_contracts::repository::usage::{
            usage_json_heap_estimate, UpsertUsageRecord, UsageCaptureMemoryBudget,
        };

        let mut usage: UpsertUsageRecord = serde_json::from_value(serde_json::json!({
            "request_id": "req-cancelled-preparation",
            "provider_name": "test",
            "model": "test",
            "status": "completed",
            "billing_status": "pending",
            "updated_at_unix_secs": 100,
            "request_body": {"content": "retained".repeat(1024)}
        }))
        .unwrap();
        let bytes = std::mem::size_of::<serde_json::Value>()
            + usage_json_heap_estimate(usage.request_body.as_ref().unwrap());
        let budget = Arc::new(UsageCaptureMemoryBudget::new(bytes));
        assert!(usage.capture_retention.reserve(Arc::clone(&budget), bytes));
        let executor = executor(1, Duration::from_secs(1), Duration::from_secs(2));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_executor = Arc::clone(&executor);
        let first = tokio::spawn(async move {
            first_executor
                .run(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    drop(usage);
                    Ok(())
                })
                .await
        });
        started_rx.await.unwrap();
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_eq!(budget.retained_bytes(), bytes);
        release_tx.send(()).unwrap();
        wait_for_worker_release(&executor).await;
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[tokio::test]
    async fn execution_timeout_keeps_permits_until_running_blocking_work_exits() {
        let executor = executor(1, Duration::from_secs(1), Duration::from_millis(20));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_executor = executor.clone();
        let first = tokio::spawn(async move {
            first_executor
                .run(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    Ok(())
                })
                .await
        });
        started_rx.await.expect("first job should start");
        let error = first
            .await
            .unwrap()
            .expect_err("running work should time out");
        assert!(
            matches!(error, DataLayerError::TimedOut(message) if message.contains("preparing"))
        );
        assert_eq!(executor.workers.available_permits(), 0);
        assert!(matches!(
            executor.run(|| Ok(())).await,
            Err(DataLayerError::TimedOut(_))
        ));
        release_tx
            .send(())
            .expect("blocking work should outlive timeout");
        wait_for_worker_release(&executor).await;
        executor
            .run(|| Ok(()))
            .await
            .expect("the executor should recover");
    }
}
