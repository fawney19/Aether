//! Compatibility facade for execution resource limits.

pub(crate) use aether_gateway_execution::{
    MAX_ERROR_BODY_BYTES, MAX_STREAM_PREFETCH_BYTES, MAX_STREAM_PREFETCH_FRAMES,
};

// Usage/audit captures are secondary copies of the stream.  Keep a hard
// ceiling even when the configurable "full" record level is otherwise
// unbounded; this does not limit bytes forwarded to the client.
pub(crate) const MAX_STREAM_BODY_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

// Stream frames are newline-delimited JSON. Binary response chunks are base64
// encoded before framing, so this must be larger than the normal 64 MiB raw
// response limit while still bounding an attacker-controlled unterminated line.
pub(crate) const MAX_EXECUTION_STREAM_FRAME_LINE_BYTES: usize = 128 * 1024 * 1024;
