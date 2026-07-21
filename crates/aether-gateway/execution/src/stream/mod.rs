//! Execution stream protocol codecs.

mod ndjson;

pub use ndjson::{decode_stream_frame_ndjson, encode_stream_frame_ndjson};
