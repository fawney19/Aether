//! Pairwise format conversion entry points.
//!
//! The public helpers in this module route through the registry, so request and
//! response conversion still pass through the typed canonical IR before a target
//! wire format is emitted.

pub mod request;
pub mod response;
