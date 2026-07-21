//! Compatibility facade for execution stream framing.

use std::io::Error as IoError;

use aether_contracts::StreamFrame;
use axum::body::Bytes;

use crate::GatewayError;

pub(crate) fn encode_stream_frame_ndjson(frame: &StreamFrame) -> Result<Bytes, IoError> {
    aether_gateway_execution::stream::encode_stream_frame_ndjson(frame)
}

pub(crate) fn decode_stream_frame_ndjson(line: &[u8]) -> Result<StreamFrame, GatewayError> {
    aether_gateway_execution::stream::decode_stream_frame_ndjson(line)
        .map_err(|err| GatewayError::Internal(err.to_string()))
}
