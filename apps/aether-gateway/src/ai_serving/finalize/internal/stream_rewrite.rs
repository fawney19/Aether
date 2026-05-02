use serde_json::Value;

use crate::ai_serving::{
    maybe_build_ai_surface_stream_rewriter, AiSurfaceFinalizeError, AiSurfaceStreamRewriter,
};
use crate::GatewayError;

pub(crate) struct LocalStreamRewriter<'a> {
    inner: AiSurfaceStreamRewriter<'a>,
}

pub(crate) fn maybe_build_local_stream_rewriter<'a>(
    report_context: Option<&'a Value>,
) -> Option<LocalStreamRewriter<'a>> {
    maybe_build_ai_surface_stream_rewriter(report_context)
        .map(|inner| LocalStreamRewriter { inner })
}

impl LocalStreamRewriter<'_> {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, GatewayError> {
        self.inner.push_chunk(chunk).map_err(map_surface_error)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<u8>, GatewayError> {
        self.inner.finish().map_err(map_surface_error)
    }
}

fn map_surface_error(error: AiSurfaceFinalizeError) -> GatewayError {
    error.into()
}

#[cfg(test)]
#[path = "../tests_stream.rs"]
mod tests;
