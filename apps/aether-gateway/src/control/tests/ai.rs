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
        Some("claude:messages")
    );
    assert!(!decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_openai_embeddings_as_embedding_not_chat() {
    let headers = headers(&[("authorization", "Bearer sk-test")]);
    let uri: Uri = "/v1/embeddings".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("openai"));
    assert_eq!(decision.route_kind.as_deref(), Some("embedding"));
    assert_ne!(decision.route_kind.as_deref(), Some("chat"));
    assert_ne!(decision.route_kind.as_deref(), Some("responses"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("openai:embedding")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_openai_rerank_as_rerank_not_chat() {
    let headers = headers(&[("authorization", "Bearer sk-test")]);
    let uri: Uri = "/v1/rerank".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("openai"));
    assert_eq!(decision.route_kind.as_deref(), Some("rerank"));
    assert_ne!(decision.route_kind.as_deref(), Some("chat"));
    assert_ne!(decision.route_kind.as_deref(), Some("embedding"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("openai:rerank")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_openai_chat_and_responses_separately_from_embedding() {
    let headers = headers(&[("authorization", "Bearer sk-test")]);
    let chat_uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
    let responses_uri: Uri = "/v1/responses".parse().expect("uri should parse");

    let chat = classify_control_route(&http::Method::POST, &chat_uri, &headers)
        .expect("chat route should classify");
    assert_eq!(chat.route_family.as_deref(), Some("openai"));
    assert_eq!(chat.route_kind.as_deref(), Some("chat"));
    assert_eq!(chat.auth_endpoint_signature.as_deref(), Some("openai:chat"));
    assert_ne!(chat.route_kind.as_deref(), Some("embedding"));

    let responses = classify_control_route(&http::Method::POST, &responses_uri, &headers)
        .expect("responses route should classify");
    assert_eq!(responses.route_family.as_deref(), Some("openai"));
    assert_eq!(responses.route_kind.as_deref(), Some("responses"));
    assert_eq!(
        responses.auth_endpoint_signature.as_deref(),
        Some("openai:responses")
    );
    assert_ne!(responses.route_kind.as_deref(), Some("embedding"));
}

#[test]
fn classifies_openai_image_generation_and_edit_but_not_variation() {
    let headers = headers(&[("authorization", "Bearer sk-test")]);

    for path in ["/v1/images/generations", "/v1/images/edits"] {
        let uri: Uri = path.parse().expect("uri should parse");
        let decision = classify_control_route(&http::Method::POST, &uri, &headers)
            .expect("image route should classify");

        assert_eq!(decision.route_family.as_deref(), Some("openai"));
        assert_eq!(decision.route_kind.as_deref(), Some("image"));
        assert_eq!(
            decision.auth_endpoint_signature.as_deref(),
            Some("openai:image")
        );
        assert!(decision.is_execution_runtime_candidate());
    }

    let variation_uri: Uri = "/v1/images/variations".parse().expect("uri should parse");
    assert!(classify_control_route(&http::Method::POST, &variation_uri, &headers).is_none());
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
        Some("claude:messages")
    );
}

#[test]
fn classifies_claude_messages_cli_when_bearer_without_api_key() {
    let headers = headers(&[("authorization", "Bearer token-123")]);
    let uri: Uri = "/v1/messages".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("claude"));
    assert_eq!(decision.route_kind.as_deref(), Some("messages"));
    assert_eq!(
        decision.request_auth_channel.as_deref(),
        Some("bearer_like")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:messages")
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
    assert_eq!(decision.route_kind.as_deref(), Some("messages"));
    assert_eq!(
        decision.request_auth_channel.as_deref(),
        Some("bearer_like")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:messages")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_claude_messages_when_api_key_without_bearer() {
    let headers = headers(&[("x-api-key", "sk-client")]);
    let uri: Uri = "/v1/messages".parse().expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("claude"));
    assert_eq!(decision.route_kind.as_deref(), Some("messages"));
    assert_eq!(decision.request_auth_channel.as_deref(), Some("api_key"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("claude:messages")
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
    assert_eq!(decision.route_kind.as_deref(), Some("generate_content"));
    assert_eq!(
        decision.request_auth_channel.as_deref(),
        Some("bearer_like")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("gemini:generate_content")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_gemini_generate_content_api_key_without_cli_marker() {
    let headers = headers(&[("x-goog-api-key", "gemini-key")]);
    let uri: Uri = "/v1beta/models/gemini-2.5-pro:generateContent"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("gemini"));
    assert_eq!(decision.route_kind.as_deref(), Some("generate_content"));
    assert_eq!(decision.request_auth_channel.as_deref(), Some("api_key"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("gemini:generate_content")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn does_not_classify_gemini_options_preflight_as_ai_execution_route() {
    let headers = headers(&[
        ("origin", "http://localhost:3000"),
        ("access-control-request-method", "POST"),
        (
            "access-control-request-headers",
            "authorization,content-type",
        ),
    ]);
    let uri: Uri = "/v1beta/models/gemini-3-flash-preview:streamGenerateContent?alt=sse"
        .parse()
        .expect("uri should parse");

    assert!(classify_control_route(&http::Method::OPTIONS, &uri, &headers).is_none());
}

#[test]
fn classifies_gemini_embed_content_as_embedding_route() {
    let headers = headers(&[("x-goog-api-key", "gemini-key")]);
    let uri: Uri = "/v1beta/models/gemini-embedding-2-preview:embedContent"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("gemini"));
    assert_eq!(decision.route_kind.as_deref(), Some("embedding"));
    assert_eq!(decision.request_auth_channel.as_deref(), Some("api_key"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("gemini:embedding")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn classifies_gemini_batch_embed_contents_as_embedding_route() {
    let headers = headers(&[("x-goog-api-key", "gemini-key")]);
    let uri: Uri = "/v1beta/models/gemini-embedding-2-preview:batchEmbedContents"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_family.as_deref(), Some("gemini"));
    assert_eq!(decision.route_kind.as_deref(), Some("embedding"));
    assert_eq!(decision.request_auth_channel.as_deref(), Some("api_key"));
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("gemini:embedding")
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

#[test]
fn classifies_antigravity_v1internal_control_plane_routes() {
    let headers = headers(&[
        ("authorization", "Bearer ant-access-token"),
        ("user-agent", "antigravity/cli/1.0.2 linux/arm64"),
    ]);

    for (path, route_kind) in [
        ("/v1internal:loadCodeAssist", "load_code_assist"),
        ("/v1internal:fetchAvailableModels", "fetch_available_models"),
        ("/v1internal:fetchUserInfo", "fetch_user_info"),
        ("/v1internal:fetchAdminControls", "fetch_admin_controls"),
        ("/v1internal:setUserSettings", "set_user_settings"),
        ("/v1internal:listExperiments", "list_experiments"),
        (
            "/v1internal:recordCodeAssistMetrics",
            "record_code_assist_metrics",
        ),
    ] {
        let uri: Uri = path.parse().expect("uri should parse");
        let decision = classify_control_route(&http::Method::POST, &uri, &headers)
            .expect("route should classify");

        assert_eq!(decision.route_class.as_deref(), Some("ai_public"));
        assert_eq!(decision.route_family.as_deref(), Some("antigravity"));
        assert_eq!(decision.route_kind.as_deref(), Some(route_kind));
        assert_eq!(
            decision.request_auth_channel.as_deref(),
            Some("bearer_like")
        );
        assert_eq!(
            decision.auth_endpoint_signature.as_deref(),
            Some("antigravity:v1internal")
        );
        assert!(
            !decision.is_execution_runtime_candidate(),
            "control-plane route {path} must be handled by local facade before execution runtime"
        );
    }
}

#[test]
fn classifies_antigravity_stream_generate_content_as_execution_route() {
    let headers = headers(&[
        ("authorization", "Bearer ant-access-token"),
        ("user-agent", "antigravity/cli/1.0.2 linux/arm64"),
    ]);
    let uri: Uri = "/v1internal:streamGenerateContent?alt=sse"
        .parse()
        .expect("uri should parse");
    let decision =
        classify_control_route(&http::Method::POST, &uri, &headers).expect("route should classify");

    assert_eq!(decision.route_class.as_deref(), Some("ai_public"));
    assert_eq!(decision.route_family.as_deref(), Some("antigravity"));
    assert_eq!(
        decision.route_kind.as_deref(),
        Some("stream_generate_content")
    );
    assert_eq!(
        decision.request_auth_channel.as_deref(),
        Some("bearer_like")
    );
    assert_eq!(
        decision.auth_endpoint_signature.as_deref(),
        Some("antigravity:v1internal")
    );
    assert!(decision.is_execution_runtime_candidate());
}

#[test]
fn rejects_unknown_antigravity_v1internal_route() {
    let headers = headers(&[
        ("authorization", "Bearer ant-access-token"),
        ("user-agent", "antigravity/cli/1.0.2 linux/arm64"),
    ]);
    let uri: Uri = "/v1internal:deleteEverything"
        .parse()
        .expect("uri should parse");

    assert!(classify_control_route(&http::Method::POST, &uri, &headers).is_none());
}
