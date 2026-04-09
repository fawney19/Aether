use axum::body::Body;
use axum::http::Response;
use serde_json::Value;

use crate::ai_pipeline::GatewayControlDecision;
use crate::{usage::GatewaySyncReportRequest, GatewayError};

#[path = "stream_rewrite.rs"]
pub(crate) mod stream;
#[path = "sync_finalize.rs"]
pub(crate) mod sync;

pub(crate) use stream::LocalStreamRewriter;
pub(crate) use sync::LocalCoreSyncFinalizeOutcome;

pub(crate) fn maybe_build_sync_finalize_outcome(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Option<LocalCoreSyncFinalizeOutcome>, GatewayError> {
    sync::maybe_build_local_core_sync_finalize_response(trace_id, decision, payload)
}

pub(crate) fn maybe_compile_sync_finalize_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: &GatewaySyncReportRequest,
) -> Result<Option<Response<Body>>, GatewayError> {
    Ok(
        maybe_build_sync_finalize_outcome(trace_id, decision, payload)?
            .map(|outcome| outcome.response),
    )
}

pub(crate) fn maybe_build_stream_response_rewriter(
    report_context: Option<&Value>,
) -> Option<LocalStreamRewriter> {
    stream::maybe_build_local_stream_rewriter(report_context)
}
