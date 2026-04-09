use serde_json::json;

use super::{
    maybe_build_provider_private_stream_normalizer, normalize_provider_private_report_context,
};

#[test]
fn normalizes_supported_private_report_context() {
    let report_context = json!({
        "has_envelope": true,
        "envelope_name": "antigravity:v1internal",
        "provider_api_format": "gemini:cli",
    });
    let normalized = normalize_provider_private_report_context(Some(&report_context))
        .expect("context should normalize");
    assert_eq!(normalized["has_envelope"], json!(false));
    assert!(normalized.get("envelope_name").is_none());
}

#[test]
fn private_stream_normalizer_unwraps_antigravity_stream() {
    let report_context = json!({
        "has_envelope": true,
        "provider_api_format": "gemini:cli",
        "client_api_format": "gemini:cli",
        "envelope_name": "antigravity:v1internal",
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut normalizer = maybe_build_provider_private_stream_normalizer(Some(&report_context))
        .expect("normalizer should exist");
    let output = normalizer
        .push_chunk(
            b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"SF\"}}}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\"},\"responseId\":\"resp_123\"}\n\n",
        )
        .expect("unwrap should succeed");
    let output_text = String::from_utf8(output).expect("text should decode");
    assert!(output_text.contains("\"_v1internal_response_id\":\"resp_123\""));
    assert!(output_text.contains("\"id\":\"call_get_weather_0\""));
}
