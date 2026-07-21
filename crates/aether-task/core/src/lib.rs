//! Runtime-independent task contracts.

mod definition;
mod fencing;
mod lease;
mod retry;

pub use definition::{TaskDefinition, TaskKind};
pub use fencing::FencingToken;
pub use lease::{TaskLease, TaskLeaseError};
pub use retry::RetryPolicy;
