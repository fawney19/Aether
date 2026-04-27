//! Legacy pairwise conversion exports.
//!
//! Primary routing lives under `crate::formats::<wire_format>` and must pass
//! through canonical IR. This module remains so older pipeline/gateway call
//! sites and focused golden tests can keep their existing function names while
//! the cleanup proceeds.

pub mod request;
pub mod response;
