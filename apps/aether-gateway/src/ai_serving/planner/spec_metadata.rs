use crate::ai_serving::planner::plan_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
    AiStreamAttempt, AiSyncAttempt,
};
use crate::ai_serving::AiExecutionDecision;
use crate::GatewayError;

pub(crate) use aether_ai_serving::{
    ai_gemini_files_spec_metadata as local_gemini_files_spec_metadata,
    ai_openai_image_spec_metadata as local_openai_image_spec_metadata,
    ai_openai_responses_spec_metadata as local_openai_responses_spec_metadata,
    ai_requested_model_family_for_same_format_provider as requested_model_family_for_same_format_provider,
    ai_requested_model_family_for_standard_source as requested_model_family_for_standard_source,
    ai_requested_model_family_for_video_create as requested_model_family_for_video_create,
    ai_same_format_provider_spec_metadata as local_same_format_provider_spec_metadata,
    ai_standard_spec_metadata as local_standard_spec_metadata,
    ai_video_create_spec_metadata as local_video_create_spec_metadata,
    AiExecutionSurfaceSpecMetadata as LocalExecutionSurfaceSpecMetadata,
    AiRequestedModelFamily as RequestedModelFamily,
};

pub(crate) fn build_sync_plan_from_requested_model_family(
    family: RequestedModelFamily,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    payload: AiExecutionDecision,
) -> Result<Option<AiSyncAttempt>, GatewayError> {
    match family {
        RequestedModelFamily::Standard => {
            build_standard_sync_plan_from_decision(parts, body_json, payload)
        }
        RequestedModelFamily::Gemini => {
            build_gemini_sync_plan_from_decision(parts, body_json, payload)
        }
    }
}

pub(crate) fn build_stream_plan_from_requested_model_family(
    family: RequestedModelFamily,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    payload: AiExecutionDecision,
) -> Result<Option<AiStreamAttempt>, GatewayError> {
    match family {
        RequestedModelFamily::Standard => {
            build_standard_stream_plan_from_decision(parts, body_json, payload, false)
        }
        RequestedModelFamily::Gemini => {
            build_gemini_stream_plan_from_decision(parts, body_json, payload)
        }
    }
}
