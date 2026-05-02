pub mod chat;
pub mod cli;

use crate::request::standard::LocalStandardSpec;

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalStandardSpec> {
    chat::resolve_sync_spec(plan_kind).or_else(|| cli::resolve_sync_spec(plan_kind))
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalStandardSpec> {
    chat::resolve_stream_spec(plan_kind).or_else(|| cli::resolve_stream_spec(plan_kind))
}
