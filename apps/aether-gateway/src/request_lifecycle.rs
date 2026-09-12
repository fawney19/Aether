use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use aether_routing_core::RoutingExecutionPolicy;
use aether_usage_runtime::{UsageProducerGuard, UsageRuntime};
use axum::body::{Body, Bytes, HttpBody};
use http::Response;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt;

use crate::request_diagnostics::{scope_request_diagnostics_with, RequestDiagnostics};
use crate::GatewayError;

tokio::task_local! {
    static CANCEL_ON_CLIENT_DISCONNECT: Arc<AtomicBool>;
}

pub(crate) fn configure_client_disconnect(policy: RoutingExecutionPolicy) {
    let _ = CANCEL_ON_CLIENT_DISCONNECT.try_with(|cancel| {
        cancel.store(policy.cancel_on_client_disconnect, Ordering::Release);
    });
}

pub(crate) fn cancel_on_client_disconnect() -> bool {
    CANCEL_ON_CLIENT_DISCONNECT
        .try_with(|cancel| cancel.load(Ordering::Acquire))
        .unwrap_or(false)
}

#[cfg(test)]
pub(crate) async fn run_request<F>(future: F) -> Result<Response<Body>, GatewayError>
where
    F: Future<Output = Result<Response<Body>, GatewayError>> + Send + 'static,
{
    run_tracked_request(future, None).await
}

pub(crate) async fn run_request_with_usage<F>(
    usage: Arc<UsageRuntime>,
    future: F,
) -> Result<Response<Body>, GatewayError>
where
    F: Future<Output = Result<Response<Body>, GatewayError>> + Send + 'static,
{
    run_tracked_request(future, Some(Arc::new(usage.track_producer()))).await
}

async fn run_tracked_request<F>(
    future: F,
    producer: Option<Arc<UsageProducerGuard>>,
) -> Result<Response<Body>, GatewayError>
where
    F: Future<Output = Result<Response<Body>, GatewayError>> + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(true));
    let diagnostics = Arc::new(RequestDiagnostics::default());
    let cancel_for_response = Arc::clone(&cancel);
    let producer_for_request = producer.clone();
    let future = CANCEL_ON_CLIENT_DISCONNECT.scope(
        Arc::clone(&cancel),
        scope_request_diagnostics_with(Some(Arc::clone(&diagnostics)), async move {
            let response = future.await?;
            let complete_on_disconnect = !cancel_for_response.load(Ordering::Acquire);
            if !complete_on_disconnect && producer.is_none() {
                return Ok(response);
            }
            Ok(response.map(|body| {
                Body::new(CompleteOnDisconnectBody {
                    body: Some(body),
                    diagnostics,
                    complete_on_disconnect,
                    producer,
                })
            }))
        }),
    );
    CompleteOnDisconnectRequest {
        future: Some(Box::pin(future)),
        cancel,
        producer: producer_for_request,
    }
    .await
}

struct CompleteOnDisconnectRequest<F>
where
    F: Future<Output = Result<Response<Body>, GatewayError>> + Send + 'static,
{
    future: Option<Pin<Box<F>>>,
    cancel: Arc<AtomicBool>,
    producer: Option<Arc<UsageProducerGuard>>,
}

impl<F> Future for CompleteOnDisconnectRequest<F>
where
    F: Future<Output = Result<Response<Body>, GatewayError>> + Send + 'static,
{
    type Output = Result<Response<Body>, GatewayError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let result = self
            .future
            .as_mut()
            .expect("request future")
            .as_mut()
            .poll(context);
        if result.is_ready() {
            self.future.take();
        }
        result
    }
}

impl<F> Drop for CompleteOnDisconnectRequest<F>
where
    F: Future<Output = Result<Response<Body>, GatewayError>> + Send + 'static,
{
    fn drop(&mut self) {
        if self.cancel.load(Ordering::Acquire) {
            return;
        }
        if let (Some(future), Ok(runtime)) =
            (self.future.take(), tokio::runtime::Handle::try_current())
        {
            let producer = self.producer.take();
            runtime.spawn(async move {
                let _producer = producer;
                if let Ok(response) = future.await {
                    drain_body(response.into_body()).await;
                }
            });
        }
    }
}

struct CompleteOnDisconnectBody {
    body: Option<Body>,
    diagnostics: Arc<RequestDiagnostics>,
    complete_on_disconnect: bool,
    // Drop the body first so its terminal handoff registers before this guard ends.
    producer: Option<Arc<UsageProducerGuard>>,
}

impl HttpBody for CompleteOnDisconnectBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let Some(body) = self.body.as_mut() else {
            return Poll::Ready(None);
        };
        let result = Pin::new(body).poll_frame(context);
        if matches!(result, Poll::Ready(None | Some(Err(_)))) {
            self.body.take();
            self.producer.take();
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.body.as_ref().is_none_or(HttpBody::is_end_stream)
    }

    fn size_hint(&self) -> SizeHint {
        self.body
            .as_ref()
            .map(HttpBody::size_hint)
            .unwrap_or_else(|| SizeHint::with_exact(0))
    }
}

impl Drop for CompleteOnDisconnectBody {
    fn drop(&mut self) {
        if !self.complete_on_disconnect {
            return;
        }
        let Some(body) = self.body.take().filter(|body| !body.is_end_stream()) else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let producer = self.producer.take();
            runtime.spawn(scope_request_diagnostics_with(
                Some(Arc::clone(&self.diagnostics)),
                async move {
                    let _producer = producer;
                    drain_body(body).await;
                },
            ));
        }
    }
}

async fn drain_body(mut body: Body) {
    while let Some(frame) = body.frame().await {
        if frame.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Duration;

    use futures_util::stream;
    use http::HeaderMap;
    use http_body_util::StreamBody;
    use tokio::sync::{mpsc, oneshot};

    use super::*;

    #[tokio::test]
    async fn disconnected_request_finishes_and_keeps_admission_and_diagnostics() {
        let gate = aether_runtime::ConcurrencyGate::new("disconnect_request", 1);
        let permit = gate.try_acquire().unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (finished_tx, finished_rx) = oneshot::channel();
        let request = tokio::spawn(run_request(async move {
            let _permit = permit;
            configure_client_disconnect(RoutingExecutionPolicy::default());
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            assert!(crate::request_diagnostics::current_request_diagnostics().is_some());
            finished_tx.send(()).unwrap();
            Ok(Response::new(Body::empty()))
        }));
        started_rx.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(gate.snapshot().in_flight, 1);
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), finished_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(gate.snapshot().in_flight, 0);
    }

    #[tokio::test]
    async fn enabled_cancellation_and_unresolved_requests_drop_immediately() {
        for resolve_policy in [false, true] {
            let (started_tx, started_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel::<()>();
            let request = tokio::spawn(run_request(async move {
                if resolve_policy {
                    configure_client_disconnect(RoutingExecutionPolicy {
                        cancel_on_client_disconnect: true,
                        ..Default::default()
                    });
                }
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok(Response::new(Body::empty()))
            }));
            started_rx.await.unwrap();
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            assert!(release_tx.send(()).is_err());
        }
    }

    #[tokio::test]
    async fn disconnected_body_drains_without_buffering_and_holds_admission() {
        for consume_first_chunk in [false, true] {
            let gate = aether_runtime::ConcurrencyGate::new("disconnect_body", 1);
            let permit = gate.try_acquire().unwrap();
            let (sender, receiver) = mpsc::channel(1);
            let (finished_tx, finished_rx) = oneshot::channel();
            let response = run_request(async move {
                configure_client_disconnect(RoutingExecutionPolicy::default());
                let body = Body::from_stream(stream::unfold(
                    (receiver, finished_tx, permit),
                    |(mut receiver, finished_tx, permit)| async move {
                        match receiver.recv().await {
                            Some(bytes) => {
                                Some((Ok::<_, io::Error>(bytes), (receiver, finished_tx, permit)))
                            }
                            None => {
                                assert!(crate::request_diagnostics::current_request_diagnostics()
                                    .is_some());
                                finished_tx.send(()).unwrap();
                                None
                            }
                        }
                    },
                ));
                Ok(Response::new(body))
            })
            .await
            .unwrap();
            let mut body = response.into_body();
            if consume_first_chunk {
                sender.send(Bytes::from_static(b"first")).await.unwrap();
                assert_eq!(
                    body.frame().await.unwrap().unwrap().into_data().unwrap(),
                    "first"
                );
            }
            drop(body);
            assert_eq!(gate.snapshot().in_flight, 1);
            tokio::time::timeout(Duration::from_secs(1), async {
                for _ in 0..100 {
                    sender.send(Bytes::from_static(b"remaining")).await.unwrap();
                }
                drop(sender);
                finished_rx.await.unwrap();
            })
            .await
            .unwrap();
            assert_eq!(gate.snapshot().in_flight, 0);
        }
    }

    #[tokio::test]
    async fn enabled_cancellation_drops_stream_receiver() {
        let (sender, receiver) = mpsc::channel::<Result<Bytes, io::Error>>(1);
        let response = run_request(async move {
            configure_client_disconnect(RoutingExecutionPolicy {
                cancel_on_client_disconnect: true,
                ..Default::default()
            });
            Ok(Response::new(Body::from_stream(stream::unfold(
                receiver,
                |mut receiver| async { receiver.recv().await.map(|item| (item, receiver)) },
            ))))
        })
        .await
        .unwrap();
        drop(response);
        assert!(sender.is_closed());
    }

    #[tokio::test]
    async fn usage_shutdown_waits_for_a_disconnected_request_before_headers() {
        let usage = Arc::new(UsageRuntime::disabled());
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let request = tokio::spawn(run_request_with_usage(usage.clone(), async move {
            configure_client_disconnect(RoutingExecutionPolicy::default());
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(Response::new(Body::empty()))
        }));
        started_rx.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert!(usage.shutdown(Duration::from_millis(30)).await.is_err());
        assert_eq!(usage.metrics_snapshot().producers_in_flight, 1);
        release_tx.send(()).unwrap();
        usage.shutdown(Duration::from_secs(1)).await.unwrap();
        assert_eq!(usage.metrics_snapshot().producers_in_flight, 0);
    }

    #[tokio::test]
    async fn usage_shutdown_waits_for_disconnected_body_drain() {
        let usage = Arc::new(UsageRuntime::disabled());
        let (sender, receiver) = mpsc::channel::<Result<Bytes, io::Error>>(1);
        let response = run_request_with_usage(usage.clone(), async move {
            configure_client_disconnect(RoutingExecutionPolicy::default());
            Ok(Response::new(Body::from_stream(stream::unfold(
                receiver,
                |mut receiver| async { receiver.recv().await.map(|item| (item, receiver)) },
            ))))
        })
        .await
        .unwrap();
        drop(response);
        assert!(usage.shutdown(Duration::from_millis(30)).await.is_err());
        sender.send(Ok(Bytes::from_static(b"last"))).await.unwrap();
        drop(sender);
        usage.shutdown(Duration::from_secs(1)).await.unwrap();
        assert_eq!(usage.metrics_snapshot().producers_in_flight, 0);
    }

    #[tokio::test]
    async fn tracked_bodies_release_shutdown_on_cancellation_or_eof() {
        for cancel_on_client_disconnect in [false, true] {
            let usage = Arc::new(UsageRuntime::disabled());
            let (sender, receiver) = mpsc::channel::<Result<Bytes, io::Error>>(1);
            let response = run_request_with_usage(usage.clone(), async move {
                configure_client_disconnect(RoutingExecutionPolicy {
                    cancel_on_client_disconnect,
                    ..Default::default()
                });
                Ok(Response::new(Body::from_stream(stream::unfold(
                    receiver,
                    |mut receiver| async { receiver.recv().await.map(|item| (item, receiver)) },
                ))))
            })
            .await
            .unwrap();
            let mut body = response.into_body();
            if cancel_on_client_disconnect {
                drop(body);
                assert!(sender.is_closed());
            } else {
                drop(sender);
                assert!(body.frame().await.is_none());
                assert_eq!(usage.metrics_snapshot().producers_in_flight, 0);
            }
            usage.shutdown(Duration::from_secs(1)).await.unwrap();
        }
    }

    #[tokio::test]
    async fn connected_response_preserves_headers_size_hint_and_trailers() {
        let response = run_request(async {
            configure_client_disconnect(RoutingExecutionPolicy::default());
            Ok(Response::builder()
                .status(201)
                .header("x-test", "unchanged")
                .body(Body::from("hello"))
                .unwrap())
        })
        .await
        .unwrap();
        assert_eq!(response.status(), 201);
        assert_eq!(response.headers()["x-test"], "unchanged");
        assert_eq!(response.body().size_hint().exact(), Some(5));
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "hello"
        );

        let mut trailers = HeaderMap::new();
        trailers.insert("x-finished", "yes".parse().unwrap());
        let response = run_request(async move {
            configure_client_disconnect(RoutingExecutionPolicy::default());
            let frames = stream::iter([
                Ok::<_, io::Error>(Frame::data(Bytes::from_static(b"hello"))),
                Ok(Frame::trailers(trailers)),
            ]);
            Ok(Response::new(Body::new(StreamBody::new(frames))))
        })
        .await
        .unwrap();
        let collected = response.into_body().collect().await.unwrap();
        assert_eq!(collected.trailers().unwrap()["x-finished"], "yes");
        assert_eq!(collected.to_bytes(), "hello");
    }
}
