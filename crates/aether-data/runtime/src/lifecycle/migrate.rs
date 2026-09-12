//! Runtime database migration entry points.
//!
//! Each driver owns its migrator and startup preparation. The facade keeps
//! the established public entry points used by gateway bootstrap code.

#[cfg(feature = "postgres")]
mod postgres;
mod types;

#[cfg(all(test, feature = "postgres"))]
mod tests;

#[cfg(feature = "postgres")]
pub use postgres::{pending_migrations, prepare_database_for_startup, run_migrations};
pub use types::PendingMigrationInfo;
