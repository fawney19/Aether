use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use tokio::time::MissedTickBehavior;

use crate::AppState;

const ADMIN_SYSTEM_IMPORT_LOCK_KEY: &str = "admin:system:import";
const ADMIN_SYSTEM_IMPORT_LOCK_TTL: Duration = Duration::from_secs(60 * 60 * 6);
const ADMIN_SYSTEM_IMPORT_LOCK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60 * 5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminSystemImportLockError {
    Conflict,
    Unavailable,
    Lost,
}

#[derive(Debug, PartialEq, Eq)]
enum AdminSystemImportRaceOutcome<T> {
    OperationCompleted(T),
    LeaseLost,
}

#[derive(Debug, PartialEq, Eq)]
enum AdminSystemImportLockRenewalFailure<E> {
    Lost,
    Backend(E),
}

struct AdminSystemImportLeaseGuard {
    runtime_state: Arc<RuntimeState>,
    lease: Option<RuntimeLockLease>,
}

impl AdminSystemImportLeaseGuard {
    fn new(app: &AppState, lease: RuntimeLockLease) -> Self {
        Self {
            runtime_state: app.runtime_state.clone(),
            lease: Some(lease),
        }
    }

    fn lease(&self) -> &RuntimeLockLease {
        self.lease
            .as_ref()
            .expect("admin system import lease guard must own a lease")
    }

    async fn release(&mut self) -> Result<(), AdminSystemImportLockError> {
        let Some(lease) = self.lease.clone() else {
            return Ok(());
        };
        match self.runtime_state.lock_release(&lease).await {
            Ok(true) => {
                self.lease.take();
                Ok(())
            }
            Ok(false) => {
                tracing::warn!(
                    lock_key = %lease.key,
                    "admin system import lock was no longer owned during release"
                );
                self.lease.take();
                Err(AdminSystemImportLockError::Lost)
            }
            Err(error) => {
                tracing::warn!(error = %error, "admin system import lock release failed");
                // Keep the lease in the guard so Drop can make one best-effort retry.
                Err(AdminSystemImportLockError::Lost)
            }
        }
    }
}

impl Drop for AdminSystemImportLeaseGuard {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let runtime_state = self.runtime_state.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        drop(handle.spawn(async move {
            match runtime_state.lock_release(&lease).await {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    lock_key = %lease.key,
                    "admin system import lock was no longer owned during asynchronous release"
                ),
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        lock_key = %lease.key,
                        "admin system import lock asynchronous release failed"
                    );
                }
            }
        }));
    }
}

pub(crate) async fn try_acquire_admin_system_import_lease(
    app: &AppState,
) -> Result<RuntimeLockLease, AdminSystemImportLockError> {
    match app
        .runtime_state()
        .lock_try_acquire(
            ADMIN_SYSTEM_IMPORT_LOCK_KEY,
            app.tunnel.local_instance_id(),
            ADMIN_SYSTEM_IMPORT_LOCK_TTL,
        )
        .await
    {
        Ok(Some(lock)) => Ok(lock),
        Ok(None) => Err(AdminSystemImportLockError::Conflict),
        Err(error) => {
            tracing::warn!(error = %error, "admin system import lock acquisition failed");
            Err(AdminSystemImportLockError::Unavailable)
        }
    }
}

pub(crate) async fn release_admin_system_import_lease(app: &AppState, lock: &RuntimeLockLease) {
    match app.runtime_state().lock_release(lock).await {
        Ok(true) => {}
        Ok(false) => tracing::warn!(
            lock_key = %lock.key,
            "admin system import lock was no longer owned during release"
        ),
        Err(error) => {
            tracing::warn!(error = %error, "admin system import lock release failed");
        }
    }
}

fn require_successful_admin_system_import_lock_renewal<E>(
    result: Result<bool, E>,
) -> Result<(), AdminSystemImportLockRenewalFailure<E>> {
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(AdminSystemImportLockRenewalFailure::Lost),
        Err(error) => Err(AdminSystemImportLockRenewalFailure::Backend(error)),
    }
}

async fn wait_for_admin_system_import_lease_loss(
    runtime_state: Arc<RuntimeState>,
    lease: RuntimeLockLease,
) {
    let mut heartbeat = tokio::time::interval(ADMIN_SYSTEM_IMPORT_LOCK_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        heartbeat.tick().await;
        match require_successful_admin_system_import_lock_renewal(
            runtime_state
                .lock_renew(&lease, ADMIN_SYSTEM_IMPORT_LOCK_TTL)
                .await,
        ) {
            Ok(()) => {}
            Err(AdminSystemImportLockRenewalFailure::Lost) => {
                tracing::warn!(
                    lock_key = %lease.key,
                    fencing_token = lease.fencing_token,
                    "admin system import lock is no longer owned; cancelling the import"
                );
                return;
            }
            Err(AdminSystemImportLockRenewalFailure::Backend(error)) => {
                tracing::warn!(
                    error = %error,
                    lock_key = %lease.key,
                    fencing_token = lease.fencing_token,
                    "admin system import lock renewal failed; cancelling the import"
                );
                return;
            }
        }
    }
}

async fn race_admin_system_import_with_lease_loss<F, L, T>(
    operation: F,
    lease_loss: L,
) -> AdminSystemImportRaceOutcome<T>
where
    F: Future<Output = T>,
    L: Future<Output = ()>,
{
    tokio::pin!(operation);
    tokio::pin!(lease_loss);
    tokio::select! {
        biased;
        _ = &mut lease_loss => AdminSystemImportRaceOutcome::LeaseLost,
        result = &mut operation => AdminSystemImportRaceOutcome::OperationCompleted(result),
    }
}

pub(crate) async fn execute_admin_system_import_exclusively<F, T>(
    app: &AppState,
    operation: F,
) -> Result<T, AdminSystemImportLockError>
where
    F: Future<Output = T>,
{
    let lease = try_acquire_admin_system_import_lease(app).await?;
    let mut guard = AdminSystemImportLeaseGuard::new(app, lease);
    let lease_loss =
        wait_for_admin_system_import_lease_loss(guard.runtime_state.clone(), guard.lease().clone());

    match race_admin_system_import_with_lease_loss(operation, lease_loss).await {
        AdminSystemImportRaceOutcome::OperationCompleted(result) => {
            guard.release().await?;
            Ok(result)
        }
        AdminSystemImportRaceOutcome::LeaseLost => Err(AdminSystemImportLockError::Lost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct DropSignal(Arc<AtomicBool>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn admin_system_import_lock_renewal_requires_current_ownership() {
        assert_eq!(
            require_successful_admin_system_import_lock_renewal::<Infallible>(Ok(true)),
            Ok(())
        );
        assert_eq!(
            require_successful_admin_system_import_lock_renewal::<Infallible>(Ok(false)),
            Err(AdminSystemImportLockRenewalFailure::Lost)
        );
        assert_eq!(
            require_successful_admin_system_import_lock_renewal(Err("redis unavailable")),
            Err(AdminSystemImportLockRenewalFailure::Backend(
                "redis unavailable"
            ))
        );
    }

    #[tokio::test]
    async fn admin_system_import_operation_is_cancelled_when_lease_is_lost() {
        let dropped = Arc::new(AtomicBool::new(false));
        let operation_dropped = dropped.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let operation = async move {
            let _drop_signal = DropSignal(operation_dropped);
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        };
        let lease_loss = async move {
            started_rx
                .await
                .expect("operation should be polled before reporting lease loss");
        };

        let outcome = race_admin_system_import_with_lease_loss(operation, lease_loss).await;

        assert_eq!(outcome, AdminSystemImportRaceOutcome::LeaseLost);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn completed_admin_system_import_releases_lease_for_reuse() {
        let app = AppState::new().expect("app state should build");

        let result = execute_admin_system_import_exclusively(&app, async { 42_u8 })
            .await
            .expect("import should complete");
        assert_eq!(result, 42);

        let lease = try_acquire_admin_system_import_lease(&app)
            .await
            .expect("completed import should release its lease");
        release_admin_system_import_lease(&app, &lease).await;
    }

    #[tokio::test]
    async fn cancelled_admin_system_import_releases_lease_without_waiting_for_ttl() {
        let app = AppState::new().expect("app state should build");
        let task_app = app.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            execute_admin_system_import_exclusively(&task_app, async move {
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
            })
            .await
        });
        started_rx
            .await
            .expect("operation should start after acquiring the lease");

        task.abort();
        let _ = task.await;

        let lease = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match try_acquire_admin_system_import_lease(&app).await {
                    Ok(lease) => break lease,
                    Err(AdminSystemImportLockError::Conflict) => tokio::task::yield_now().await,
                    Err(error) => panic!("unexpected lock acquisition error: {error:?}"),
                }
            }
        })
        .await
        .expect("cancelled import should release its lease promptly");
        release_admin_system_import_lease(&app, &lease).await;
    }
}
