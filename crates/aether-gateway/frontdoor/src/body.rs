//! Bounded request-body buffering for frontdoor adapters.
//!
//! The policy grows weighted reservations as bytes arrive and holds them through
//! normalization. Growth never waits while retaining a partial buffer, so
//! concurrent uploads cannot deadlock while competing for the remaining budget.

use axum::body::Body;
use bytes::Bytes;
use futures_util::StreamExt;
use http::{header, HeaderMap, StatusCode};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_BODY_BUFFER_PERMIT_BYTES: usize = 64 * 1024;
const MAX_REQUEST_CONTENT_ENCODINGS: usize = 8;

#[derive(Debug, Clone)]
pub struct BodyBufferPolicy {
    max_bytes: u64,
    read_timeout: Option<Duration>,
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
        Self::new_with_optional_read_timeout(
            max_bytes,
            Some(read_timeout),
            queue_timeout,
            budget_bytes,
            budget,
        )
    }

    pub fn new_with_optional_read_timeout(
        max_bytes: u64,
        read_timeout: Option<Duration>,
        queue_timeout: Duration,
        budget_bytes: usize,
        budget: Arc<Semaphore>,
    ) -> Self {
        Self::with_optional_read_timeout_and_permit_bytes(
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
        Self::with_optional_read_timeout_and_permit_bytes(
            max_bytes,
            Some(read_timeout),
            queue_timeout,
            budget_bytes,
            permit_bytes,
            budget,
        )
    }

    pub fn with_optional_read_timeout_and_permit_bytes(
        max_bytes: u64,
        read_timeout: Option<Duration>,
        queue_timeout: Duration,
        budget_bytes: usize,
        permit_bytes: usize,
        budget: Arc<Semaphore>,
    ) -> Self {
        Self {
            max_bytes,
            read_timeout: read_timeout.filter(|timeout| !timeout.is_zero()),
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
        self.read_timeout.unwrap_or_default()
    }

    pub fn optional_read_timeout(&self) -> Option<Duration> {
        self.read_timeout
    }

    pub fn queue_timeout(&self) -> Duration {
        self.queue_timeout
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    /// The body buffer budget is also a hard per-request ceiling.  A request
    /// whose configured body limit is larger than the shared budget must not
    /// be allowed to reserve only the budget and then collect/decompress past
    /// it, otherwise chunked and compressed requests can defeat the memory
    /// bound.
    pub fn effective_max_bytes(&self) -> u64 {
        self.max_bytes
            .min(u64::try_from(self.budget_bytes).unwrap_or(u64::MAX))
    }

    pub fn reservation_bytes(&self, headers: &HeaderMap) -> usize {
        reservation_bytes(
            headers,
            self.max_bytes,
            self.budget_bytes,
            self.permit_bytes,
        )
    }

    pub fn reservation_permits(&self, reservation_bytes: usize) -> u32 {
        reservation_permits(reservation_bytes, self.permit_bytes)
    }

    pub async fn reserve(
        &self,
        headers: &HeaderMap,
    ) -> Result<BodyBufferReservation, BodyBufferError> {
        let effective_max_bytes = self.effective_max_bytes();
        let declared_content_length = validate_request_body_headers(headers)?;
        if let Some(declared) = declared_content_length {
            if declared > effective_max_bytes {
                return Err(BodyBufferError::TooLarge {
                    limit_bytes: effective_max_bytes,
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
            memory: BodyBufferBudget {
                permit,
                budget: Arc::clone(&self.budget),
                budget_bytes: self.budget_bytes,
                permit_bytes: self.permit_bytes,
                requested_bytes,
            },
            max_bytes: effective_max_bytes,
            read_timeout: self.read_timeout,
        })
    }
}

#[derive(Debug)]
pub struct BodyBufferReservation {
    memory: BodyBufferBudget,
    max_bytes: u64,
    read_timeout: Option<Duration>,
}

#[derive(Debug)]
pub struct BodyBufferBudget {
    permit: OwnedSemaphorePermit,
    budget: Arc<Semaphore>,
    budget_bytes: usize,
    permit_bytes: usize,
    requested_bytes: usize,
}

impl BodyBufferBudget {
    /// Reserve a new high-water mark before retaining or decoding more bytes.
    /// Never queue for growth while another partial request may hold the rest.
    pub fn try_reserve_bytes(&mut self, requested_bytes: usize) -> Result<(), BodyBufferError> {
        if requested_bytes > self.budget_bytes {
            return Err(self.overloaded(requested_bytes));
        }
        let permits = reservation_permits(requested_bytes, self.permit_bytes) as usize;
        let additional = permits.saturating_sub(self.permit.num_permits());
        if additional > 0 {
            let permit = Arc::clone(&self.budget)
                .try_acquire_many_owned(additional as u32)
                .map_err(|_| self.overloaded(requested_bytes))?;
            self.permit.merge(permit);
        }
        self.requested_bytes = self.requested_bytes.max(requested_bytes);
        Ok(())
    }

    fn overloaded(&self, requested_bytes: usize) -> BodyBufferError {
        BodyBufferError::Overloaded {
            requested_bytes,
            budget_bytes: self.budget_bytes,
            timeout_ms: 0,
        }
    }
}

impl BodyBufferReservation {
    pub fn requested_bytes(&self) -> usize {
        self.memory.requested_bytes
    }

    pub async fn collect(self, body: Body) -> Result<BufferedBody, BodyBufferError> {
        let Self {
            mut memory,
            max_bytes,
            read_timeout,
        } = self;
        let started_at = Instant::now();
        let collect = async {
            let mut stream = body.into_data_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| BodyBufferError::ReadFailed {
                    message: error.to_string(),
                })?;
                let length =
                    bytes
                        .len()
                        .checked_add(chunk.len())
                        .ok_or(BodyBufferError::TooLarge {
                            limit_bytes: max_bytes,
                        })?;
                if length as u64 > max_bytes {
                    return Err(BodyBufferError::TooLarge {
                        limit_bytes: max_bytes,
                    });
                }
                if length > bytes.capacity() {
                    let mut capacity = if length <= DEFAULT_BODY_BUFFER_PERMIT_BYTES {
                        length
                    } else {
                        bytes
                            .capacity()
                            .saturating_mul(2)
                            .max(length)
                            .min(usize::try_from(max_bytes).unwrap_or(usize::MAX))
                    };
                    if memory.try_reserve_bytes(capacity).is_err() {
                        memory.try_reserve_bytes(length)?;
                        capacity = length;
                    }
                    // Account for geometric growth, falling back to the bytes needed under load.
                    bytes
                        .try_reserve_exact(capacity - bytes.len())
                        .map_err(|error| BodyBufferError::ReadFailed {
                            message: error.to_string(),
                        })?;
                    if bytes.capacity() > capacity {
                        memory.try_reserve_bytes(bytes.capacity())?;
                    }
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Bytes::from(bytes))
        };
        let bytes = match read_timeout {
            Some(read_timeout) => tokio::time::timeout(read_timeout, collect)
                .await
                .map_err(|_| BodyBufferError::Timeout {
                    timeout_ms: duration_millis(read_timeout),
                }),
            None => Ok(collect.await),
        }??;

        Ok(BufferedBody {
            bytes,
            memory,
            elapsed: started_at.elapsed(),
        })
    }
}

#[derive(Debug)]
pub struct BufferedBody {
    bytes: Bytes,
    memory: BodyBufferBudget,
    elapsed: Duration,
}

impl BufferedBody {
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }

    pub fn requested_bytes(&self) -> usize {
        self.memory.requested_bytes
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Retain the permit for a callback that does not grow the buffered payload.
    pub fn try_map<T, E>(self, map: impl FnOnce(Bytes) -> Result<T, E>) -> Result<T, E> {
        self.try_map_with_budget(|bytes, _| map(bytes))
    }

    /// The callback must account for decoded buffers before allocating them.
    pub fn try_map_with_budget<T, E>(
        self,
        map: impl FnOnce(Bytes, &mut BodyBufferBudget) -> Result<T, E>,
    ) -> Result<T, E> {
        let Self {
            bytes, mut memory, ..
        } = self;
        let result = map(bytes, &mut memory);
        drop(memory);
        result
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BodyBufferError {
    InvalidHeaders {
        message: String,
    },
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
            Self::InvalidHeaders { .. } => StatusCode::BAD_REQUEST,
            Self::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Overloaded { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout { .. } => StatusCode::REQUEST_TIMEOUT,
            Self::ReadFailed { .. } => StatusCode::BAD_REQUEST,
        }
    }

    pub fn client_message(&self) -> String {
        match self {
            Self::InvalidHeaders { .. } => "Invalid request body headers".to_string(),
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
            Self::InvalidHeaders { .. } => "invalid_request_body_headers",
            Self::TooLarge { .. } => "request_body_too_large",
            Self::Overloaded { .. } => "request_body_buffer_overloaded",
            Self::Timeout { .. } => "request_body_read_timeout",
            Self::ReadFailed { .. } => "request_body_read_failed",
        }
    }
}

fn validate_request_body_headers(headers: &HeaderMap) -> Result<Option<u64>, BodyBufferError> {
    let declared_content_length = declared_content_length(headers)?;
    if declared_content_length.is_some() && headers.contains_key(header::TRANSFER_ENCODING) {
        return Err(invalid_body_headers(
            "content-length and transfer-encoding must not be combined",
        ));
    }
    validate_content_encoding_headers(headers)?;
    Ok(declared_content_length)
}

fn declared_content_length(headers: &HeaderMap) -> Result<Option<u64>, BodyBufferError> {
    let mut values = headers.get_all(header::CONTENT_LENGTH).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(invalid_body_headers("duplicate content-length header"));
    }
    let value = value
        .to_str()
        .map_err(|_| invalid_body_headers("invalid content-length header"))?
        .trim();
    if value.is_empty() || value.contains(',') {
        return Err(invalid_body_headers("ambiguous content-length header"));
    }
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| invalid_body_headers("invalid content-length header"))
}

fn validate_content_encoding_headers(headers: &HeaderMap) -> Result<(), BodyBufferError> {
    if headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .nth(1)
        .is_some()
    {
        return Err(invalid_body_headers("duplicate content-encoding header"));
    }
    let mut count = 0usize;
    for value in headers.get_all(header::CONTENT_ENCODING).iter() {
        let value = value
            .to_str()
            .map_err(|_| invalid_body_headers("invalid content-encoding header"))?;
        for encoding in value.split(',') {
            if encoding.trim().is_empty() {
                return Err(invalid_body_headers("invalid content-encoding header"));
            }
            count = count.saturating_add(1);
            if count > MAX_REQUEST_CONTENT_ENCODINGS {
                return Err(invalid_body_headers(
                    "content-encoding chain exceeds the supported limit",
                ));
            }
        }
    }
    Ok(())
}

fn invalid_body_headers(message: &str) -> BodyBufferError {
    BodyBufferError::InvalidHeaders {
        message: message.to_string(),
    }
}

fn reservation_bytes(
    headers: &HeaderMap,
    max_bytes: u64,
    budget_bytes: usize,
    permit_bytes: usize,
) -> usize {
    let reservation_ceiling = usize::try_from(max_bytes)
        .unwrap_or(usize::MAX)
        .min(budget_bytes);
    declared_content_length(headers)
        .ok()
        .flatten()
        .map(|value| {
            usize::try_from(value)
                .unwrap_or(usize::MAX)
                .min(reservation_ceiling)
        })
        .unwrap_or_else(|| permit_bytes.min(reservation_ceiling))
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
    async fn rejects_ambiguous_content_length_headers_before_reading_body() {
        let mut headers = HeaderMap::new();
        headers.append(header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        headers.append(header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        let error = policy(10, Duration::from_secs(1), Arc::new(Semaphore::new(1)))
            .reserve(&headers)
            .await
            .expect_err("duplicate content-length must be rejected");
        assert!(matches!(error, BodyBufferError::InvalidHeaders { .. }));
    }

    #[tokio::test]
    async fn rejects_content_length_with_transfer_encoding_before_reading_body() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        let error = policy(10, Duration::from_secs(1), Arc::new(Semaphore::new(1)))
            .reserve(&headers)
            .await
            .expect_err("content-length plus transfer-encoding must be rejected");
        assert!(matches!(error, BodyBufferError::InvalidHeaders { .. }));
    }

    #[tokio::test]
    async fn rejects_overlong_content_encoding_chain_before_reading_body() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip, gzip, gzip, gzip, gzip, gzip, gzip, gzip, gzip"),
        );
        let error = policy(10, Duration::from_secs(1), Arc::new(Semaphore::new(1)))
            .reserve(&headers)
            .await
            .expect_err("overlong content-encoding chain must be rejected");
        assert!(matches!(error, BodyBufferError::InvalidHeaders { .. }));
    }

    #[tokio::test]
    async fn rejects_duplicate_content_encoding_fields_before_reading_body() {
        let mut headers = HeaderMap::new();
        headers.append(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        headers.append(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        let error = policy(10, Duration::from_secs(1), Arc::new(Semaphore::new(1)))
            .reserve(&headers)
            .await
            .expect_err("duplicate content-encoding fields must be rejected");
        assert!(matches!(error, BodyBufferError::InvalidHeaders { .. }));
    }

    #[tokio::test]
    async fn body_limit_larger_than_budget_is_capped_before_reading() {
        let budget = Arc::new(Semaphore::new(5));
        let policy = BodyBufferPolicy::with_permit_bytes(
            10,
            Duration::from_secs(1),
            Duration::from_secs(1),
            5,
            1,
            Arc::clone(&budget),
        );
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("10"));

        let error = policy
            .reserve(&headers)
            .await
            .expect_err("declared body above the shared budget must be rejected");
        assert_eq!(error, BodyBufferError::TooLarge { limit_bytes: 5 });
        assert_eq!(budget.available_permits(), 5);

        let mut small_headers = HeaderMap::new();
        small_headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("5"));
        let reservation = policy
            .reserve(&small_headers)
            .await
            .expect("body within the shared budget should remain accepted");
        assert_eq!(reservation.requested_bytes(), 5);
        let buffered = reservation
            .collect(Body::from(Bytes::from_static(b"01234")))
            .await
            .expect("body within the shared budget should collect");
        assert_eq!(buffered.bytes().as_ref(), b"01234");
        drop(buffered);
        assert_eq!(budget.available_permits(), 5);
    }

    #[tokio::test]
    async fn unlimited_body_is_still_bounded_by_budget() {
        let budget = Arc::new(Semaphore::new(4));
        let policy = BodyBufferPolicy::with_permit_bytes(
            u64::MAX,
            Duration::from_secs(1),
            Duration::from_secs(1),
            4,
            1,
            Arc::clone(&budget),
        );
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let reservation = policy
            .reserve(&headers)
            .await
            .expect("encoded unlimited body should reserve its initial chunk");
        assert_eq!(reservation.requested_bytes(), 1);
        assert_eq!(budget.available_permits(), 3);

        let error = reservation
            .collect(Body::from(Bytes::from_static(b"01234")))
            .await
            .expect_err("unlimited body must still be bounded by the shared budget");
        assert_eq!(error, BodyBufferError::TooLarge { limit_bytes: 4 });
        assert_eq!(budget.available_permits(), 4);

        let reservation = policy
            .reserve(&HeaderMap::new())
            .await
            .expect("a body at the budget boundary should reserve");
        let buffered = reservation
            .collect(Body::from(Bytes::from_static(b"0123")))
            .await
            .expect("a body at the budget boundary should collect");
        assert_eq!(buffered.bytes().as_ref(), b"0123");
        drop(buffered);
        assert_eq!(budget.available_permits(), 4);
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
    async fn disabled_read_timeout_allows_slow_body_to_complete() {
        let stream = stream::once(async { Ok::<Bytes, std::io::Error>(Bytes::from_static(b"{")) })
            .chain(stream::once(async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok::<Bytes, std::io::Error>(Bytes::from_static(b"}"))
            }));
        let policy = BodyBufferPolicy::with_optional_read_timeout_and_permit_bytes(
            1024,
            None,
            Duration::from_secs(1),
            1024,
            DEFAULT_BODY_BUFFER_PERMIT_BYTES,
            Arc::new(Semaphore::new(1)),
        );
        let reservation = policy
            .reserve(&HeaderMap::new())
            .await
            .expect("reservation should succeed");
        let buffered = tokio::time::timeout(
            Duration::from_secs(1),
            reservation.collect(Body::from_stream(stream)),
        )
        .await
        .expect("test body should finish")
        .expect("disabled read timeout should allow a slow body");
        assert_eq!(buffered.bytes().as_ref(), b"{}");
    }

    #[test]
    fn zero_read_timeout_is_normalized_to_disabled() {
        let policy = BodyBufferPolicy::new_with_optional_read_timeout(
            1024,
            Some(Duration::ZERO),
            Duration::from_secs(1),
            1024,
            Arc::new(Semaphore::new(1)),
        );
        assert_eq!(policy.optional_read_timeout(), None);
        assert_eq!(policy.read_timeout(), Duration::ZERO);
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

    #[tokio::test]
    async fn small_compressed_and_unknown_length_requests_share_the_budget() {
        let budget_bytes = 256 * 1024 * 1024;
        let budget = Arc::new(Semaphore::new(
            budget_bytes / DEFAULT_BODY_BUFFER_PERMIT_BYTES,
        ));
        let policy = policy(
            budget_bytes as u64,
            Duration::from_secs(1),
            Arc::clone(&budget),
        );
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("1024"));
        let compressed = policy.reserve(&headers).await.unwrap();
        let unknown = policy.reserve(&HeaderMap::new()).await.unwrap();
        assert_eq!(compressed.requested_bytes(), 1024);
        assert_eq!(unknown.requested_bytes(), DEFAULT_BODY_BUFFER_PERMIT_BYTES);
        assert_eq!(budget.available_permits(), 4094);
        drop((compressed, unknown));
        assert_eq!(budget.available_permits(), 4096);
    }

    #[tokio::test]
    async fn unknown_length_body_grows_its_reservation_and_holds_it_until_normalized() {
        let budget = Arc::new(Semaphore::new(8));
        let policy = BodyBufferPolicy::with_permit_bytes(
            8,
            Duration::from_secs(1),
            Duration::from_secs(1),
            8,
            1,
            Arc::clone(&budget),
        );
        let reservation = policy.reserve(&HeaderMap::new()).await.unwrap();
        assert_eq!(budget.available_permits(), 7);
        let body = Body::from_stream(stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"ab")),
            Ok(Bytes::from_static(b"cd")),
        ]));
        let buffered = reservation.collect(body).await.unwrap();
        assert_eq!(buffered.bytes().as_ref(), b"abcd");
        assert_eq!(budget.available_permits(), 4);
        buffered
            .try_map_with_budget(|_, memory| {
                memory.try_reserve_bytes(7)?;
                assert_eq!(budget.available_permits(), 1);
                Ok::<_, BodyBufferError>(())
            })
            .unwrap();
        assert_eq!(budget.available_permits(), 8);
    }

    #[tokio::test]
    async fn partial_upload_growth_rejects_without_waiting_and_releases_its_budget() {
        let budget = Arc::new(Semaphore::new(2));
        let policy = BodyBufferPolicy::with_permit_bytes(
            2,
            Duration::from_secs(60),
            Duration::from_secs(60),
            2,
            1,
            Arc::clone(&budget),
        );
        let first = policy.reserve(&HeaderMap::new()).await.unwrap();
        let second = policy.reserve(&HeaderMap::new()).await.unwrap();
        let error =
            tokio::time::timeout(Duration::from_millis(100), first.collect(Body::from("ab")))
                .await
                .expect("growth must not wait while holding a partial reservation")
                .unwrap_err();
        assert_eq!(
            error,
            BodyBufferError::Overloaded {
                requested_bytes: 2,
                budget_bytes: 2,
                timeout_ms: 0,
            }
        );
        assert_eq!(budget.available_permits(), 1);
        let buffered = second.collect(Body::from("ab")).await.unwrap();
        assert_eq!(budget.available_permits(), 0);
        drop(buffered);
        assert_eq!(budget.available_permits(), 2);
    }

    #[tokio::test]
    async fn normalization_budget_failure_releases_all_upload_permits() {
        let budget = Arc::new(Semaphore::new(4));
        let policy = BodyBufferPolicy::with_permit_bytes(
            4,
            Duration::from_secs(1),
            Duration::from_secs(1),
            4,
            1,
            Arc::clone(&budget),
        );
        let buffered = policy
            .reserve(&HeaderMap::new())
            .await
            .unwrap()
            .collect(Body::from("ab"))
            .await
            .unwrap();
        let result = buffered.try_map_with_budget(|_, memory| memory.try_reserve_bytes(5));
        assert!(matches!(
            result,
            Err(BodyBufferError::Overloaded {
                requested_bytes: 5,
                ..
            })
        ));
        assert_eq!(budget.available_permits(), 4);
    }

    #[tokio::test]
    async fn upload_growth_uses_available_budget_without_requiring_geometric_headroom() {
        let budget = Arc::new(Semaphore::new(128));
        let held = Arc::clone(&budget).acquire_many_owned(32).await.unwrap();
        let policy = BodyBufferPolicy::with_permit_bytes(
            128 * 1024,
            Duration::from_secs(1),
            Duration::from_secs(1),
            128 * 1024,
            1024,
            Arc::clone(&budget),
        );
        let body = Body::from_stream(stream::iter([
            Ok::<_, std::io::Error>(Bytes::from(vec![b'a'; 70_000])),
            Ok(Bytes::from(vec![b'b'; 20_000])),
        ]));
        let buffered = policy
            .reserve(&HeaderMap::new())
            .await
            .unwrap()
            .collect(body)
            .await
            .unwrap();
        assert_eq!(buffered.bytes().len(), 90_000);
        assert_eq!(buffered.requested_bytes(), 90_000);
        drop((buffered, held));
        assert_eq!(budget.available_permits(), 128);
    }

    #[tokio::test]
    async fn cancellation_releases_partial_upload_budget() {
        let budget = Arc::new(Semaphore::new(4));
        let policy = BodyBufferPolicy::with_permit_bytes(
            4,
            Duration::from_secs(60),
            Duration::from_secs(1),
            4,
            1,
            Arc::clone(&budget),
        );
        let reservation = policy.reserve(&HeaderMap::new()).await.unwrap();
        let body = Body::from_stream(
            stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"ab")) })
                .chain(stream::pending()),
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), reservation.collect(body))
                .await
                .is_err()
        );
        assert_eq!(budget.available_permits(), 4);
    }
}
