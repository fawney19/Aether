use crate::ai_pipeline::planner::common::RequestedModelFamily;
use crate::ai_pipeline::planner::plan_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
    LocalStreamPlanAndReport, LocalSyncPlanAndReport,
};
use crate::ai_pipeline::{
    GatewayControlSyncDecisionResponse, LocalGeminiFilesSpec, LocalOpenAiCliSpec,
    LocalOpenAiImageSpec, LocalSameFormatProviderFamily, LocalSameFormatProviderSpec,
    LocalStandardSourceFamily, LocalStandardSpec, LocalVideoCreateFamily, LocalVideoCreateSpec,
};
use crate::GatewayError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalExecutionSurfaceSpecMetadata {
    pub(crate) api_format: &'static str,
    pub(crate) decision_kind: &'static str,
    pub(crate) report_kind: Option<&'static str>,
    pub(crate) require_streaming: bool,
    pub(crate) requested_model_family: Option<RequestedModelFamily>,
}

pub(crate) fn requested_model_family_for_standard_source(
    family: LocalStandardSourceFamily,
) -> RequestedModelFamily {
    match family {
        LocalStandardSourceFamily::Standard => RequestedModelFamily::Standard,
        LocalStandardSourceFamily::Gemini => RequestedModelFamily::Gemini,
    }
}

pub(crate) fn local_standard_spec_metadata(
    spec: LocalStandardSpec,
) -> LocalExecutionSurfaceSpecMetadata {
    LocalExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: Some(requested_model_family_for_standard_source(spec.family)),
    }
}

pub(crate) fn local_same_format_provider_spec_metadata(
    spec: LocalSameFormatProviderSpec,
) -> LocalExecutionSurfaceSpecMetadata {
    LocalExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: Some(requested_model_family_for_same_format_provider(spec.family)),
    }
}

pub(crate) fn local_openai_cli_spec_metadata(
    spec: LocalOpenAiCliSpec,
) -> LocalExecutionSurfaceSpecMetadata {
    LocalExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: None,
    }
}

pub(crate) fn local_gemini_files_spec_metadata(
    spec: LocalGeminiFilesSpec,
) -> LocalExecutionSurfaceSpecMetadata {
    LocalExecutionSurfaceSpecMetadata {
        api_format: "gemini:files",
        decision_kind: spec.decision_kind,
        report_kind: spec.report_kind,
        require_streaming: spec.require_streaming,
        requested_model_family: None,
    }
}

pub(crate) fn local_openai_image_spec_metadata(
    spec: LocalOpenAiImageSpec,
) -> LocalExecutionSurfaceSpecMetadata {
    LocalExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: Some(RequestedModelFamily::Standard),
    }
}

pub(crate) fn local_video_create_spec_metadata(
    spec: LocalVideoCreateSpec,
) -> LocalExecutionSurfaceSpecMetadata {
    LocalExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: false,
        requested_model_family: Some(requested_model_family_for_video_create(spec.family)),
    }
}

pub(crate) fn requested_model_family_for_same_format_provider(
    family: LocalSameFormatProviderFamily,
) -> RequestedModelFamily {
    match family {
        LocalSameFormatProviderFamily::Standard => RequestedModelFamily::Standard,
        LocalSameFormatProviderFamily::Gemini => RequestedModelFamily::Gemini,
    }
}

pub(crate) fn requested_model_family_for_video_create(
    family: LocalVideoCreateFamily,
) -> RequestedModelFamily {
    match family {
        LocalVideoCreateFamily::OpenAi => RequestedModelFamily::Standard,
        LocalVideoCreateFamily::Gemini => RequestedModelFamily::Gemini,
    }
}

pub(crate) fn build_sync_plan_from_requested_model_family(
    family: RequestedModelFamily,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    payload: GatewayControlSyncDecisionResponse,
) -> Result<Option<LocalSyncPlanAndReport>, GatewayError> {
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
    payload: GatewayControlSyncDecisionResponse,
) -> Result<Option<LocalStreamPlanAndReport>, GatewayError> {
    match family {
        RequestedModelFamily::Standard => {
            build_standard_stream_plan_from_decision(parts, body_json, payload, false)
        }
        RequestedModelFamily::Gemini => {
            build_gemini_stream_plan_from_decision(parts, body_json, payload)
        }
    }
}
