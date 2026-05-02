use std::fmt;

pub use self::sse::{encode_done_sse, encode_json_sse, map_claude_stop_reason};
pub use self::standard::stream_core::CanonicalStreamEvent;
pub use self::standard::stream_core::CanonicalStreamFrame;
pub use self::stream_rewrite::{
    maybe_build_ai_surface_stream_rewriter, resolve_finalize_stream_rewrite_mode,
    AiSurfaceStreamRewriter, FinalizeStreamRewriteMode,
};

pub mod common;
pub mod error_body;
pub mod openai_image_stream;
pub mod sse;
pub mod standard;
pub mod stream_rewrite;
pub mod sync_products;
pub mod sync_to_stream;

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
