use serde_json::Value;

use crate::ai_pipeline::{
    encode_done_sse, encode_json_sse as encode_json_sse_impl, map_claude_stop_reason,
    PipelineFinalizeError,
};
use crate::GatewayError;

fn map_error(err: PipelineFinalizeError) -> GatewayError {
    err.into()
}

pub(crate) fn encode_json_sse(event: Option<&str>, value: &Value) -> Result<Vec<u8>, GatewayError> {
    encode_json_sse_impl(event, value).map_err(map_error)
}
