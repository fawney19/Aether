use aether_ai_formats::api::{
    LocalGeminiFilesSpec, LocalOpenAiImageSpec, LocalOpenAiResponsesSpec,
    LocalSameFormatProviderFamily, LocalSameFormatProviderSpec, LocalStandardSourceFamily,
    LocalStandardSpec, LocalVideoCreateFamily, LocalVideoCreateSpec,
};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiRequestedModelFamily {
    Standard,
    Gemini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiExecutionSurfaceSpecMetadata {
    pub api_format: &'static str,
    pub decision_kind: &'static str,
    pub report_kind: Option<&'static str>,
    pub require_streaming: bool,
    pub requested_model_family: Option<AiRequestedModelFamily>,
}

pub const fn ai_requested_model_family_for_standard_source(
    family: LocalStandardSourceFamily,
) -> AiRequestedModelFamily {
    match family {
        LocalStandardSourceFamily::Standard => AiRequestedModelFamily::Standard,
        LocalStandardSourceFamily::Gemini => AiRequestedModelFamily::Gemini,
    }
}

pub const fn ai_standard_spec_metadata(spec: LocalStandardSpec) -> AiExecutionSurfaceSpecMetadata {
    AiExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: Some(ai_requested_model_family_for_standard_source(spec.family)),
    }
}

pub const fn ai_same_format_provider_spec_metadata(
    spec: LocalSameFormatProviderSpec,
) -> AiExecutionSurfaceSpecMetadata {
    AiExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: Some(ai_requested_model_family_for_same_format_provider(
            spec.family,
        )),
    }
}

pub const fn ai_openai_responses_spec_metadata(
    spec: LocalOpenAiResponsesSpec,
) -> AiExecutionSurfaceSpecMetadata {
    AiExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: None,
    }
}

pub const fn ai_gemini_files_spec_metadata(
    spec: LocalGeminiFilesSpec,
) -> AiExecutionSurfaceSpecMetadata {
    AiExecutionSurfaceSpecMetadata {
        api_format: "gemini:files",
        decision_kind: spec.decision_kind,
        report_kind: spec.report_kind,
        require_streaming: spec.require_streaming,
        requested_model_family: None,
    }
}

pub const fn ai_openai_image_spec_metadata(
    spec: LocalOpenAiImageSpec,
) -> AiExecutionSurfaceSpecMetadata {
    AiExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: spec.require_streaming,
        requested_model_family: Some(AiRequestedModelFamily::Standard),
    }
}

pub const fn ai_video_create_spec_metadata(
    spec: LocalVideoCreateSpec,
) -> AiExecutionSurfaceSpecMetadata {
    AiExecutionSurfaceSpecMetadata {
        api_format: spec.api_format,
        decision_kind: spec.decision_kind,
        report_kind: Some(spec.report_kind),
        require_streaming: false,
        requested_model_family: Some(ai_requested_model_family_for_video_create(spec.family)),
    }
}

pub const fn ai_requested_model_family_for_same_format_provider(
    family: LocalSameFormatProviderFamily,
) -> AiRequestedModelFamily {
    match family {
        LocalSameFormatProviderFamily::Standard => AiRequestedModelFamily::Standard,
        LocalSameFormatProviderFamily::Gemini => AiRequestedModelFamily::Gemini,
    }
}

pub const fn ai_requested_model_family_for_video_create(
    family: LocalVideoCreateFamily,
) -> AiRequestedModelFamily {
    match family {
        LocalVideoCreateFamily::OpenAi => AiRequestedModelFamily::Standard,
        LocalVideoCreateFamily::Gemini => AiRequestedModelFamily::Gemini,
    }
}

pub fn extract_ai_gemini_model_from_path(path: &str) -> Option<String> {
    let (_, suffix) = path.split_once("/models/")?;
    let model = suffix
        .split_once(':')
        .map(|(value, _)| value)
        .unwrap_or(suffix);
    let model = model.trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

pub fn extract_ai_standard_requested_model(body_json: &Value) -> Option<String> {
    body_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn extract_ai_requested_model_from_request_path(
    request_path: &str,
    body_json: &Value,
    family: AiRequestedModelFamily,
) -> Option<String> {
    match family {
        AiRequestedModelFamily::Standard => extract_ai_standard_requested_model(body_json),
        AiRequestedModelFamily::Gemini => extract_ai_gemini_model_from_path(request_path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_ai_formats::api::{LocalStandardSourceMode, OPENAI_CHAT_SYNC_SUCCESS_REPORT_KIND};

    #[test]
    fn standard_spec_metadata_maps_model_family_and_report_kind() {
        let metadata = ai_standard_spec_metadata(LocalStandardSpec {
            mode: LocalStandardSourceMode::Chat,
            family: LocalStandardSourceFamily::Standard,
            api_format: "openai:chat",
            decision_kind: "openai_chat_sync",
            report_kind: OPENAI_CHAT_SYNC_SUCCESS_REPORT_KIND,
            require_streaming: false,
        });

        assert_eq!(metadata.api_format, "openai:chat");
        assert_eq!(
            metadata.requested_model_family,
            Some(AiRequestedModelFamily::Standard)
        );
    }

    #[test]
    fn video_spec_metadata_maps_gemini_family_without_stream_requirement() {
        let metadata = ai_video_create_spec_metadata(LocalVideoCreateSpec {
            family: LocalVideoCreateFamily::Gemini,
            api_format: "gemini:video",
            decision_kind: "gemini_video_create",
            report_kind: "gemini_video_create_success",
        });

        assert!(!metadata.require_streaming);
        assert_eq!(
            metadata.requested_model_family,
            Some(AiRequestedModelFamily::Gemini)
        );
    }

    #[test]
    fn gemini_model_path_parser_trims_method_suffix() {
        let model = extract_ai_gemini_model_from_path(
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
        );

        assert_eq!(model.as_deref(), Some("gemini-2.5-pro"));
    }

    #[test]
    fn standard_requested_model_parser_reads_request_body_model() {
        let requested_model = extract_ai_standard_requested_model(
            &serde_json::json!({ "model": " claude-sonnet-4 " }),
        );

        assert_eq!(requested_model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn requested_model_parser_delegates_by_family() {
        let body = serde_json::json!({ "model": " claude-sonnet-4 " });

        assert_eq!(
            extract_ai_requested_model_from_request_path(
                "/v1/chat/completions",
                &body,
                AiRequestedModelFamily::Standard,
            )
            .as_deref(),
            Some("claude-sonnet-4")
        );
        assert_eq!(
            extract_ai_requested_model_from_request_path(
                "/v1beta/models/gemini-2.5-pro:generateContent",
                &serde_json::json!({}),
                AiRequestedModelFamily::Gemini,
            )
            .as_deref(),
            Some("gemini-2.5-pro")
        );
    }
}
