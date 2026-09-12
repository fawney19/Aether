use std::error::Error;
use std::fmt;

pub enum ResponseBodyReadError {
    TooLarge { max_bytes: usize },
    Read(reqwest::Error),
}

// Avoid trusting a remote Content-Length as an allocation hint.  The stream
// remains allowed to grow up to the caller's actual body limit, but the first
// allocation stays modest when a peer advertises a very large response.
const MAX_INITIAL_RESPONSE_BODY_CAPACITY_BYTES: usize = 16 * 1024 * 1024;

impl fmt::Debug for ResponseBodyReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ResponseBodyReadError");
        match self {
            Self::TooLarge { max_bytes } => {
                debug
                    .field("kind", &"too_large")
                    .field("max_bytes", max_bytes);
            }
            // Reqwest's Debug output may include the complete request URL,
            // including credentials embedded in a path or query. Keep the
            // underlying value available to explicit category helpers, but
            // never render it through this public error boundary.
            Self::Read(_) => {
                debug.field("kind", &"read");
            }
        }
        debug.finish()
    }
}

impl fmt::Display for ResponseBodyReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { max_bytes } => {
                write!(formatter, "response body exceeds {max_bytes} bytes")
            }
            Self::Read(_) => write!(formatter, "failed to read response body"),
        }
    }
}

impl Error for ResponseBodyReadError {
    // Do not expose the reqwest error chain to generic reporters. Callers that
    // need a retry/telemetry category can still pattern-match `Read(error)`
    // and inspect the concrete reqwest value deliberately.
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Read a small control-plane response without trusting `Content-Length`.
///
/// The advertised length is rejected early when available, while the streamed
/// byte count remains authoritative for missing or dishonest length headers.
pub async fn read_response_bytes_with_limit(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ResponseBodyReadError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(ResponseBodyReadError::TooLarge { max_bytes });
    }

    let initial_capacity = initial_response_body_capacity(response.content_length(), max_bytes);
    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(ResponseBodyReadError::Read)?
    {
        append_chunk_with_limit(&mut body, &chunk, max_bytes)?;
    }
    Ok(body)
}

fn initial_response_body_capacity(content_length: Option<u64>, max_bytes: usize) -> usize {
    content_length
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(max_bytes)
        .min(MAX_INITIAL_RESPONSE_BODY_CAPACITY_BYTES)
}

fn append_chunk_with_limit(
    body: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
) -> Result<(), ResponseBodyReadError> {
    let Some(next_len) = body.len().checked_add(chunk.len()) else {
        return Err(ResponseBodyReadError::TooLarge { max_bytes });
    };
    if next_len > max_bytes {
        return Err(ResponseBodyReadError::TooLarge { max_bytes });
    }
    body.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{append_chunk_with_limit, initial_response_body_capacity, ResponseBodyReadError};
    use std::error::Error;

    #[test]
    fn streamed_body_accepts_exact_limit() {
        let mut body = b"1234".to_vec();
        append_chunk_with_limit(&mut body, b"5678", 8).expect("exact limit should pass");
        assert_eq!(body, b"12345678");
    }

    #[test]
    fn streamed_body_rejects_limit_plus_one_without_appending_chunk() {
        let mut body = b"1234".to_vec();
        let error = append_chunk_with_limit(&mut body, b"56789", 8)
            .expect_err("limit plus one should fail");
        assert!(matches!(
            error,
            ResponseBodyReadError::TooLarge { max_bytes: 8 }
        ));
        assert_eq!(body, b"1234");
    }

    #[test]
    fn public_error_rendering_does_not_include_read_error_details() {
        let error = ResponseBodyReadError::TooLarge { max_bytes: 64 };
        assert_eq!(error.to_string(), "response body exceeds 64 bytes");
        assert!(error.source().is_none());
        assert!(format!("{error:?}").contains("too_large"));
    }

    #[test]
    fn initial_capacity_does_not_trust_giant_content_length() {
        assert_eq!(
            initial_response_body_capacity(Some(u64::MAX), usize::MAX),
            16 * 1024 * 1024
        );
        assert_eq!(initial_response_body_capacity(Some(1024), usize::MAX), 1024);
        assert_eq!(initial_response_body_capacity(None, usize::MAX), 0);
    }
}
