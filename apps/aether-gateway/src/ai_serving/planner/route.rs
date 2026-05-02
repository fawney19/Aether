use crate::ai_serving::GatewayControlDecision;
use crate::ai_serving::{
    is_matching_stream_http_request as is_matching_stream_http_request_impl,
    resolve_execution_runtime_stream_plan_kind as resolve_execution_runtime_stream_plan_kind_impl,
    resolve_execution_runtime_sync_plan_kind as resolve_execution_runtime_sync_plan_kind_impl,
    supports_stream_execution_decision_kind as supports_stream_execution_decision_kind_impl,
    supports_sync_execution_decision_kind as supports_sync_execution_decision_kind_impl,
};

pub(crate) fn resolve_execution_runtime_stream_plan_kind(
    parts: &http::request::Parts,
    decision: &GatewayControlDecision,
) -> Option<&'static str> {
    resolve_execution_runtime_stream_plan_kind_impl(
        decision.route_class.as_deref(),
        decision.route_family.as_deref(),
        decision.route_kind.as_deref(),
        decision.request_auth_channel.as_deref(),
        &parts.method,
        parts.uri.path(),
    )
}

pub(crate) fn resolve_execution_runtime_sync_plan_kind(
    parts: &http::request::Parts,
    decision: &GatewayControlDecision,
) -> Option<&'static str> {
    resolve_execution_runtime_sync_plan_kind_impl(
        decision.route_class.as_deref(),
        decision.route_family.as_deref(),
        decision.route_kind.as_deref(),
        decision.request_auth_channel.as_deref(),
        &parts.method,
        parts.uri.path(),
    )
}

pub(crate) fn is_matching_stream_request(
    plan_kind: &str,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
) -> bool {
    is_matching_stream_http_request_impl(plan_kind, parts, body_json, body_base64)
}

pub(crate) fn supports_sync_execution_decision_kind(plan_kind: &str) -> bool {
    supports_sync_execution_decision_kind_impl(plan_kind)
}

pub(crate) fn supports_stream_execution_decision_kind(plan_kind: &str) -> bool {
    supports_stream_execution_decision_kind_impl(plan_kind)
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, Request};
    use base64::Engine as _;

    use super::{
        is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
        resolve_execution_runtime_sync_plan_kind, supports_stream_execution_decision_kind,
        supports_sync_execution_decision_kind,
    };
    use crate::ai_serving::GatewayControlDecision;

    fn sample_decision(route_family: &str, route_kind: &str) -> GatewayControlDecision {
        GatewayControlDecision {
            public_path: "/".to_string(),
            public_query_string: None,
            route_class: Some("ai_public".to_string()),
            route_family: Some(route_family.to_string()),
            route_kind: Some(route_kind.to_string()),
            request_auth_channel: None,
            auth_context: None,
            admin_principal: None,
            auth_endpoint_signature: None,
            execution_runtime_candidate: true,
            local_auth_rejection: None,
        }
    }

    fn sample_decision_with_auth_channel(
        route_family: &str,
        route_kind: &str,
        request_auth_channel: &str,
    ) -> GatewayControlDecision {
        let mut decision = sample_decision(route_family, route_kind);
        decision.request_auth_channel = Some(request_auth_channel.to_string());
        decision
    }

    #[test]
    fn resolves_openai_chat_plan_kinds_via_format_crate() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/v1/chat/completions")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let decision = sample_decision("openai", "chat");

        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(&parts, &decision),
            Some("openai_chat_sync")
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(&parts, &decision),
            Some("openai_chat_stream")
        );
    }

    #[test]
    fn resolves_endpoint_route_kinds_by_request_auth_channel_via_format_crate() {
        let claude_request = Request::builder()
            .method(Method::POST)
            .uri("/v1/messages")
            .body(())
            .expect("request should build");
        let (claude_parts, _) = claude_request.into_parts();

        let claude_api_key = sample_decision_with_auth_channel("claude", "messages", "api_key");
        let claude_bearer = sample_decision_with_auth_channel("claude", "messages", "bearer_like");
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(&claude_parts, &claude_api_key),
            Some("claude_chat_sync")
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(&claude_parts, &claude_bearer),
            Some("claude_cli_stream")
        );

        let gemini_request = Request::builder()
            .method(Method::POST)
            .uri("/v1beta/models/gemini-2.5-pro:generateContent")
            .body(())
            .expect("request should build");
        let (gemini_parts, _) = gemini_request.into_parts();

        let gemini_api_key =
            sample_decision_with_auth_channel("gemini", "generate_content", "api_key");
        let gemini_bearer =
            sample_decision_with_auth_channel("gemini", "generate_content", "bearer_like");
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(&gemini_parts, &gemini_api_key),
            Some("gemini_chat_sync")
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(&gemini_parts, &gemini_bearer),
            Some("gemini_cli_sync")
        );
    }

    #[test]
    fn stream_matching_uses_surface_route_logic() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/v1/chat/completions")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();

        assert!(!is_matching_stream_request(
            "openai_chat_stream",
            &parts,
            &serde_json::json!({"stream": false}),
            None,
        ));
        assert!(is_matching_stream_request(
            "openai_chat_stream",
            &parts,
            &serde_json::json!({"stream": true}),
            None,
        ));
        assert!(supports_sync_execution_decision_kind("openai_chat_sync"));
        assert!(supports_stream_execution_decision_kind(
            "openai_chat_stream"
        ));
    }

    #[test]
    fn image_stream_matching_parses_multipart_stream_flag() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/v1/images/edits")
            .header(
                http::header::CONTENT_TYPE,
                "multipart/form-data; boundary=image-stream-boundary",
            )
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let body = concat!(
            "--image-stream-boundary\r\n",
            "Content-Disposition: form-data; name=\"stream\"\r\n\r\n",
            "true\r\n",
            "--image-stream-boundary--\r\n"
        );
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());

        assert!(is_matching_stream_request(
            "openai_image_stream",
            &parts,
            &serde_json::json!({}),
            Some(body_base64.as_str()),
        ));
    }
}
