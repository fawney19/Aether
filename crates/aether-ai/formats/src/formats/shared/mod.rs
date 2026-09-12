use base64::Engine as _;
use std::fmt;

/// Report bodies are produced from execution responses, whose normal decoded
/// transport limit is 64 MiB.  Keep format finalizers on the same boundary so
/// a base64 field cannot trigger an unchecked allocation before parsing.
pub(crate) const MAX_SYNC_REPORT_BODY_BYTES: usize = 64 * 1024 * 1024;

pub mod error_body;
pub mod family;
pub mod image_bridge;
pub mod model_directives;
pub mod passthrough;
pub mod request;
pub mod request_matrix;
pub mod response;
pub mod routing;
pub mod sse;
pub mod standard_matrix;
pub mod standard_normalize;
pub mod stream_core;
pub mod stream_rewrite;
pub mod sync_products;
pub mod sync_to_stream;
pub mod video;

pub(crate) fn decode_sync_report_body_base64(
    body_base64: &str,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    if body_base64.is_empty() {
        return Ok(Vec::new());
    }

    let max_encoded_len = MAX_SYNC_REPORT_BODY_BYTES
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .unwrap_or(usize::MAX);
    if body_base64.len() > max_encoded_len {
        return Err(AiSurfaceFinalizeError::new(format!(
            "sync report body exceeds {} decoded bytes",
            MAX_SYNC_REPORT_BODY_BYTES
        )));
    }

    let bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
    if bytes.len() > MAX_SYNC_REPORT_BODY_BYTES {
        return Err(AiSurfaceFinalizeError::new(format!(
            "sync report body exceeds {} decoded bytes",
            MAX_SYNC_REPORT_BODY_BYTES
        )));
    }
    Ok(bytes)
}

pub use self::sse::{encode_done_sse, encode_json_sse, map_claude_stop_reason};
pub use self::stream_core::{CanonicalStreamEvent, CanonicalStreamFrame};
pub use self::stream_rewrite::{
    maybe_build_ai_surface_stream_rewriter, resolve_finalize_stream_rewrite_mode,
    AiSurfaceStreamRewriter, FinalizeStreamRewriteMode,
};

#[derive(Debug)]
pub struct AiSurfaceFinalizeError(pub String);

impl AiSurfaceFinalizeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for AiSurfaceFinalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AI surface finalize error: {}", self.0)
    }
}

impl std::error::Error for AiSurfaceFinalizeError {}

impl From<serde_json::Error> for AiSurfaceFinalizeError {
    fn from(source: serde_json::Error) -> Self {
        Self(source.to_string())
    }
}

impl From<base64::DecodeError> for AiSurfaceFinalizeError {
    fn from(source: base64::DecodeError) -> Self {
        Self(source.to_string())
    }
}
