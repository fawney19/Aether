//! Transport-independent admission contracts.
//!
//! This crate deliberately contains no Tokio, HTTP client, database, or Redis
//! dependency. Runtime adapters can implement the decisions without pulling
//! the gateway's infrastructure graph into domain crates.

mod budget;
mod metrics;
mod permit;
mod policy;

pub use budget::{AdmissionConfigError, DbClass, RedisLane, ResourceBudget, ResourceClass};
pub use metrics::AdmissionMetricsSnapshot;
pub use permit::{PermitKind, RequestPermitSet};
pub use policy::{
    AdmissionDecision, AdmissionPolicy, AdmissionRejectReason, AdmissionRequest,
    DefaultAdmissionPolicy,
};
