//! Requests that can stay in the same public/provider contract family.

mod provider;

pub(crate) use self::provider::{
    build_local_stream_plan_and_reports as build_local_same_format_stream_plan_and_reports,
    build_local_sync_plan_and_reports as build_local_same_format_sync_plan_and_reports,
    maybe_build_local_same_format_provider_decision_payload_for_candidate,
    maybe_build_stream_local_same_format_provider_decision_payload,
    maybe_build_sync_local_same_format_provider_decision_payload,
};
pub(crate) use crate::ai_serving::transport::provider_types::provider_type_supports_local_same_format_transport;
