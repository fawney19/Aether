use crate::handlers::admin::request::AdminAppState;
use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::time::Duration;

const DEVICE_POLL_LEASE_TTL: Duration = Duration::from_secs(300);
const DEVICE_POLL_LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdminProviderOAuthDevicePollLeaseFailure {
    Lost,
    Unavailable,
}

pub(super) enum AdminProviderOAuthDevicePollLeaseAcquire {
    Acquired(AdminProviderOAuthDevicePollLease),
    Contended,
    Unavailable,
}

pub(super) struct AdminProviderOAuthDevicePollLease {
    runtime: RuntimeState,
    lease: Option<RuntimeLockLease>,
}

impl AdminProviderOAuthDevicePollLease {
    pub(super) async fn try_acquire(
        state: &AdminAppState<'_>,
        session_id: &str,
    ) -> AdminProviderOAuthDevicePollLeaseAcquire {
        let runtime = state.runtime_state().clone();
        let lock_key = admin_provider_oauth_device_poll_lock_key(session_id);
        let owner = format!(
            "aether-gateway-admin-provider-oauth-device-poll:{}",
            uuid::Uuid::new_v4()
        );
        match runtime
            .lock_try_acquire(&lock_key, &owner, DEVICE_POLL_LEASE_TTL)
            .await
        {
            Ok(Some(lease)) => AdminProviderOAuthDevicePollLeaseAcquire::Acquired(Self {
                runtime,
                lease: Some(lease),
            }),
            Ok(None) => AdminProviderOAuthDevicePollLeaseAcquire::Contended,
            Err(error) => {
                tracing::warn!(
                    lock_key = %lock_key,
                    error = %crate::error::redact_error_detail(&error),
                    "gateway provider OAuth device poll lease acquisition failed"
                );
                AdminProviderOAuthDevicePollLeaseAcquire::Unavailable
            }
        }
    }

    pub(super) async fn run<F, Output>(
        &self,
        operation: F,
    ) -> Result<Output, AdminProviderOAuthDevicePollLeaseFailure>
    where
        F: Future<Output = Output>,
    {
        let output = prefer_lease_loss(
            operation,
            wait_for_admin_provider_oauth_device_poll_lease_loss(
                self.runtime.clone(),
                self.lease
                    .as_ref()
                    .expect("an acquired device poll lease must contain its runtime lease")
                    .clone(),
            ),
        )
        .await?;
        self.confirm_ownership().await?;
        Ok(output)
    }

    async fn confirm_ownership(&self) -> Result<(), AdminProviderOAuthDevicePollLeaseFailure> {
        let lease = self
            .lease
            .as_ref()
            .expect("an acquired device poll lease must contain its runtime lease");
        match self.runtime.lock_renew(lease, DEVICE_POLL_LEASE_TTL).await {
            Ok(renewed) => ensure_admin_provider_oauth_device_poll_lease_renewed(renewed),
            Err(error) => {
                tracing::error!(
                    lock_key = %lease.key,
                    error = %crate::error::redact_error_detail(&error),
                    "gateway provider OAuth device poll final lease renewal failed"
                );
                Err(AdminProviderOAuthDevicePollLeaseFailure::Unavailable)
            }
        }
    }

    pub(super) async fn release(mut self) {
        let Some(lease) = self.lease.as_ref().cloned() else {
            return;
        };
        match self.runtime.lock_release(&lease).await {
            Ok(_) => {
                self.lease.take();
            }
            Err(error) => {
                tracing::warn!(
                    lock_key = %lease.key,
                    error = %crate::error::redact_error_detail(&error),
                    "gateway provider OAuth device poll lease release failed"
                );
                // Keep the lease in the guard so Drop can make one best-effort retry.
            }
        }
    }
}

impl Drop for AdminProviderOAuthDevicePollLease {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let runtime = self.runtime.clone();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            if let Err(error) = runtime.lock_release(&lease).await {
                tracing::warn!(
                    lock_key = %lease.key,
                    error = %crate::error::redact_error_detail(&error),
                    "gateway provider OAuth device poll lease Drop release failed"
                );
            }
        });
    }
}

fn admin_provider_oauth_device_poll_lock_key(session_id: &str) -> String {
    format!(
        "admin-provider-oauth-device-poll:sha256:{:x}",
        Sha256::digest(session_id.as_bytes())
    )
}

fn ensure_admin_provider_oauth_device_poll_lease_renewed(
    renewed: bool,
) -> Result<(), AdminProviderOAuthDevicePollLeaseFailure> {
    if renewed {
        Ok(())
    } else {
        Err(AdminProviderOAuthDevicePollLeaseFailure::Lost)
    }
}

async fn wait_for_admin_provider_oauth_device_poll_lease_loss(
    runtime: RuntimeState,
    lease: RuntimeLockLease,
) -> AdminProviderOAuthDevicePollLeaseFailure {
    let first_renewal = tokio::time::Instant::now() + DEVICE_POLL_LEASE_RENEW_INTERVAL;
    let mut renewal_timer =
        tokio::time::interval_at(first_renewal, DEVICE_POLL_LEASE_RENEW_INTERVAL);
    renewal_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        renewal_timer.tick().await;
        match runtime.lock_renew(&lease, DEVICE_POLL_LEASE_TTL).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::error!(
                    lock_key = %lease.key,
                    "gateway provider OAuth device poll lease was lost"
                );
                return AdminProviderOAuthDevicePollLeaseFailure::Lost;
            }
            Err(error) => {
                tracing::error!(
                    lock_key = %lease.key,
                    error = ?error,
                    "gateway provider OAuth device poll lease renewal failed"
                );
                return AdminProviderOAuthDevicePollLeaseFailure::Unavailable;
            }
        }
    }
}

async fn prefer_lease_loss<Operation, LeaseLoss, Output>(
    operation: Operation,
    lease_loss: LeaseLoss,
) -> Result<Output, AdminProviderOAuthDevicePollLeaseFailure>
where
    Operation: Future<Output = Output>,
    LeaseLoss: Future<Output = AdminProviderOAuthDevicePollLeaseFailure>,
{
    tokio::pin!(operation);
    tokio::pin!(lease_loss);
    tokio::select! {
        biased;
        failure = &mut lease_loss => Err(failure),
        output = &mut operation => Ok(output),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        admin_provider_oauth_device_poll_lock_key,
        ensure_admin_provider_oauth_device_poll_lease_renewed, prefer_lease_loss,
        AdminProviderOAuthDevicePollLeaseFailure,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::task::{Context, Poll};

    struct ReadyOperation {
        polled: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl Future for ReadyOperation {
        type Output = &'static str;

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Self::Output> {
            self.polled.store(true, Ordering::Release);
            Poll::Ready("must not be published")
        }
    }

    impl Drop for ReadyOperation {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[test]
    fn device_poll_lease_lock_key_hashes_session_id() {
        let session_id = "secret-device-session";
        let first = admin_provider_oauth_device_poll_lock_key(session_id);
        let second = admin_provider_oauth_device_poll_lock_key(session_id);

        assert_eq!(first, second);
        assert!(!first.contains(session_id));
    }

    #[test]
    fn device_poll_lease_renew_false_fails_closed() {
        assert_eq!(
            ensure_admin_provider_oauth_device_poll_lease_renewed(false),
            Err(AdminProviderOAuthDevicePollLeaseFailure::Lost)
        );
        assert!(ensure_admin_provider_oauth_device_poll_lease_renewed(true).is_ok());
    }

    #[tokio::test]
    async fn device_poll_lease_loss_future_has_priority_and_cancels_operation() {
        let polled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let operation = ReadyOperation {
            polled: Arc::clone(&polled),
            dropped: Arc::clone(&dropped),
        };

        let result = prefer_lease_loss(
            operation,
            std::future::ready(AdminProviderOAuthDevicePollLeaseFailure::Lost),
        )
        .await;

        assert_eq!(result, Err(AdminProviderOAuthDevicePollLeaseFailure::Lost));
        assert!(!polled.load(Ordering::Acquire));
        assert!(dropped.load(Ordering::Acquire));
    }
}
