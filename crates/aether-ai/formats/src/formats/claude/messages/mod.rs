pub mod chat_spec;
pub mod cli_spec;
pub mod request;
pub mod response;
pub mod spec;
pub mod stream;

use crate::formats::shared::family::LocalStandardSpec;

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalStandardSpec> {
    chat_spec::resolve_sync_spec(plan_kind).or_else(|| cli_spec::resolve_sync_spec(plan_kind))
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalStandardSpec> {
    chat_spec::resolve_stream_spec(plan_kind).or_else(|| cli_spec::resolve_stream_spec(plan_kind))
}
