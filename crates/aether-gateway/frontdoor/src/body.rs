//! Bounded request-body buffering for frontdoor adapters.
//!
//! The policy reserves weighted memory before reading a body and holds the
//! reservation through the caller's normalization callback. This keeps body
//! buffering independent from gateway business routing while preventing a
//! burst of compressed requests from bypassing the memory budget.

use axum::body::{to_bytes, Body};
use bytes::Bytes;
use http::{header, HeaderMap, StatusCode};
use std::error::Error as StdError;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_BODY_BUFFER_PERMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct BodyBufferPolicy {
    max_bytes: u64,
    read_timeout: Duration,
    queue_timeout: Duration,
    budget_bytes: usize,
    permit_bytes: usize,
    budget: Arc<Semaphore>,
}

impl BodyBufferPolicy {
    pub fn new(
        max_bytes: u64,
        read_timeout: Duration,
        queue_timeout: Duration,
        budget_bytes: usize,
        budget: Arc<Semaphore>,
    ) -> Self {
        Self::with_permit_bytes(
            max_bytes,
            read_timeout,
            queue_timeout,
            budget_bytes,
            DEFAULT_BODY_BUFFER_PERMIT_BYTES,
            budget,
        )
    }

    pub fn with_permit_bytes(
        max_bytes: u64,
        read_timeout: Duration,
        queue_timeout: Duration,
        budget_bytes: usize,
        permit_bytes: usize,
        budget: Arc<Semaphore>,
    ) -> Self {
        Self {
            max_bytes,
            read_timeout,
            queue_timeout,
            budget_bytes,
            permit_bytes: permit_bytes.max(1),
            budget,
        }
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn read_timeout(&self) -> Duration {
        self.read_timeout
    }

    pub fn queue_timeout(&self) -> Duration {
        self.queue_timeout
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn reservation_bytes(&self, headers: &HeaderMap) -> usize {
        reservation_bytes(headers, self.max_bytes)
    }

    pub fn reservation_permits(&self, reservation_bytes: usize) -> u32 {
        reservation_permits(reservation_bytes, self.permit_bytes)
    }

    pub async fn reserve(
        &self,
        headers: &HeaderMap,
    ) -> Result<BodyBufferReservation, BodyBufferError> {
        if let Some(declared) = declared_content_length(headers) {
            if declared > self.max_bytes {
                return Err(BodyBufferError::TooLarge {
                    limit_bytes: self.max_bytes,
                });
            }
        }

        let requested_bytes = self.reservation_bytes(headers);
        let permits = self.reservation_permits(requested_bytes);
        let timeout_ms = duration_millis(self.queue_timeout);
        let permit = match tokio::time::timeout(
            self.queue_timeout,
            Arc::clone(&self.budget).acquire_many_owned(permits),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) | Err(_) => {
                return Err(BodyBufferError::Overloaded {
                    requested_bytes,
                    budget_bytes: self.budget_bytes,
                    timeout_ms,
                });
            }
        };

        Ok(BodyBufferReservation {
            permit,
            max_bytes: self.max_bytes,
            read_timeout: self.read_timeout,
            requested_bytes,
        })
    }
}

#[derive(Debug)]
pub struct BodyBufferReservation {
    permit: OwnedSemaphorePermit,
    max_bytes: u64,
    read_timeout: Duration,
    requested_bytes: usize,
}

impl BodyBufferReservation {
    pub fn requested_bytes(&self) -> usize {
        self.requested_bytes
    }

    pub async fn collect(self, body: Body) -> Result<BufferedBody, BodyBufferError> {
        let Self {
            permit,
            max_bytes,
            read_timeout,
            requested_bytes,
        } = self;
        let started_at = Instant::now();
        let body_limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        let bytes = match tokio::time::timeout(read_timeout, to_bytes(body, body_limit)).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) if collection_exceeded_limit(&error) => {
                return Err(BodyBufferError::TooLarge {
                    limit_bytes: max_bytes,
                });
            }
            Ok(Err(error)) => {
                return Err(BodyBufferError::ReadFailed {
                    message: error.to_string(),
                });
            }
            Err(_) => {
                return Err(BodyBufferError::Timeout {
                    timeout_ms: duration_millis(read_timeout),
                });
            }
        };

        Ok(BufferedBody {
            bytes,
            permit: Some(permit),
            requested_bytes,
            elapsed: started_at.elapsed(),
        })
    }
}

#[derive(Debug)]
pub struct BufferedBody {
    bytes: Bytes,
    permit: Option<OwnedSemaphorePermit>,
    requested_bytes: usize,
    elapsed: Duration,
}

impl BufferedBody {
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    pub fn requested_bytes(&self) -> usize {
        self.requested_bytes
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Apply normalization while retaining the memory permit until the
    /// callback completes.
    pub fn try_map<T, E>(self, map: impl FnOnce(Bytes) -> Result<T, E>) -> Result<T, E> {
        let Self { bytes, permit, .. } = self;
        let result = map(bytes);
        drop(permit);
        result
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BodyBufferError {
    TooLarge {
        limit_bytes: u64,
    },
    Overloaded {
        requested_bytes: usize,
        budget_bytes: usize,
        timeout_ms: u64,
    },
    Timeout {
        timeout_ms: u64,
    },
    ReadFailed {
        message: String,
    },
}

impl BodyBufferError {
    pub fn http_status(&self) -> StatusCode {
        match self {
            Self::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Overloaded { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout { .. } => StatusCode::REQUEST_TIMEOUT,
            Self::ReadFailed { .. } => StatusCode::BAD_REQUEST,
        }
    }

    pub fn client_message(&self) -> String {
        match self {
            Self::TooLarge { limit_bytes } => format!("Request body exceeds {limit_bytes} bytes"),
            Self::Overloaded { .. } => {
                "Request body buffering capacity is temporarily exhausted".to_string()
            }
            Self::Timeout { .. } => {
                "Request body read timed out before the gateway could route the request".to_string()
            }
            Self::ReadFailed { .. } => "Failed to read request body".to_string(),
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::TooLarge { .. } => "request_body_too_large",
            Self::Overloaded { .. } => "request_body_buffer_overloaded",
            Self::Timeout { .. } => "request_body_read_timeout",
            Self::ReadFailed { .. } => "request_body_read_failed",
        }
    }
}

fn declared_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn reservation_bytes(headers: &HeaderMap, max_bytes: u64) -> usize {
    let max_bytes = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let encoded = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty() && !value.eq_ignore_ascii_case("identity"));
    if encoded {
        return max_bytes;
    }
    declared_content_length(headers)
        .map(|value| usize::try_from(value).unwrap_or(usize::MAX).min(max_bytes))
        .unwrap_or(max_bytes)
}

fn reservation_permits(reservation_bytes: usize, permit_bytes: usize) -> u32 {
    let permits = reservation_bytes
        .max(1)
        .saturating_add(permit_bytes.saturating_sub(1))
        / permit_bytes.max(1);
    u32::try_from(permits).unwrap_or(u32::MAX).max(1)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn collection_exceeded_limit(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.to_string().contains("length limit exceeded") {
            return true;
        }
        current = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{BodyBufferError, BodyBufferPolicy, DEFAULT_BODY_BUFFER_PERMIT_BYTES};
    use axum::body::{Body, Bytes};
    use futures_util::{stream, StreamExt};
    use http::{header, HeaderMap, HeaderValue};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;

    fn policy(max_bytes: u64, timeout: Duration, budget: Arc<Semaphore>) -> BodyBufferPolicy {
        BodyBufferPolicy::with_permit_bytes(
            max_bytes,
            timeout,
            timeout,
            max_bytes as usize,
            DEFAULT_BODY_BUFFER_PERMIT_BYTES,
            budget,
        )
    }

    #[tokio::test]
    async fn rejects_declared_content_length_before_reading_body() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("6"));
        let error = policy(5, Duration::from_secs(1), Arc::new(Semaphore::new(1)))
            .reserve(&headers)
            .await
            .expect_err("declared body should be rejected");
        assert_eq!(error, BodyBufferError::TooLarge { limit_bytes: 5 });
    }

    #[tokio::test]
    async fn holds_weighted_permit_through_normalization_callback() {
        let budget = Arc::new(Semaphore::new(1));
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("2"));
        let reservation = policy(1024, Duration::from_secs(1), Arc::clone(&budget))
            .reserve(&headers)
            .await
            .expect("reservation should succeed");
        let buffered = reservation
            .collect(Body::from(Bytes::from_static(b"{}")))
            .await
            .expect("body should collect");
        assert_eq!(budget.available_permits(), 0);
        let normalized = buffered
            .try_map(Ok::<_, ()>)
            .expect("mapping should succeed");
        assert_eq!(normalized.as_ref(), b"{}");
        assert_eq!(budget.available_permits(), 1);
    }

    #[tokio::test]
    async fn rejects_chunked_body_when_collected_bytes_exceed_limit() {
        let reservation = policy(5, Duration::from_secs(1), Arc::new(Semaphore::new(1)))
            .reserve(&HeaderMap::new())
            .await
            .expect("reservation should succeed");
        let error = reservation
            .collect(Body::from(Bytes::from_static(b"abcdef")))
            .await
            .expect_err("chunked body should remain bounded while reading");
        assert_eq!(error, BodyBufferError::TooLarge { limit_bytes: 5 });
    }

    #[tokio::test]
    async fn times_out_slow_body_reads() {
        let stream = stream::once(async { Ok::<Bytes, std::io::Error>(Bytes::from_static(b"{")) })
            .chain(stream::pending());
        let reservation = policy(1024, Duration::from_millis(5), Arc::new(Semaphore::new(1)))
            .reserve(&HeaderMap::new())
            .await
            .expect("reservation should succeed");
        let error = reservation
            .collect(Body::from_stream(stream))
            .await
            .expect_err("slow body should time out");
        assert_eq!(error, BodyBufferError::Timeout { timeout_ms: 5 });
    }

    #[tokio::test]
    async fn rejects_when_weighted_budget_is_exhausted() {
        let budget = Arc::new(Semaphore::new(1));
        let _held = Arc::clone(&budget)
            .acquire_owned()
            .await
            .expect("test permit should be available");
        let error = policy(1024, Duration::from_millis(5), budget)
            .reserve(&HeaderMap::new())
            .await
            .expect_err("exhausted budget should fail closed");
        assert!(matches!(error, BodyBufferError::Overloaded { .. }));
    }
}
