use std::fmt;

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
