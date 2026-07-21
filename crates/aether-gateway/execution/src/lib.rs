//! Gateway execution request-lifecycle components.
//!
//! This crate starts with transport-independent stream framing and bounded
//! execution limits. Planner, executor, and response-finalization adapters can
//! migrate behind this boundary without making the binary their owner.

mod limits;
pub mod stream;

pub use limits::{MAX_ERROR_BODY_BYTES, MAX_STREAM_PREFETCH_BYTES, MAX_STREAM_PREFETCH_FRAMES};
