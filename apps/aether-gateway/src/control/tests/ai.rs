use http::Uri;

use super::{classify_control_route, headers};

#[test]
fn classifies_claude_count_tokens_as_non_execution_runtime_public_route() {
    let headers = headers(&[("x-api-key", "sk-test")]);
    let uri: Uri = "/v1/messages/count_tokens"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("claude"));
    assert_eq!(decision.route_kind.as_deref(), Some("count_tokens"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:chat")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_models_list_as_claude_when_headers_match() {
    let headers = headers(&[
        ("x-api-key", "sk-claude"),
        ("anthropic-version", "2023-06-01"),
    ]);
    let uri: Uri = "/v1/models".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::GET, &uri, &headers).expect("route should classify");

    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:chat")
    );
}

#[test]
fn classifies_claude_messages_cli_when_bearer_without_api_key() {
    let headers = headers(&[("authorization", "Bearer token-123")]);
    let uri: Uri = "/v1/messages".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("claude"));
    assert_eq!(decision.route_kind.as_deref(), Some("cli"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:cli")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_claude_messages_cli_when_bearer_is_present_even_with_api_key() {
    let headers = headers(&[
        ("authorization", "Bearer token-123"),
        ("x-api-key", "sk-client"),
    ]);
    let uri: Uri = "/v1/messages".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("claude"));
    assert_eq!(decision.route_kind.as_deref(), Some("cli"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:cli")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_claude_messages_chat_when_api_key_without_bearer() {
    let headers = headers(&[("x-api-key", "sk-client")]);
    let uri: Uri = "/v1/messages".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("claude"));
    assert_eq!(decision.route_kind.as_deref(), Some("chat"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:chat")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_gemini_cli_generate_content_when_x_app_contains_cli() {
    let headers = headers(&[("x-app", "Gemini-CLI")]);
    let uri: Uri = "/v1beta/models/gemini-2.5-pro:generateContent"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("gemini"));
    assert_eq!(decision.route_kind.as_deref(), Some("cli"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("gemini:cli")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_gemini_predict_long_running_as_video_route() {
    let headers = headers(&[]);
    let uri: Uri = "/v1beta/models/veo-3:predictLongRunning"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("gemini"));
    assert_eq!(decision.route_kind.as_deref(), Some("video"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("gemini:video")
    );
    assert!(decision.is_execution_runtime_candidate());
}
