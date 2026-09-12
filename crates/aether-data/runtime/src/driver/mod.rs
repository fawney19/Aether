//! Compatibility paths for database adapter crates.
//!
//! Adapter code belongs in `aether-data-postgres`. These modules preserve existing `aether_data::driver`
//! imports while application-facing composition remains in `backend`.

#[cfg(feature = "postgres")]
pub mod postgres;
