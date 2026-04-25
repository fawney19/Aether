use super::specialized::is_openai_image_stream_request;
use crate::ai_pipeline::GatewayControlDecision;
use crate::ai_pipeline::{
    is_matching_stream_request as is_matching_stream_request_impl,
    resolve_execution_runtime_stream_plan_kind as resolve_execution_runtime_stream_plan_kind_impl,
    resolve_execution_runtime_sync_plan_kind as resolve_execution_runtime_sync_plan_kind_impl,
    supports_stream_scheduler_decision_kind as supports_stream_scheduler_decision_kind_impl,
    supports_sync_scheduler_decision_kind as supports_sync_scheduler_decision_kind_impl,
    OPENAI_IMAGE_STREAM_PLAN_KIND,
};

pub(crate) fn resolve_execution_runtime_stream_plan_kind(
    parts: &http::request::Parts,
    decision: &GatewayControlDecision,
) -> Option<&'static str> {
    resolve_execution_runtime_stream_plan_kind_impl(
        decision.route_class.as_deref(),
        decision.route_family.as_deref(),
        decision.route_kind.as_deref(),
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
    if plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND {
        return is_openai_image_stream_request(parts, body_json, body_base64);
    }
    is_matching_stream_request_impl(plan_kind, parts.uri.path(), body_json)
}

pub(crate) fn supports_sync_scheduler_decision_kind(plan_kind: &str) -> bool {
    supports_sync_scheduler_decision_kind_impl(plan_kind)
}

pub(crate) fn supports_stream_scheduler_decision_kind(plan_kind: &str) -> bool {
    supports_stream_scheduler_decision_kind_impl(plan_kind)
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, Request};
    use base64::Engine as _;

    use super::{
        is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
        resolve_execution_runtime_sync_plan_kind, supports_stream_scheduler_decision_kind,
        supports_sync_scheduler_decision_kind,
    };
    use crate::ai_pipeline::GatewayControlDecision;

    fn sample_decision(route_family: &str, route_kind: &str) -> GatewayControlDecision {
        GatewayControlDecision {
            public_path: "/".to_string(),
            public_query_string: None,
            route_class: Some("ai_public".to_string()),
            route_family: Some(route_family.to_string()),
            route_kind: Some(route_kind.to_string()),
            auth_context: None,
            admin_principal: None,
            auth_endpoint_signature: None,
            execution_runtime_candidate: true,
            local_auth_rejection: None,
        }
    }

    #[test]
    fn resolves_openai_chat_plan_kinds_via_pipeline_crate() {
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
    fn stream_matching_uses_pipeline_route_logic() {
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
        assert!(supports_sync_scheduler_decision_kind("openai_chat_sync"));
        assert!(supports_stream_scheduler_decision_kind(
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
