use std::future::Future;
use std::time::Duration;

use aether_contracts::ExecutionPlan;
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};

const STREAM_IDLE_TIMEOUT_MS_ENV: &str = "AETHER_GATEWAY_UPSTREAM_STREAM_IDLE_TIMEOUT_MS";
const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;

pub(crate) fn resolve_stream_idle_timeout(plan: &ExecutionPlan) -> Option<Duration> {
    if !plan.stream {
        return None;
    }
    let configured = std::env::var(STREAM_IDLE_TIMEOUT_MS_ENV).ok();
    stream_idle_timeout_from_config(
        plan.timeouts.as_ref().and_then(|timeouts| timeouts.read_ms),
        configured.as_deref(),
    )
}

fn stream_idle_timeout_from_config(
    read_ms: Option<u64>,
    configured: Option<&str>,
) -> Option<Duration> {
    let timeout_ms = read_ms
        .or_else(|| configured.and_then(|value| value.trim().parse::<u64>().ok()))
        .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT_MS);
    // Zero explicitly disables the idle limit for providers with long silent reasoning phases.
    (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms))
}

pub(crate) fn stream_idle_timeout_message(timeout: Duration) -> String {
    format!(
        "provider stream idle read timeout after {} ms",
        timeout.as_millis()
    )
}

pub(crate) async fn await_stream_idle_read<T>(
    future: impl Future<Output = T>,
    timeout: Option<Duration>,
) -> Result<T, Duration> {
    match timeout {
        Some(timeout) => tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| timeout),
        None => Ok(future.await),
    }
}

pub(crate) fn skip_empty_upstream_chunks<E: Send + 'static>(
    upstream: impl Stream<Item = Result<Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, E>> + Send {
    async_stream::stream! {
        tokio::pin!(upstream);
        while let Some(item) = upstream.next().await {
            match item {
                Ok(chunk) if chunk.is_empty() => {
                    // Empty frames are not progress; yield so an always-ready source cannot
                    // monopolize the executor or prevent its enclosing timeout from firing.
                    tokio::task::yield_now().await;
                }
                item => yield item,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn idle_timeout_configuration_preserves_provider_override_and_explicit_disable() {
        assert_eq!(
            stream_idle_timeout_from_config(None, None),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            stream_idle_timeout_from_config(None, Some(" invalid ")),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            stream_idle_timeout_from_config(None, Some(" 600000 ")),
            Some(Duration::from_secs(600))
        );
        assert_eq!(
            stream_idle_timeout_from_config(Some(120_000), Some("600000")),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            stream_idle_timeout_from_config(Some(0), Some("600000")),
            None
        );
        assert_eq!(stream_idle_timeout_from_config(None, Some("0")), None);
    }

    #[tokio::test]
    async fn idle_timeout_cancels_the_pending_upstream_read() {
        struct DropMarker(Arc<AtomicBool>);
        impl Drop for DropMarker {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let marker = DropMarker(Arc::clone(&dropped));
        let outcome = await_stream_idle_read(
            async move {
                let _marker = marker;
                std::future::pending::<()>().await;
            },
            Some(Duration::from_millis(5)),
        )
        .await;
        assert_eq!(outcome, Err(Duration::from_millis(5)));
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn idle_timeout_allows_progressing_stream_to_outlive_one_timeout() {
        for _ in 0..3 {
            assert_eq!(
                await_stream_idle_read(
                    async {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        1
                    },
                    Some(Duration::from_millis(25))
                )
                .await,
                Ok(1)
            );
        }
        assert_eq!(
            await_stream_idle_read(
                async {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    2
                },
                None
            )
            .await,
            Ok(2)
        );
    }

    #[tokio::test]
    async fn empty_upstream_chunks_do_not_reset_idle_timeout() {
        let upstream = futures_util::stream::repeat(Ok::<_, ()>(Bytes::new()));
        let filtered = skip_empty_upstream_chunks(upstream);
        tokio::pin!(filtered);
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            await_stream_idle_read(filtered.next(), Some(Duration::from_millis(5))),
        )
        .await
        .expect("empty ready chunks must yield to the idle timer");
        assert_eq!(outcome, Err(Duration::from_millis(5)));
    }

    #[tokio::test]
    async fn downstream_keepalive_ticks_do_not_reset_pending_upstream_idle_timeout() {
        let read = await_stream_idle_read(
            std::future::pending::<()>(),
            Some(Duration::from_millis(30)),
        );
        tokio::pin!(read);
        let mut keepalive = tokio::time::interval(Duration::from_millis(2));
        let mut ticks = 0;
        loop {
            tokio::select! {
                result = &mut read => {
                    assert_eq!(result, Err(Duration::from_millis(30)));
                    assert!(ticks > 0);
                    break;
                }
                _ = keepalive.tick() => { ticks += 1; }
            }
        }
    }
}
