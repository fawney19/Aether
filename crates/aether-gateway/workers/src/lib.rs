use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use aether_task_runtime::TaskSupervisorMetrics;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SingletonWorkerConfig {
    pub lease_ttl: Duration,
    pub renew_interval: Duration,
    pub retry_interval: Duration,
}

impl Default for SingletonWorkerConfig {
    fn default() -> Self {
        Self {
            lease_ttl: Duration::from_secs(90),
            renew_interval: Duration::from_secs(30),
            retry_interval: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingletonWorkerContext {
    task_key: &'static str,
    owner: String,
    fencing_token: u64,
}

impl SingletonWorkerContext {
    pub const fn task_key(&self) -> &'static str {
        self.task_key
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub const fn fencing_token(&self) -> u64 {
        self.fencing_token
    }
}

struct SingletonLeaseGuard {
    runtime_state: Arc<RuntimeState>,
    lease: Option<RuntimeLockLease>,
}

impl Drop for SingletonLeaseGuard {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let runtime_state = self.runtime_state.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            let _ = runtime_state.lock_release(&lease).await;
        });
    }
}

pub fn spawn_singleton_worker<F, Fut>(
    runtime_state: Arc<RuntimeState>,
    metrics: TaskSupervisorMetrics,
    owner: String,
    task_key: &'static str,
    config: SingletonWorkerConfig,
    worker: F,
) -> JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    spawn_singleton_worker_with_context(
        runtime_state,
        metrics,
        owner,
        task_key,
        config,
        move |_| worker(),
    )
}

pub fn spawn_singleton_worker_with_context<F, Fut>(
    runtime_state: Arc<RuntimeState>,
    metrics: TaskSupervisorMetrics,
    owner: String,
    task_key: &'static str,
    config: SingletonWorkerConfig,
    worker: F,
) -> JoinHandle<()>
where
    F: Fn(SingletonWorkerContext) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let lock_key = format!("task_runtime:singleton:{task_key}");
        loop {
            let lease = loop {
                match runtime_state
                    .lock_try_acquire(&lock_key, &owner, config.lease_ttl)
                    .await
                {
                    Ok(Some(lease)) => break lease,
                    Ok(None) => {
                        metrics.record_singleton_lease_contention(task_key);
                        tokio::time::sleep(config.retry_interval).await;
                    }
                    Err(error) => {
                        metrics.record_singleton_lease_error(task_key);
                        warn!(
                            event_name = "singleton_task_lease_acquire_failed",
                            log_type = "ops",
                            task = task_key,
                            owner = %owner,
                            error = ?error,
                            "singleton worker could not acquire lease"
                        );
                        tokio::time::sleep(config.retry_interval).await;
                    }
                }
            };

            let worker_context = SingletonWorkerContext {
                task_key,
                owner: owner.clone(),
                fencing_token: lease.fencing_token,
            };
            let mut lease_guard = SingletonLeaseGuard {
                runtime_state: runtime_state.clone(),
                lease: Some(lease),
            };
            let restart_after_release = {
                let worker_future = worker(worker_context);
                tokio::pin!(worker_future);
                let mut renew_timer = tokio::time::interval(config.renew_interval);
                renew_timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
                renew_timer.tick().await;

                loop {
                    tokio::select! {
                        _ = &mut worker_future => break false,
                        _ = renew_timer.tick() => {
                            let Some(lease) = lease_guard.lease.as_ref() else {
                                break false;
                            };
                            match runtime_state.lock_renew(lease, config.lease_ttl).await {
                                Ok(true) => {}
                                Ok(false) => {
                                    metrics.record_singleton_lease_lost(task_key);
                                    warn!(
                                        event_name = "singleton_task_lease_lost",
                                        log_type = "ops",
                                        task = task_key,
                                        owner = %owner,
                                        "singleton worker lease is no longer owned"
                                    );
                                    break true;
                                }
                                Err(error) => {
                                    metrics.record_singleton_lease_lost(task_key);
                                    metrics.record_singleton_lease_error(task_key);
                                    warn!(
                                        event_name = "singleton_task_lease_renew_failed",
                                        log_type = "ops",
                                        task = task_key,
                                        owner = %owner,
                                        error = ?error,
                                        "singleton worker lease renewal failed"
                                    );
                                    break true;
                                }
                            }
                        }
                    }
                }
            };

            let release_result = if let Some(lease) = lease_guard.lease.as_ref() {
                runtime_state.lock_release(lease).await
            } else {
                return;
            };
            if let Err(error) = release_result {
                metrics.record_singleton_lease_error(task_key);
                warn!(
                    event_name = "singleton_task_lease_release_failed",
                    log_type = "ops",
                    task = task_key,
                    owner = %owner,
                    error = ?error,
                    "singleton worker lease release failed"
                );
            } else {
                lease_guard.lease.take();
            }

            if !restart_after_release {
                return;
            }
            tokio::time::sleep(config.retry_interval).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        spawn_singleton_worker, spawn_singleton_worker_with_context, SingletonWorkerConfig,
    };
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use aether_task_runtime::TaskSupervisorMetrics;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn allows_only_one_owner_for_shared_runtime() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let metrics = TaskSupervisorMetrics::default();
        let executions = Arc::new(AtomicUsize::new(0));
        let config = SingletonWorkerConfig {
            lease_ttl: Duration::from_millis(60),
            renew_interval: Duration::from_millis(15),
            retry_interval: Duration::from_millis(5),
        };

        let first = spawn_singleton_worker(
            runtime.clone(),
            metrics.clone(),
            "node-a".to_string(),
            "test.singleton",
            config,
            {
                let executions = executions.clone();
                move || {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }
            },
        );
        let second = spawn_singleton_worker(
            runtime,
            metrics.clone(),
            "node-b".to_string(),
            "test.singleton",
            config,
            {
                let executions = executions.clone();
                move || {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }
            },
        );

        tokio::time::sleep(Duration::from_millis(160)).await;
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert!(metrics.snapshot().singleton_lease_contention_total > 0);
        first.abort();
        second.abort();
        let _ = first.await;
        let _ = second.await;
    }

    #[tokio::test]
    async fn shutdown_releases_lease_before_ttl_expires() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let metrics = TaskSupervisorMetrics::default();
        let executions = Arc::new(AtomicUsize::new(0));
        let config = SingletonWorkerConfig {
            lease_ttl: Duration::from_secs(5),
            renew_interval: Duration::from_secs(1),
            retry_interval: Duration::from_millis(5),
        };

        let first = spawn_singleton_worker(
            runtime.clone(),
            metrics.clone(),
            "node-a".to_string(),
            "test.shutdown",
            config,
            {
                let executions = executions.clone();
                move || {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }
            },
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while executions.load(Ordering::SeqCst) < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first owner should start");
        first.abort();
        let _ = first.await;

        let second = spawn_singleton_worker(
            runtime,
            metrics,
            "node-b".to_string(),
            "test.shutdown",
            config,
            {
                let executions = executions.clone();
                move || {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }
            },
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while executions.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second owner should take over without waiting for the lease TTL");

        second.abort();
        let _ = second.await;
    }

    #[tokio::test]
    async fn passes_monotonic_fencing_token_to_worker_context() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let metrics = TaskSupervisorMetrics::default();
        let config = SingletonWorkerConfig {
            lease_ttl: Duration::from_secs(1),
            renew_interval: Duration::from_millis(100),
            retry_interval: Duration::from_millis(5),
        };

        let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_singleton_worker_with_context(
            runtime.clone(),
            metrics.clone(),
            "node-a".to_string(),
            "test.context",
            config,
            move |context| {
                let first_tx = first_tx.clone();
                async move {
                    first_tx.send(context).expect("send first context");
                }
            },
        )
        .await
        .expect("first worker should complete");
        let first = first_rx.recv().await.expect("first context");
        assert_eq!(first.task_key(), "test.context");
        assert_eq!(first.owner(), "node-a");

        let (second_tx, mut second_rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_singleton_worker_with_context(
            runtime,
            metrics,
            "node-b".to_string(),
            "test.context",
            config,
            move |context| {
                let second_tx = second_tx.clone();
                async move {
                    second_tx.send(context).expect("send second context");
                }
            },
        )
        .await
        .expect("second worker should complete");
        let second = second_rx.recv().await.expect("second context");
        assert!(second.fencing_token() > first.fencing_token());
    }

    #[tokio::test]
    async fn restarts_worker_after_lease_expires_before_renewal() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let metrics = TaskSupervisorMetrics::default();
        let config = SingletonWorkerConfig {
            lease_ttl: Duration::from_millis(20),
            renew_interval: Duration::from_millis(50),
            retry_interval: Duration::from_millis(5),
        };
        let (context_tx, mut context_rx) = tokio::sync::mpsc::unbounded_channel();

        let handle = spawn_singleton_worker_with_context(
            runtime,
            metrics.clone(),
            "node-a".to_string(),
            "test.restart.expired",
            config,
            move |context| {
                let context_tx = context_tx.clone();
                async move {
                    context_tx.send(context).expect("send worker context");
                    std::future::pending::<()>().await;
                }
            },
        );

        let first = tokio::time::timeout(Duration::from_secs(1), context_rx.recv())
            .await
            .expect("first worker should start")
            .expect("first worker context");
        let second = tokio::time::timeout(Duration::from_secs(1), context_rx.recv())
            .await
            .expect("worker should restart after losing its lease")
            .expect("restarted worker context");

        assert!(second.fencing_token() > first.fencing_token());
        assert!(metrics.snapshot().singleton_lease_lost_total >= 1);
        handle.abort();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn restarts_worker_after_redis_renew_error_when_backend_recovers() {
        let Ok(mut server) = aether_test_support::ManagedRedisServer::start().await else {
            return;
        };
        let redis_config = aether_runtime_state::RedisClientConfig {
            url: server.redis_url().to_string(),
            key_prefix: Some(format!(
                "aether-gateway-workers-restart-test-{}",
                std::process::id()
            )),
        };
        let Ok(runtime) = RuntimeState::redis(redis_config, Some(100)).await else {
            return;
        };
        let metrics = TaskSupervisorMetrics::default();
        let config = SingletonWorkerConfig {
            lease_ttl: Duration::from_millis(500),
            renew_interval: Duration::from_millis(25),
            retry_interval: Duration::from_millis(10),
        };
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = spawn_singleton_worker(
            Arc::new(runtime),
            metrics.clone(),
            "redis-node-a".to_string(),
            "test.restart.redis-error",
            config,
            move || {
                let started_tx = started_tx.clone();
                async move {
                    started_tx.send(()).expect("record worker start");
                    std::future::pending::<()>().await;
                }
            },
        );

        tokio::time::timeout(Duration::from_secs(1), started_rx.recv())
            .await
            .expect("first worker should start")
            .expect("first worker start notification");
        server.stop().expect("stop redis server");
        tokio::time::timeout(Duration::from_secs(2), async {
            while metrics.snapshot().singleton_lease_error_total == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("renewal should observe the redis outage");

        server.restart().await.expect("restart redis server");
        tokio::time::timeout(Duration::from_secs(3), started_rx.recv())
            .await
            .expect("worker should restart after redis recovers")
            .expect("restarted worker notification");
        assert!(metrics.snapshot().singleton_lease_lost_total >= 1);

        handle.abort();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn redis_backend_allows_only_one_owner_when_available() {
        let Ok(server) = aether_test_support::ManagedRedisServer::start().await else {
            return;
        };
        let config = aether_runtime_state::RedisClientConfig {
            url: server.redis_url().to_string(),
            key_prefix: Some(format!(
                "aether-gateway-workers-test-{}",
                std::process::id()
            )),
        };
        let Ok(runtime_a) = RuntimeState::redis(config.clone(), Some(500)).await else {
            return;
        };
        let Ok(runtime_b) = RuntimeState::redis(config, Some(500)).await else {
            return;
        };
        let runtime_a = Arc::new(runtime_a);
        let runtime_b = Arc::new(runtime_b);
        let metrics = TaskSupervisorMetrics::default();
        let executions = Arc::new(AtomicUsize::new(0));
        let config = SingletonWorkerConfig {
            lease_ttl: Duration::from_millis(120),
            renew_interval: Duration::from_millis(30),
            retry_interval: Duration::from_millis(5),
        };

        let first = spawn_singleton_worker(
            runtime_a,
            metrics.clone(),
            "redis-node-a".to_string(),
            "test.redis.singleton",
            config,
            {
                let executions = executions.clone();
                move || {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }
            },
        );
        let second = spawn_singleton_worker(
            runtime_b,
            metrics.clone(),
            "redis-node-b".to_string(),
            "test.redis.singleton",
            config,
            {
                let executions = executions.clone();
                move || {
                    let executions = executions.clone();
                    async move {
                        executions.fetch_add(1, Ordering::SeqCst);
                        std::future::pending::<()>().await;
                    }
                }
            },
        );

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        first.abort();
        second.abort();
        let _ = first.await;
        let _ = second.await;
    }
}
