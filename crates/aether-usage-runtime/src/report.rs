use std::collections::BTreeMap;

use aether_contracts::{ExecutionStreamTerminalSummary, ExecutionTelemetry};
use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const GEMINI_FILE_MAPPING_TTL_SECONDS: u64 = 60 * 60 * 48;
const GEMINI_FILE_MAPPING_CACHE_PREFIX: &str = "gemini_files:key";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewaySyncReportRequest {
    pub trace_id: String,
    pub report_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_context: Option<serde_json::Value>,
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_body_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<ExecutionTelemetry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewayStreamReportRequest {
    pub trace_id: String,
    pub report_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_context: Option<serde_json::Value>,
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_summary: Option<ExecutionStreamTerminalSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<ExecutionTelemetry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InternalFinalizeRoute {
    pub public_path: &'static str,
    pub route_family: &'static str,
    pub route_kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiFileMappingEntry {
    pub file_name: String,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
}

pub fn infer_internal_finalize_signature(payload: &GatewaySyncReportRequest) -> Option<String> {
    let report_context = payload.report_context.as_ref()?;
    let from_context = report_context
        .get("client_api_format")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            report_context
                .get("provider_api_format")
                .and_then(serde_json::Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if from_context.is_some() {
        return from_context;
    }

    let report_kind = payload.report_kind.trim().to_ascii_lowercase();
    if report_kind.starts_with("openai_chat_") {
        return Some("openai:chat".to_string());
    }
    if report_kind.starts_with("openai_compact_") {
        return Some("openai:compact".to_string());
    }
    if report_kind.starts_with("openai_image_") {
        return Some("openai:image".to_string());
    }
    if report_kind.starts_with("openai_cli_") {
        return Some("openai:cli".to_string());
    }
    if report_kind.starts_with("openai_video_") {
        return Some("openai:video".to_string());
    }
    if report_kind.starts_with("claude_chat_") {
        return Some("claude:chat".to_string());
    }
    if report_kind.starts_with("claude_cli_") {
        return Some("claude:cli".to_string());
    }
    if report_kind.starts_with("gemini_chat_") {
        return Some("gemini:chat".to_string());
    }
    if report_kind.starts_with("gemini_cli_") {
        return Some("gemini:cli".to_string());
    }
    if report_kind.starts_with("gemini_video_") {
        return Some("gemini:video".to_string());
    }
    None
}

pub fn resolve_internal_finalize_route(signature: &str) -> Option<InternalFinalizeRoute> {
    match signature {
        "openai:chat" => Some(InternalFinalizeRoute {
            public_path: "/v1/chat/completions",
            route_family: "openai",
            route_kind: "chat",
        }),
        "openai:cli" => Some(InternalFinalizeRoute {
            public_path: "/v1/responses",
            route_family: "openai",
            route_kind: "cli",
        }),
        "openai:compact" => Some(InternalFinalizeRoute {
            public_path: "/v1/responses/compact",
            route_family: "openai",
            route_kind: "compact",
        }),
        "openai:image" => Some(InternalFinalizeRoute {
            public_path: "/v1/images/generations",
            route_family: "openai",
            route_kind: "image",
        }),
        "openai:video" => Some(InternalFinalizeRoute {
            public_path: "/v1/videos",
            route_family: "openai",
            route_kind: "video",
        }),
        "claude:chat" => Some(InternalFinalizeRoute {
            public_path: "/v1/messages",
            route_family: "claude",
            route_kind: "chat",
        }),
        "claude:cli" => Some(InternalFinalizeRoute {
            public_path: "/v1/messages",
            route_family: "claude",
            route_kind: "cli",
        }),
        "gemini:chat" => Some(InternalFinalizeRoute {
            public_path: "/v1beta/models",
            route_family: "gemini",
            route_kind: "chat",
        }),
        "gemini:cli" => Some(InternalFinalizeRoute {
            public_path: "/v1beta/models",
            route_family: "gemini",
            route_kind: "cli",
        }),
        "gemini:video" => Some(InternalFinalizeRoute {
            public_path: "/v1beta/models",
            route_family: "gemini",
            route_kind: "video",
        }),
        _ => None,
    }
}

pub fn normalize_gemini_file_name(file_name: &str) -> Option<String> {
    let file_name = file_name.trim();
    if file_name.is_empty() {
        return None;
    }
    if file_name.starts_with("files/") {
        Some(file_name.to_string())
    } else {
        Some(format!("files/{file_name}"))
    }
}

pub fn gemini_file_mapping_cache_key(file_name: &str) -> String {
    format!("{GEMINI_FILE_MAPPING_CACHE_PREFIX}:{file_name}")
}

pub fn extract_gemini_file_mapping_entries(
    payload: &GatewaySyncReportRequest,
) -> Vec<GeminiFileMappingEntry> {
    let Some(body) = extract_sync_report_body_json(payload) else {
        return Vec::new();
    };
    let Some(object) = body.as_object() else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    maybe_push_gemini_file_mapping_entry(&mut entries, object);

    if let Some(file_object) = object.get("file").and_then(Value::as_object) {
        maybe_push_gemini_file_mapping_entry(&mut entries, file_object);
    }

    if let Some(files) = object.get("files").and_then(Value::as_array) {
        for item in files {
            if let Some(file_object) = item.as_object() {
                maybe_push_gemini_file_mapping_entry(&mut entries, file_object);
            }
        }
    }

    entries
}

pub fn report_request_id(report_context: Option<&serde_json::Value>) -> &str {
    report_context
        .and_then(|context| context.get("request_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("-")
}

pub fn is_local_ai_sync_report_kind(report_kind: &str) -> bool {
    matches!(
        report_kind,
        "openai_chat_sync_success"
            | "claude_chat_sync_success"
            | "gemini_chat_sync_success"
            | "openai_chat_sync_error"
            | "claude_chat_sync_error"
            | "gemini_chat_sync_error"
            | "openai_cli_sync_success"
            | "openai_image_sync_success"
            | "claude_cli_sync_success"
            | "gemini_cli_sync_success"
            | "openai_cli_sync_error"
            | "openai_compact_sync_error"
            | "claude_cli_sync_error"
            | "gemini_cli_sync_error"
            | "openai_video_create_sync_success"
            | "openai_video_remix_sync_success"
            | "gemini_video_create_sync_success"
            | "openai_video_delete_sync_success"
            | "openai_video_cancel_sync_success"
            | "gemini_video_cancel_sync_success"
            | "openai_video_create_sync_error"
            | "openai_video_remix_sync_error"
            | "gemini_video_create_sync_error"
            | "openai_video_delete_sync_error"
            | "openai_video_cancel_sync_error"
            | "gemini_video_cancel_sync_error"
            | "gemini_files_store_mapping"
            | "gemini_files_delete_mapping"
    )
}

pub fn is_local_ai_stream_report_kind(report_kind: &str) -> bool {
    matches!(
        report_kind,
        "openai_chat_stream_success"
            | "claude_chat_stream_success"
            | "gemini_chat_stream_success"
            | "openai_cli_stream_success"
            | "claude_cli_stream_success"
            | "gemini_cli_stream_success"
    )
}

pub fn sync_report_represents_failure(
    payload: &GatewaySyncReportRequest,
    error_type: Option<&str>,
) -> bool {
    if payload.report_kind == "openai_video_delete_sync_success" && payload.status_code == 404 {
        return false;
    }

    payload.status_code >= 400
        || payload.report_kind.contains("error")
        || error_type.is_some()
        || payload
            .body_json
            .as_ref()
            .and_then(|body| body.get("error"))
            .is_some_and(|value| !value.is_null())
}

pub fn should_handle_local_sync_report(
    report_context: Option<&serde_json::Value>,
    report_kind: &str,
) -> bool {
    crate::report_context::report_context_is_locally_actionable(report_context)
        && is_local_ai_sync_report_kind(report_kind)
}

pub fn should_handle_local_stream_report(
    report_context: Option<&serde_json::Value>,
    report_kind: &str,
) -> bool {
    crate::report_context::report_context_is_locally_actionable(report_context)
        && is_local_ai_stream_report_kind(report_kind)
}

fn maybe_push_gemini_file_mapping_entry(
    entries: &mut Vec<GeminiFileMappingEntry>,
    object: &serde_json::Map<String, Value>,
) {
    let file_name = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(normalize_gemini_file_name);
    let Some(file_name) = file_name else {
        return;
    };

    if entries.iter().any(|entry| entry.file_name == file_name) {
        return;
    }

    entries.push(GeminiFileMappingEntry {
        file_name,
        display_name: object
            .get("displayName")
            .or_else(|| object.get("display_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        mime_type: object
            .get("mimeType")
            .or_else(|| object.get("mime_type"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    });
}

fn extract_sync_report_body_json(payload: &GatewaySyncReportRequest) -> Option<Value> {
    if let Some(body_json) = payload.body_json.as_ref() {
        return Some(body_json.clone());
    }
    if let Some(client_body_json) = payload.client_body_json.as_ref() {
        return Some(client_body_json.clone());
    }
    if !content_type_starts_with(&payload.headers, "application/json") {
        return None;
    }

    let body_base64 = payload.body_base64.as_deref()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn content_type_starts_with(headers: &BTreeMap<String, String>, expected_prefix: &str) -> bool {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_ascii_lowercase())
        .is_some_and(|value| value.starts_with(expected_prefix))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use base64::Engine as _;
    use serde_json::json;

    use super::{
        extract_gemini_file_mapping_entries, gemini_file_mapping_cache_key,
        infer_internal_finalize_signature, is_local_ai_stream_report_kind,
        is_local_ai_sync_report_kind, normalize_gemini_file_name, report_request_id,
        resolve_internal_finalize_route, should_handle_local_stream_report,
        should_handle_local_sync_report, sync_report_represents_failure, GatewaySyncReportRequest,
        GeminiFileMappingEntry, InternalFinalizeRoute,
    };

    fn sample_sync_report(report_kind: &str, status_code: u16) -> GatewaySyncReportRequest {
        GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: report_kind.to_string(),
            report_context: None,
            status_code,
            headers: BTreeMap::new(),
            body_json: None,
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        }
    }

    fn sample_sync_report_with_context(
        report_kind: &str,
        report_context: serde_json::Value,
    ) -> GatewaySyncReportRequest {
        GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: report_kind.to_string(),
            report_context: Some(report_context),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: None,
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        }
    }

    #[test]
    fn classifies_local_ai_sync_report_kinds() {
        assert!(is_local_ai_sync_report_kind(
            "openai_video_create_sync_success"
        ));
        assert!(is_local_ai_sync_report_kind("openai_image_sync_success"));
        assert!(is_local_ai_sync_report_kind("gemini_files_delete_mapping"));
        assert!(!is_local_ai_sync_report_kind("unknown_sync_kind"));
    }

    #[test]
    fn classifies_local_ai_stream_report_kinds() {
        assert!(is_local_ai_stream_report_kind("openai_chat_stream_success"));
        assert!(!is_local_ai_stream_report_kind("openai_chat_stream_error"));
    }

    #[test]
    fn treats_openai_video_delete_404_success_as_non_failure() {
        let payload = sample_sync_report("openai_video_delete_sync_success", 404);
        assert!(!sync_report_represents_failure(&payload, None));
    }

    #[test]
    fn detects_sync_report_failure_from_status_kind_error_type_or_body() {
        let status_payload = sample_sync_report("openai_chat_sync_success", 500);
        assert!(sync_report_represents_failure(&status_payload, None));

        let kind_payload = sample_sync_report("openai_chat_sync_error", 200);
        assert!(sync_report_represents_failure(&kind_payload, None));

        let error_type_payload = sample_sync_report("openai_chat_sync_success", 200);
        assert!(sync_report_represents_failure(
            &error_type_payload,
            Some("authentication_error")
        ));

        let mut error_body_payload = sample_sync_report("openai_chat_sync_success", 200);
        error_body_payload.body_json = Some(json!({"error": {"message": "bad request"}}));
        assert!(sync_report_represents_failure(&error_body_payload, None));

        let mut null_error_payload = sample_sync_report("openai_chat_sync_success", 200);
        null_error_payload.body_json = Some(json!({"error": null}));
        assert!(!sync_report_represents_failure(&null_error_payload, None));

        let success_payload = sample_sync_report("openai_chat_sync_success", 200);
        assert!(!sync_report_represents_failure(&success_payload, None));
    }

    #[test]
    fn infers_internal_finalize_signature_from_context_or_report_kind() {
        let from_context = sample_sync_report_with_context(
            "unknown_sync_finalize",
            json!({"client_api_format": "gemini:video"}),
        );
        assert_eq!(
            infer_internal_finalize_signature(&from_context),
            Some("gemini:video".to_string())
        );

        let from_report_kind =
            sample_sync_report_with_context("openai_video_create_sync_finalize", json!({}));
        assert_eq!(
            infer_internal_finalize_signature(&from_report_kind),
            Some("openai:video".to_string())
        );

        let from_image_report_kind =
            sample_sync_report_with_context("openai_image_sync_finalize", json!({}));
        assert_eq!(
            infer_internal_finalize_signature(&from_image_report_kind),
            Some("openai:image".to_string())
        );

        let unknown = sample_sync_report("unknown_sync_finalize", 200);
        assert_eq!(infer_internal_finalize_signature(&unknown), None);
    }

    #[test]
    fn resolves_internal_finalize_route_for_supported_signatures() {
        assert_eq!(
            resolve_internal_finalize_route("openai:compact"),
            Some(InternalFinalizeRoute {
                public_path: "/v1/responses/compact",
                route_family: "openai",
                route_kind: "compact",
            })
        );
        assert_eq!(
            resolve_internal_finalize_route("gemini:video"),
            Some(InternalFinalizeRoute {
                public_path: "/v1beta/models",
                route_family: "gemini",
                route_kind: "video",
            })
        );
        assert_eq!(
            resolve_internal_finalize_route("openai:image"),
            Some(InternalFinalizeRoute {
                public_path: "/v1/images/generations",
                route_family: "openai",
                route_kind: "image",
            })
        );
        assert_eq!(resolve_internal_finalize_route("unknown:kind"), None);
    }

    #[test]
    fn normalizes_gemini_file_names() {
        assert_eq!(
            normalize_gemini_file_name("abc123"),
            Some("files/abc123".to_string())
        );
        assert_eq!(
            normalize_gemini_file_name("files/abc123"),
            Some("files/abc123".to_string())
        );
        assert_eq!(normalize_gemini_file_name("   "), None);
    }

    #[test]
    fn builds_gemini_file_mapping_cache_keys() {
        assert_eq!(
            gemini_file_mapping_cache_key("files/abc123"),
            "gemini_files:key:files/abc123"
        );
    }

    #[test]
    fn extracts_gemini_file_mapping_entries_from_supported_shapes() {
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: "gemini_files_store_mapping".to_string(),
            report_context: None,
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({
                "name": "abc123",
                "displayName": "root-name",
                "file": {
                    "name": "files/def456",
                    "mimeType": "image/png"
                },
                "files": [
                    {
                        "name": "abc123",
                        "display_name": "deduped"
                    },
                    {
                        "name": "ghi789",
                        "display_name": "third"
                    }
                ]
            })),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let entries = extract_gemini_file_mapping_entries(&payload);
        assert_eq!(
            entries,
            vec![
                GeminiFileMappingEntry {
                    file_name: "files/abc123".to_string(),
                    display_name: Some("root-name".to_string()),
                    mime_type: None,
                },
                GeminiFileMappingEntry {
                    file_name: "files/def456".to_string(),
                    display_name: None,
                    mime_type: Some("image/png".to_string()),
                },
                GeminiFileMappingEntry {
                    file_name: "files/ghi789".to_string(),
                    display_name: Some("third".to_string()),
                    mime_type: None,
                }
            ]
        );
    }

    #[test]
    fn extracts_gemini_file_mapping_entries_from_base64_json_body() {
        let encoded_body = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&json!({
                "name": "base64-file"
            }))
            .expect("json should encode"),
        );
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: "gemini_files_store_mapping".to_string(),
            report_context: None,
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "application/json; charset=utf-8".to_string(),
            )]),
            body_json: None,
            client_body_json: None,
            body_base64: Some(encoded_body),
            telemetry: None,
        };

        let entries = extract_gemini_file_mapping_entries(&payload);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name, "files/base64-file");
    }

    #[test]
    fn reads_report_request_id_from_context() {
        assert_eq!(
            report_request_id(Some(&json!({"request_id": "req-123"}))),
            "req-123"
        );
        assert_eq!(report_request_id(Some(&json!({"request_id": "   "}))), "-");
        assert_eq!(report_request_id(None), "-");
    }

    #[test]
    fn decides_when_local_sync_report_should_be_handled() {
        assert!(should_handle_local_sync_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_chat_sync_success"
        ));
        assert!(should_handle_local_sync_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_image_sync_success"
        ));
        assert!(!should_handle_local_sync_report(
            Some(&json!({"request_id": "req-123"})),
            "openai_chat_sync_success"
        ));
        assert!(!should_handle_local_sync_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "unknown_sync_kind"
        ));
    }

    #[test]
    fn decides_when_local_stream_report_should_be_handled() {
        assert!(should_handle_local_stream_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_chat_stream_success"
        ));
        assert!(!should_handle_local_stream_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_chat_stream_error"
        ));
    }
}
