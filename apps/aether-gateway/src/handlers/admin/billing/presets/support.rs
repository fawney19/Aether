use super::super::{
    build_admin_billing_bad_request_response, normalize_admin_billing_required_text,
};
use axum::{
    body::{Body, Bytes},
    response::Response,
};
use serde::Deserialize;
use serde_json::json;

fn default_admin_billing_preset_mode() -> String {
    "merge".to_string()
}

#[derive(Debug, Deserialize)]
struct AdminBillingPresetApplyRequest {
    preset: String,
    #[serde(default = "default_admin_billing_preset_mode")]
    mode: String,
}

pub(super) fn build_admin_billing_presets_payload() -> serde_json::Value {
    json!({
        "items": [
            {
                "name": "aether-core",
                "version": "1.0",
                "description": "Aether built-in dimension collectors for common api_formats/task_types.",
                "collector_count": build_admin_billing_aether_core_collectors().len(),
            }
        ],
    })
}

fn build_admin_billing_aether_core_collectors() -> Vec<crate::AdminBillingCollectorWriteInput> {
    vec![
        crate::AdminBillingCollectorWriteInput {
            api_format: "OPENAI:CHAT".to_string(),
            task_type: "chat".to_string(),
            dimension_name: "input_tokens".to_string(),
            source_type: "response".to_string(),
            source_path: Some("usage.prompt_tokens".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "OPENAI:CHAT".to_string(),
            task_type: "chat".to_string(),
            dimension_name: "output_tokens".to_string(),
            source_type: "response".to_string(),
            source_path: Some("usage.completion_tokens".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "CLAUDE:CHAT".to_string(),
            task_type: "chat".to_string(),
            dimension_name: "input_tokens".to_string(),
            source_type: "response".to_string(),
            source_path: Some("usage.input_tokens".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "CLAUDE:CHAT".to_string(),
            task_type: "chat".to_string(),
            dimension_name: "output_tokens".to_string(),
            source_type: "response".to_string(),
            source_path: Some("usage.output_tokens".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "GEMINI:CHAT".to_string(),
            task_type: "chat".to_string(),
            dimension_name: "input_tokens".to_string(),
            source_type: "response".to_string(),
            source_path: Some("usageMetadata.promptTokenCount".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "GEMINI:CHAT".to_string(),
            task_type: "chat".to_string(),
            dimension_name: "output_tokens".to_string(),
            source_type: "response".to_string(),
            source_path: Some("usageMetadata.candidatesTokenCount".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "OPENAI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_resolution_key".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.size".to_string()),
            value_type: "string".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "OPENAI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_resolution_key".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.resolution".to_string()),
            value_type: "string".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 0,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "OPENAI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_size_bytes".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.video_size_bytes".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 0,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "OPENAI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_duration_seconds".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.video_duration_seconds".to_string()),
            value_type: "float".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "OPENAI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_duration_seconds".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.duration_seconds".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 0,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "GEMINI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_resolution_key".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.size".to_string()),
            value_type: "string".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "GEMINI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_resolution_key".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.resolution".to_string()),
            value_type: "string".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 0,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "GEMINI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_size_bytes".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.video_size_bytes".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 0,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "GEMINI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_duration_seconds".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.video_duration_seconds".to_string()),
            value_type: "float".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 10,
            is_enabled: true,
        },
        crate::AdminBillingCollectorWriteInput {
            api_format: "GEMINI:CHAT".to_string(),
            task_type: "video".to_string(),
            dimension_name: "video_duration_seconds".to_string(),
            source_type: "metadata".to_string(),
            source_path: Some("task.duration_seconds".to_string()),
            value_type: "int".to_string(),
            transform_expression: None,
            default_value: None,
            priority: 0,
            is_enabled: true,
        },
    ]
}

pub(super) fn resolve_admin_billing_preset_collectors(
    preset: &str,
) -> Option<(&'static str, Vec<crate::AdminBillingCollectorWriteInput>)> {
    let normalized = preset.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "aether-core" | "default" => {
            Some(("aether-core", build_admin_billing_aether_core_collectors()))
        }
        _ => None,
    }
}

pub(super) fn parse_admin_billing_preset_apply_request(
    request_body: Option<&Bytes>,
) -> Result<(String, String), Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(build_admin_billing_bad_request_response("请求体不能为空"));
    };
    let request = match serde_json::from_slice::<AdminBillingPresetApplyRequest>(request_body) {
        Ok(value) => value,
        Err(err) => {
            return Err(build_admin_billing_bad_request_response(format!(
                "Invalid request body: {err}"
            )))
        }
    };
    let preset = match normalize_admin_billing_required_text(&request.preset, "preset", 100) {
        Ok(value) => value,
        Err(detail) => return Err(build_admin_billing_bad_request_response(detail)),
    };
    let mode = request.mode.trim().to_ascii_lowercase();
    if !matches!(mode.as_str(), "merge" | "overwrite") {
        return Err(build_admin_billing_bad_request_response(
            "mode must be one of merge, overwrite",
        ));
    }
    Ok((preset, mode))
}
