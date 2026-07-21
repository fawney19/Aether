//! Empty-database bootstrap workflows.
//!
//! Bootstrap is separate from migration execution: it prepares a fresh database
//! from a squashed snapshot, then stamps the migrations covered by that
//! snapshot so normal migration runners only apply later changes.

pub(crate) mod postgres;
