//! Gateway control-plane contracts and request metadata.
//!
//! Concrete authentication, persistence, and handlers remain adapters. This
//! crate owns the request context passed between those adapters.

pub mod public;

pub use public::PublicRequestContext;
