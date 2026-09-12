use async_stream::stream;
use axum::body::Body;
use axum::http::Response;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

use crate::concurrency::ConcurrencyPermit;

const ADMISSION_HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub trait AdmissionPermitHealth: Send + Sync {
    fn is_healthy(&self) -> bool;

    fn requires_health_poll(&self) -> bool {
        true
    }
}

impl AdmissionPermitHealth for ConcurrencyPermit {
    fn is_healthy(&self) -> bool {
        true
    }

    fn requires_health_poll(&self) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct AdmissionPermit {
    _permits: Vec<Arc<dyn AdmissionPermitHealth>>,
}

impl std::fmt::Debug for AdmissionPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmissionPermit")
            .field("permit_count", &self._permits.len())
            .finish()
    }
}

impl AdmissionPermit {
    pub fn from_parts<D: AdmissionPermitHealth + 'static>(
        local: Option<ConcurrencyPermit>,
        distributed: Option<D>,
    ) -> Option<Self> {
        let mut permits = Vec::<Arc<dyn AdmissionPermitHealth>>::new();
        if let Some(local) = local {
            permits.push(Arc::new(local));
        }
        if let Some(distributed) = distributed {
            permits.push(Arc::new(distributed));
        }
        (!permits.is_empty()).then_some(Self { _permits: permits })
    }

    pub fn combine(permits: impl IntoIterator<Item = AdmissionPermit>) -> Option<Self> {
        let permits = permits
            .into_iter()
            .flat_map(|permit| permit._permits)
            .collect::<Vec<_>>();
        (!permits.is_empty()).then_some(Self { _permits: permits })
    }

    pub fn is_healthy(&self) -> bool {
        self._permits.iter().all(|permit| permit.is_healthy())
    }

    fn requires_health_poll(&self) -> bool {
        self._permits
            .iter()
            .any(|permit| permit.requires_health_poll())
    }
}

impl From<ConcurrencyPermit> for AdmissionPermit {
    fn from(value: ConcurrencyPermit) -> Self {
        Self {
            _permits: vec![Arc::new(value)],
        }
    }
}

pub fn maybe_hold_axum_response_permit(
    response: Response<Body>,
    permit: Option<AdmissionPermit>,
) -> Response<Body> {
    match permit {
        Some(permit) => hold_axum_response_permit(response, permit),
        None => response,
    }
}

pub async fn hold_admission_permit_until<F>(permit: Option<AdmissionPermit>, future: F)
where
    F: std::future::Future<Output = ()>,
{
    hold_admission_permit_until_with_interval(permit, future, ADMISSION_HEALTH_POLL_INTERVAL).await;
}

fn hold_axum_response_permit(response: Response<Body>, permit: AdmissionPermit) -> Response<Body> {
    hold_axum_response_permit_with_interval(response, permit, ADMISSION_HEALTH_POLL_INTERVAL)
}

async fn hold_admission_permit_until_with_interval<F>(
    permit: Option<AdmissionPermit>,
    future: F,
    health_poll_interval: Duration,
) where
    F: std::future::Future<Output = ()>,
{
    let Some(permit) = permit else {
        future.await;
        return;
    };
    if !permit.requires_health_poll() {
        let _permit = permit;
        future.await;
        return;
    }

    tokio::pin!(future);
    let mut health = tokio::time::interval(health_poll_interval);
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    health.tick().await;
    loop {
        if !permit.is_healthy() {
            break;
        }
        tokio::select! {
            _ = &mut future => break,
            _ = health.tick() => {
                if !permit.is_healthy() {
                    break;
                }
            }
        }
    }
}

fn hold_axum_response_permit_with_interval(
    response: Response<Body>,
    permit: AdmissionPermit,
    health_poll_interval: Duration,
) -> Response<Body> {
    if !permit.requires_health_poll() {
        let (parts, body) = response.into_parts();
        let stream = stream! {
            let _permit = permit;
            let mut body_stream = body.into_data_stream();
            while let Some(item) = body_stream.next().await {
                yield item;
            }
        };
        return Response::from_parts(parts, Body::from_stream(stream));
    }

    let (parts, body) = response.into_parts();
    let stream = stream! {
        let _permit = permit;
        let mut body_stream = body.into_data_stream();
        let mut health = tokio::time::interval(health_poll_interval);
        health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        health.tick().await;
        loop {
            if !_permit.is_healthy() {
                break;
            }
            tokio::select! {
                item = body_stream.next() => match item {
                    Some(item) if _permit.is_healthy() => yield item,
                    Some(_) | None => break,
                },
                _ = health.tick() => {
                    if !_permit.is_healthy() {
                        break;
                    }
                }
            }
        }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

#[cfg(test)]
mod tests {
    use super::{
        hold_admission_permit_until, hold_admission_permit_until_with_interval,
        hold_axum_response_permit_with_interval, maybe_hold_axum_response_permit, AdmissionPermit,
        AdmissionPermitHealth,
    };
    use crate::ConcurrencyGate;
    use axum::body::{to_bytes, Body};
    use axum::http::Response;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct TestPermitHealth(Arc<AtomicBool>);

    impl AdmissionPermitHealth for TestPermitHealth {
        fn is_healthy(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }

    #[tokio::test]
    async fn holds_permit_until_response_body_is_consumed() {
        let gate = ConcurrencyGate::new("test", 1);
        let permit = gate.try_acquire().expect("first permit");
        let response = Response::new(Body::from_stream(
            async_stream::stream! { yield Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(b"ok")); },
        ));

        let wrapped = maybe_hold_axum_response_permit(response, Some(permit.into()));
        assert_eq!(gate.snapshot().in_flight, 1);
        assert!(gate.try_acquire().is_err(), "permit should still be held");

        let body = to_bytes(wrapped.into_body(), usize::MAX)
            .await
            .expect("body should drain");
        assert_eq!(body.as_ref(), b"ok");
        assert_eq!(gate.snapshot().in_flight, 0);
    }

    #[test]
    fn cloned_permit_releases_capacity_only_after_last_clone_drops() {
        let gate = ConcurrencyGate::new("test", 1);
        let permit = AdmissionPermit::from(gate.try_acquire().expect("first permit"));
        let cloned = permit.clone();

        drop(permit);
        assert_eq!(gate.snapshot().in_flight, 1);
        assert!(
            gate.try_acquire().is_err(),
            "capacity should remain held by the clone"
        );

        drop(cloned);
        assert_eq!(gate.snapshot().in_flight, 0);
        assert!(
            gate.try_acquire().is_ok(),
            "last clone should release capacity"
        );
    }

    #[tokio::test]
    async fn holds_combined_local_and_distributed_permit_until_future_finishes() {
        let local_gate = ConcurrencyGate::new("local", 1);
        let local = local_gate.try_acquire().expect("local permit");
        let distributed_gate = ConcurrencyGate::new("distributed", 1);
        let distributed = distributed_gate.try_acquire().expect("distributed permit");

        let task = tokio::spawn(hold_admission_permit_until(
            AdmissionPermit::from_parts(Some(local), Some(distributed)),
            async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            },
        ));

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        assert!(
            local_gate.try_acquire().is_err(),
            "local permit should still be held"
        );
        assert!(
            distributed_gate.try_acquire().is_err(),
            "distributed permit should still be held"
        );

        task.await.expect("task should complete");
        assert_eq!(local_gate.snapshot().in_flight, 0);
        assert_eq!(distributed_gate.snapshot().in_flight, 0);
    }

    #[tokio::test]
    async fn unhealthy_distributed_permit_cancels_held_future() {
        let local_gate = ConcurrencyGate::new("local", 1);
        let local = local_gate.try_acquire().expect("local permit");
        let healthy = Arc::new(AtomicBool::new(true));
        let permit =
            AdmissionPermit::from_parts(Some(local), Some(TestPermitHealth(Arc::clone(&healthy))));

        let task = tokio::spawn(hold_admission_permit_until_with_interval(
            permit,
            std::future::pending(),
            std::time::Duration::from_millis(5),
        ));
        healthy.store(false, Ordering::Release);

        tokio::time::timeout(std::time::Duration::from_millis(100), task)
            .await
            .expect("unhealthy permit should cancel the held future")
            .expect("held future task should not panic");
        assert_eq!(local_gate.snapshot().in_flight, 0);
    }

    #[tokio::test]
    async fn unhealthy_distributed_permit_ends_idle_response_body() {
        let local_gate = ConcurrencyGate::new("local", 1);
        let local = local_gate.try_acquire().expect("local permit");
        let healthy = Arc::new(AtomicBool::new(true));
        let permit =
            AdmissionPermit::from_parts(Some(local), Some(TestPermitHealth(Arc::clone(&healthy))))
                .expect("combined permit");
        let response = Response::new(Body::from_stream(futures_util::stream::pending::<
            Result<axum::body::Bytes, std::convert::Infallible>,
        >()));
        let wrapped = hold_axum_response_permit_with_interval(
            response,
            permit,
            std::time::Duration::from_millis(5),
        );

        healthy.store(false, Ordering::Release);
        let body = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            to_bytes(wrapped.into_body(), usize::MAX),
        )
        .await
        .expect("unhealthy permit should end an idle body")
        .expect("body collection should succeed");
        assert!(body.is_empty());
        assert_eq!(local_gate.snapshot().in_flight, 0);
    }
}
