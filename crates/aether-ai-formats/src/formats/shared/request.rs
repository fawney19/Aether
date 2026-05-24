use base64::Engine as _;

use crate::formats::id::api_format_uses_body_stream_field;

/// JSON key under which `upstream_is_stream` is written into the AI execution
/// report context and propagated into usage metadata. Shared by the producer
/// (`aether-ai-serving::report_context`) and every downstream consumer so that
/// renames cannot silently desync them — a string-literal mismatch here would
/// degrade to default values (e.g. assuming streaming) without any compile-time
/// signal.
pub const UPSTREAM_IS_STREAM_KEY: &str = "upstream_is_stream";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpstreamStreamPolicy {
    Auto,
    ForceStream,
    ForceNonStream,
}

pub fn parse_direct_request_body(
    is_json_request: bool,
    body_bytes: &[u8],
) -> Option<(serde_json::Value, Option<String>)> {
    if is_json_request {
        if body_bytes.is_empty() {
            Some((serde_json::json!({}), None))
        } else {
            serde_json::from_slice::<serde_json::Value>(body_bytes)
                .ok()
                .map(|value| (value, None))
        }
    } else {
        Some((
            serde_json::json!({}),
            (!body_bytes.is_empty())
                .then(|| base64::engine::general_purpose::STANDARD.encode(body_bytes)),
        ))
    }
}

pub fn force_upstream_streaming_for_provider(
    provider_type: &str,
    provider_api_format: &str,
) -> bool {
    provider_type.trim().eq_ignore_ascii_case("codex")
        && aether_ai_formats::is_openai_responses_format(provider_api_format)
}

pub(crate) fn parse_upstream_stream_policy(
    value: Option<&serde_json::Value>,
) -> UpstreamStreamPolicy {
    let Some(value) = value else {
        return UpstreamStreamPolicy::Auto;
    };
    if let Some(value) = value.as_bool() {
        return if value {
            UpstreamStreamPolicy::ForceStream
        } else {
            UpstreamStreamPolicy::ForceNonStream
        };
    }

    let serde_json::Value::String(value) = value else {
        return UpstreamStreamPolicy::Auto;
    };
    let raw = value.trim().to_ascii_lowercase();
    match raw.as_str() {
        "" | "auto" | "follow" | "client" | "default" => UpstreamStreamPolicy::Auto,
        "force_stream" | "stream" | "sse" | "true" | "1" | "yes" => {
            UpstreamStreamPolicy::ForceStream
        }
        "force_non_stream" | "force_sync" | "non_stream" | "sync" | "false" | "0" | "no" => {
            UpstreamStreamPolicy::ForceNonStream
        }
        _ => UpstreamStreamPolicy::Auto,
    }
}

pub(crate) fn upstream_stream_policy_from_endpoint_config(
    endpoint_config: Option<&serde_json::Value>,
) -> UpstreamStreamPolicy {
    let Some(config) = endpoint_config.and_then(serde_json::Value::as_object) else {
        return UpstreamStreamPolicy::Auto;
    };
    for key in [
        "upstream_stream_policy",
        "upstreamStreamPolicy",
        "upstream_stream",
    ] {
        if let Some(value) = config.get(key) {
            return parse_upstream_stream_policy(Some(value));
        }
    }
    UpstreamStreamPolicy::Auto
}

pub fn endpoint_config_forces_upstream_stream_policy(
    endpoint_config: Option<&serde_json::Value>,
) -> bool {
    matches!(
        upstream_stream_policy_from_endpoint_config(endpoint_config),
        UpstreamStreamPolicy::ForceStream | UpstreamStreamPolicy::ForceNonStream
    )
}

/// Resolves the upstream provider transport mode.
///
/// `client_is_stream` means the request landed on a streaming surface or should
/// be treated as streaming; the original JSON body may not have had
/// `"stream": true`.
pub(crate) fn resolve_upstream_is_stream(
    client_is_stream: bool,
    hard_requires_streaming: bool,
    policy: UpstreamStreamPolicy,
) -> bool {
    // ForceStream is unconditional, while ForceNonStream yields to hard
    // stream-only constraints such as Kiro or Codex OpenAI Responses.
    match policy {
        UpstreamStreamPolicy::ForceStream => true,
        UpstreamStreamPolicy::ForceNonStream => hard_requires_streaming,
        UpstreamStreamPolicy::Auto => hard_requires_streaming || client_is_stream,
    }
}

pub fn enforce_request_body_stream_field(
    body: &mut serde_json::Value,
    provider_api_format: &str,
    upstream_is_stream: bool,
    require_body_stream_field: bool,
) {
    let Some(body_object) = body.as_object_mut() else {
        return;
    };
    if !api_format_uses_body_stream_field(provider_api_format) {
        body_object.remove("stream");
        return;
    }

    // Final-body fallback catches body rules, directive patches, and other
    // provider-body mutations that introduce `stream`.
    if upstream_is_stream || require_body_stream_field || body_object.contains_key("stream") {
        body_object.insert(
            "stream".to_string(),
            serde_json::Value::Bool(upstream_is_stream),
        );
    } else {
        body_object.remove("stream");
    }
}

pub fn resolve_upstream_is_stream_from_endpoint_config(
    endpoint_config: Option<&serde_json::Value>,
    client_is_stream: bool,
    hard_requires_streaming: bool,
) -> bool {
    resolve_upstream_is_stream(
        client_is_stream,
        hard_requires_streaming,
        upstream_stream_policy_from_endpoint_config(endpoint_config),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        endpoint_config_forces_upstream_stream_policy, enforce_request_body_stream_field,
        force_upstream_streaming_for_provider, parse_direct_request_body,
        parse_upstream_stream_policy, resolve_upstream_is_stream,
        resolve_upstream_is_stream_from_endpoint_config,
        upstream_stream_policy_from_endpoint_config, UpstreamStreamPolicy,
    };
    use serde_json::json;

    #[test]
    fn parses_empty_json_body_as_empty_object() {
        assert_eq!(
            parse_direct_request_body(true, b""),
            Some((serde_json::json!({}), None))
        );
    }

    #[test]
    fn rejects_invalid_json_body() {
        assert_eq!(parse_direct_request_body(true, b"{invalid"), None);
    }

    #[test]
    fn encodes_non_json_body_as_base64() {
        assert_eq!(
            parse_direct_request_body(false, b"hello"),
            Some((serde_json::json!({}), Some("aGVsbG8=".to_string())))
        );
    }

    #[test]
    fn forces_streaming_for_codex_openai_responses() {
        assert!(force_upstream_streaming_for_provider(
            "codex",
            "openai:responses"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:responses:compact"
        ));
    }

    #[test]
    fn does_not_force_streaming_for_compact_or_other_provider_types() {
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:responses:compact"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "openai",
            "openai:responses"
        ));
    }

    #[test]
    fn parses_python_compatible_upstream_stream_policy_values() {
        assert_eq!(
            parse_upstream_stream_policy(None),
            UpstreamStreamPolicy::Auto
        );
        for value in [
            json!(""),
            json!("auto"),
            json!("follow"),
            json!("client"),
            json!("default"),
            json!("unknown"),
        ] {
            assert_eq!(
                parse_upstream_stream_policy(Some(&value)),
                UpstreamStreamPolicy::Auto
            );
        }
        for value in [
            json!(true),
            json!("force_stream"),
            json!("stream"),
            json!("sse"),
            json!("true"),
            json!("1"),
            json!("yes"),
        ] {
            assert_eq!(
                parse_upstream_stream_policy(Some(&value)),
                UpstreamStreamPolicy::ForceStream
            );
        }
        for value in [
            json!(false),
            json!("force_non_stream"),
            json!("force_sync"),
            json!("non_stream"),
            json!("sync"),
            json!("false"),
            json!("0"),
            json!("no"),
        ] {
            assert_eq!(
                parse_upstream_stream_policy(Some(&value)),
                UpstreamStreamPolicy::ForceNonStream
            );
        }
    }

    #[test]
    fn parses_non_string_non_bool_policy_values_as_auto() {
        for value in [json!(1), json!(0), json!(null), json!({}), json!([])] {
            assert_eq!(
                parse_upstream_stream_policy(Some(&value)),
                UpstreamStreamPolicy::Auto
            );
        }
    }

    #[test]
    fn enforces_request_body_stream_field_for_stream_and_streamless_formats() {
        let mut openai_chat = json!({"stream": true});
        enforce_request_body_stream_field(&mut openai_chat, "openai:chat", false, false);
        assert_eq!(openai_chat.get("stream"), Some(&json!(false)));

        let mut ordinary_sync = json!({"messages": []});
        enforce_request_body_stream_field(&mut ordinary_sync, "openai:chat", false, false);
        assert!(ordinary_sync.get("stream").is_none());

        let mut forced_sync = json!({"messages": []});
        enforce_request_body_stream_field(&mut forced_sync, "openai:chat", false, true);
        assert_eq!(forced_sync.get("stream"), Some(&json!(false)));

        let mut compact = json!({"stream": true});
        enforce_request_body_stream_field(&mut compact, "openai:responses:compact", true, true);
        assert!(compact.get("stream").is_none());
    }

    #[test]
    fn reads_endpoint_policy_keys_in_python_compatible_order() {
        assert_eq!(
            upstream_stream_policy_from_endpoint_config(Some(&json!({
                "upstream_stream_policy": "force_non_stream",
                "upstreamStreamPolicy": "force_stream",
                "upstream_stream": "force_stream"
            }))),
            UpstreamStreamPolicy::ForceNonStream
        );
        assert_eq!(
            upstream_stream_policy_from_endpoint_config(Some(&json!({
                "upstreamStreamPolicy": "force_stream"
            }))),
            UpstreamStreamPolicy::ForceStream
        );
        assert_eq!(
            upstream_stream_policy_from_endpoint_config(Some(&json!({
                "upstream_stream": false
            }))),
            UpstreamStreamPolicy::ForceNonStream
        );
    }

    #[test]
    fn detects_forced_endpoint_policy_values() {
        assert!(endpoint_config_forces_upstream_stream_policy(Some(
            &json!({"upstream_stream_policy": "force_stream"})
        )));
        assert!(endpoint_config_forces_upstream_stream_policy(Some(
            &json!({"upstream_stream_policy": "force_non_stream"})
        )));
        assert!(!endpoint_config_forces_upstream_stream_policy(Some(
            &json!({"upstream_stream_policy": "auto"})
        )));
        assert!(!endpoint_config_forces_upstream_stream_policy(None));
    }

    #[test]
    fn resolves_upstream_stream_policy_against_client_mode_and_hard_constraints() {
        assert!(resolve_upstream_is_stream(
            false,
            false,
            UpstreamStreamPolicy::ForceStream
        ));
        assert!(!resolve_upstream_is_stream(
            true,
            false,
            UpstreamStreamPolicy::ForceNonStream
        ));
        assert!(resolve_upstream_is_stream(
            true,
            true,
            UpstreamStreamPolicy::ForceNonStream
        ));
        assert!(!resolve_upstream_is_stream(
            false,
            false,
            UpstreamStreamPolicy::Auto
        ));
        assert!(resolve_upstream_is_stream(
            true,
            false,
            UpstreamStreamPolicy::Auto
        ));
        assert!(resolve_upstream_is_stream(
            false,
            true,
            UpstreamStreamPolicy::Auto
        ));
    }

    #[test]
    fn resolves_endpoint_policy_config_to_upstream_mode() {
        assert!(resolve_upstream_is_stream_from_endpoint_config(
            Some(&json!({"upstream_stream_policy": "force_stream"})),
            false,
            false,
        ));
        assert!(!resolve_upstream_is_stream_from_endpoint_config(
            Some(&json!({"upstream_stream_policy": "force_non_stream"})),
            true,
            false,
        ));
        assert!(resolve_upstream_is_stream_from_endpoint_config(
            Some(&json!({"upstream_stream_policy": "auto"})),
            true,
            false,
        ));
        assert!(!resolve_upstream_is_stream_from_endpoint_config(
            None, false, false,
        ));
    }
}
