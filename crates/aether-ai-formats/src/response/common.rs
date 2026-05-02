use std::collections::BTreeMap;

use serde_json::Value;

use crate::contracts::core_success_background_report_kind;

#[derive(Debug, Clone, PartialEq)]
pub struct LocalSyncReportParts {
    pub trace_id: String,
    pub report_kind: String,
    pub report_context: Option<Value>,
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    pub body_json: Option<Value>,
    pub client_body_json: Option<Value>,
    pub body_base64: Option<String>,
}

pub fn build_generated_tool_call_id(index: usize) -> String {
    format!("call_auto_{index}")
}

pub fn canonicalize_tool_arguments(value: Option<Value>) -> String {
    match value {
        Some(Value::String(text)) => text,
        Some(other) => serde_json::to_string(&other).unwrap_or_else(|_| "null".to_string()),
        None => "{}".to_string(),
    }
}

pub fn remove_empty_pages_from_tool_arguments(arguments: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(arguments) else {
        return arguments.to_string();
    };
    let Some(object) = value.as_object_mut() else {
        return arguments.to_string();
    };
    if object.get("pages").and_then(Value::as_str) != Some("") {
        return arguments.to_string();
    }
    object.remove("pages");
    serde_json::to_string(&value).unwrap_or_else(|_| arguments.to_string())
}

pub fn prepare_local_success_response_parts(
    headers: &BTreeMap<String, String>,
    body_json: &Value,
) -> serde_json::Result<(Vec<u8>, BTreeMap<String, String>)> {
    prepare_local_success_response_parts_owned(headers.clone(), body_json)
}

pub fn prepare_local_success_response_parts_owned(
    mut headers: BTreeMap<String, String>,
    body_json: &Value,
) -> serde_json::Result<(Vec<u8>, BTreeMap<String, String>)> {
    headers.remove("content-encoding");
    headers.remove("content-length");
    headers.insert("content-type".to_string(), "application/json".to_string());
    let body_bytes = serde_json::to_vec(body_json)?;
    headers.insert("content-length".to_string(), body_bytes.len().to_string());
    Ok((body_bytes, headers))
}

fn should_capture_client_sync_success_body(payload: &LocalSyncReportParts) -> bool {
    payload
        .report_context
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|context| context.get("upstream_is_stream"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn build_local_success_background_report(
    payload: &LocalSyncReportParts,
    body_json: Value,
    headers: BTreeMap<String, String>,
) -> Option<LocalSyncReportParts> {
    let report_kind = core_success_background_report_kind(payload.report_kind.as_str())?;
    let upstream_is_stream = should_capture_client_sync_success_body(payload);
    let client_body_json = upstream_is_stream.then(|| body_json.clone());
    let provider_body_json = if upstream_is_stream {
        payload.body_json.clone()
    } else {
        Some(body_json)
    };
    let provider_body_base64 = if upstream_is_stream {
        payload.body_base64.clone()
    } else {
        None
    };

    Some(LocalSyncReportParts {
        trace_id: payload.trace_id.clone(),
        report_kind: report_kind.to_string(),
        report_context: payload.report_context.clone(),
        status_code: payload.status_code,
        headers,
        body_json: provider_body_json,
        client_body_json,
        body_base64: provider_body_base64,
    })
}

pub fn build_local_success_conversion_background_report(
    payload: &LocalSyncReportParts,
    client_body_json: Value,
    provider_body_json: Value,
) -> Option<LocalSyncReportParts> {
    let report_kind = core_success_background_report_kind(payload.report_kind.as_str())?;

    Some(LocalSyncReportParts {
        trace_id: payload.trace_id.clone(),
        report_kind: report_kind.to_string(),
        report_context: payload.report_context.clone(),
        status_code: payload.status_code,
        headers: payload.headers.clone(),
        body_json: Some(provider_body_json),
        client_body_json: Some(client_body_json),
        body_base64: None,
    })
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use serde_json::Value;

    use super::{
        build_generated_tool_call_id, build_local_success_background_report,
        build_local_success_conversion_background_report, canonicalize_tool_arguments,
        prepare_local_success_response_parts, prepare_local_success_response_parts_owned,
        remove_empty_pages_from_tool_arguments, LocalSyncReportParts,
    };
    use std::collections::BTreeMap;

    #[test]
    fn generated_tool_call_ids_are_stable() {
        assert_eq!(build_generated_tool_call_id(3), "call_auto_3");
    }

    #[test]
    fn canonicalizes_tool_arguments() {
        assert_eq!(
            canonicalize_tool_arguments(Some(serde_json::json!({"x": 1}))),
            "{\"x\":1}"
        );
        assert_eq!(canonicalize_tool_arguments(None), "{}");
    }

    #[test]
    fn removes_empty_pages_from_tool_arguments() {
        assert_eq!(
            remove_empty_pages_from_tool_arguments(
                r#"{"file_path":"/tmp/a.txt","offset":1,"limit":20,"pages":""}"#
            ),
            r#"{"file_path":"/tmp/a.txt","offset":1,"limit":20}"#
        );
        assert_eq!(
            remove_empty_pages_from_tool_arguments(r#"{"pages":"1-2"}"#),
            r#"{"pages":"1-2"}"#
        );
        assert_eq!(
            remove_empty_pages_from_tool_arguments(r#"{"pages":"#),
            r#"{"pages":"#
        );
    }

    #[test]
    fn prepare_local_success_response_parts_normalizes_headers() {
        let headers = BTreeMap::from([
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-length".to_string(), "999".to_string()),
            ("x-test".to_string(), "1".to_string()),
        ]);
        let (body_bytes, normalized_headers) =
            prepare_local_success_response_parts(&headers, &serde_json::json!({"ok": true}))
                .expect("response parts should serialize");

        assert_eq!(
            serde_json::from_slice::<Value>(&body_bytes).expect("json body"),
            serde_json::json!({"ok": true})
        );
        assert_eq!(
            normalized_headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert!(!normalized_headers.contains_key("content-encoding"));
        let expected_length = body_bytes.len().to_string();
        assert_eq!(
            normalized_headers.get("content-length").map(String::as_str),
            Some(expected_length.as_str())
        );
        assert_eq!(
            normalized_headers.get("x-test").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn prepare_local_success_response_parts_owned_normalizes_headers() {
        let headers = BTreeMap::from([
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-length".to_string(), "999".to_string()),
            ("x-test".to_string(), "1".to_string()),
        ]);
        let (body_bytes, normalized_headers) =
            prepare_local_success_response_parts_owned(headers, &serde_json::json!({"ok": true}))
                .expect("response parts should serialize");

        assert_eq!(
            serde_json::from_slice::<Value>(&body_bytes).expect("json body"),
            serde_json::json!({"ok": true})
        );
        assert_eq!(
            normalized_headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert!(!normalized_headers.contains_key("content-encoding"));
        let expected_length = body_bytes.len().to_string();
        assert_eq!(
            normalized_headers.get("content-length").map(String::as_str),
            Some(expected_length.as_str())
        );
        assert_eq!(
            normalized_headers.get("x-test").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn build_local_success_background_report_maps_finalize_kind() {
        let payload = LocalSyncReportParts {
            trace_id: "trace-1".to_string(),
            report_kind: "openai_chat_sync_finalize".to_string(),
            report_context: Some(serde_json::json!({"request_id": "req-1"})),
            status_code: 200,
            headers: BTreeMap::from([("x-test".to_string(), "1".to_string())]),
            body_json: None,
            client_body_json: None,
            body_base64: None,
        };

        let report = build_local_success_background_report(
            &payload,
            serde_json::json!({"id": "resp-1"}),
            payload.headers.clone(),
        )
        .expect("success report should be built");

        assert_eq!(report.report_kind, "openai_chat_sync_success");
        assert_eq!(report.body_json, Some(serde_json::json!({"id": "resp-1"})));
        assert_eq!(report.client_body_json, None);
    }

    #[test]
    fn build_local_success_background_report_preserves_provider_stream_for_upstream_stream_sync() {
        let payload = LocalSyncReportParts {
            trace_id: "trace-1b".to_string(),
            report_kind: "openai_chat_sync_finalize".to_string(),
            report_context: Some(serde_json::json!({
                "request_id": "req-1b",
                "upstream_is_stream": true
            })),
            status_code: 200,
            headers: BTreeMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
            body_json: None,
            client_body_json: None,
            body_base64: Some(base64::engine::general_purpose::STANDARD.encode(
                concat!(
                    "event: response.created\n",
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-1b\",\"object\":\"response\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
                    "event: response.output_text.delta\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-1b\",\"object\":\"response\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
                )
            )),
        };

        let report = build_local_success_background_report(
            &payload,
            serde_json::json!({"id": "resp-1b"}),
            payload.headers.clone(),
        )
        .expect("success report should be built");

        assert_eq!(report.body_json, None);
        assert_eq!(
            report.client_body_json,
            Some(serde_json::json!({"id": "resp-1b"}))
        );
        assert_eq!(report.body_base64, payload.body_base64);
    }

    #[test]
    fn build_local_success_conversion_background_report_maps_provider_body() {
        let payload = LocalSyncReportParts {
            trace_id: "trace-2".to_string(),
            report_kind: "openai_chat_sync_finalize".to_string(),
            report_context: Some(serde_json::json!({"request_id": "req-2"})),
            status_code: 200,
            headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            body_json: None,
            client_body_json: None,
            body_base64: None,
        };

        let report = build_local_success_conversion_background_report(
            &payload,
            serde_json::json!({"client": true}),
            serde_json::json!({"provider": true}),
        )
        .expect("conversion success report should be built");

        assert_eq!(report.report_kind, "openai_chat_sync_success");
        assert_eq!(
            report.body_json,
            Some(serde_json::json!({"provider": true}))
        );
        assert_eq!(
            report.client_body_json,
            Some(serde_json::json!({"client": true}))
        );
    }
}
