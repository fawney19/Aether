pub(crate) mod candidate_loop;
mod orchestration;
mod outcome;
mod plan_fallback;
mod policy;
mod remote;
mod stream_path;
mod sync_path;

pub(crate) use crate::request_candidate_runtime::{
    persist_available_local_candidate, persist_skipped_local_candidate,
};
pub(crate) use candidate_loop::{
    execute_stream_plan_and_reports, execute_sync_plan_and_reports,
    mark_unused_local_candidate_items,
};
pub(crate) use orchestration::*;
pub(crate) use outcome::{
    beautify_local_execution_client_error_message, build_local_execution_exhaustion,
    build_local_execution_runtime_miss_context, record_failed_usage_for_exhausted_request,
    record_failed_usage_for_runtime_miss_request, LocalExecutionExhaustion,
    LocalExecutionRequestOutcome, LocalExecutionRuntimeMissContext,
};
pub(crate) use plan_fallback::{
    maybe_execute_stream_via_plan_fallback, maybe_execute_sync_via_plan_fallback,
};
pub(crate) use policy::{
    build_direct_plan_bypass_cache_key, mark_direct_plan_bypass,
    should_bypass_execution_runtime_decision, should_bypass_execution_runtime_plan,
    should_skip_direct_plan,
};
pub(crate) use remote::{
    maybe_execute_stream_via_remote_decision, maybe_execute_sync_via_remote_decision,
};
pub(crate) use stream_path::maybe_execute_via_stream_decision_path;
pub(crate) use sync_path::maybe_execute_via_sync_decision_path;
