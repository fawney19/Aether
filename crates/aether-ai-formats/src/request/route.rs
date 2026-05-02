use http::Method;

use crate::contracts::{
    CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
    CLAUDE_CLI_SYNC_PLAN_KIND, GEMINI_CHAT_STREAM_PLAN_KIND, GEMINI_CHAT_SYNC_PLAN_KIND,
    GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND, GEMINI_FILES_DELETE_PLAN_KIND,
    GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND, GEMINI_FILES_LIST_PLAN_KIND,
    GEMINI_FILES_UPLOAD_PLAN_KIND, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND,
    OPENAI_IMAGE_STREAM_PLAN_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND,
    OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND, OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND,
    OPENAI_RESPONSES_STREAM_PLAN_KIND, OPENAI_RESPONSES_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_CONTENT_PLAN_KIND,
    OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::request::specialized::image::is_openai_image_stream_request;

pub fn resolve_execution_runtime_stream_plan_kind(
    route_class: Option<&str>,
    route_family: Option<&str>,
    route_kind: Option<&str>,
    request_auth_channel: Option<&str>,
    method: &Method,
    path: &str,
) -> Option<&'static str> {
    if route_class != Some("ai_public") {
        return None;
    }

    if route_family == Some("gemini")
        && route_kind == Some("files")
        && *method == Method::GET
        && path.ends_with(":download")
    {
        return Some(GEMINI_FILES_DOWNLOAD_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("chat")
        && *method == Method::POST
        && path == "/v1/chat/completions"
    {
        return Some(OPENAI_CHAT_STREAM_PLAN_KIND);
    }

    if route_family == Some("claude")
        && is_claude_messages_route_kind(route_kind)
        && *method == Method::POST
        && path == "/v1/messages"
    {
        return Some(resolve_claude_messages_plan_kind(
            request_auth_channel,
            CLAUDE_CHAT_STREAM_PLAN_KIND,
            CLAUDE_CLI_STREAM_PLAN_KIND,
        ));
    }

    if route_family == Some("gemini")
        && is_gemini_generate_content_route_kind(route_kind)
        && *method == Method::POST
        && path.ends_with(":streamGenerateContent")
    {
        return Some(resolve_gemini_generate_content_plan_kind(
            request_auth_channel,
            GEMINI_CHAT_STREAM_PLAN_KIND,
            GEMINI_CLI_STREAM_PLAN_KIND,
        ));
    }

    if route_family == Some("openai")
        && is_openai_responses_route_kind(route_kind)
        && *method == Method::POST
        && path == "/v1/responses"
    {
        return Some(OPENAI_RESPONSES_STREAM_PLAN_KIND);
    }

    if route_family == Some("openai")
        && is_openai_responses_compact_route_kind(route_kind)
        && *method == Method::POST
        && path == "/v1/responses/compact"
    {
        return Some(OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("image")
        && *method == Method::POST
        && matches!(path, "/v1/images/generations" | "/v1/images/edits")
    {
        return Some(OPENAI_IMAGE_STREAM_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("video")
        && *method == Method::GET
        && path.ends_with("/content")
    {
        return Some(OPENAI_VIDEO_CONTENT_PLAN_KIND);
    }

    None
}

pub fn resolve_execution_runtime_sync_plan_kind(
    route_class: Option<&str>,
    route_family: Option<&str>,
    route_kind: Option<&str>,
    request_auth_channel: Option<&str>,
    method: &Method,
    path: &str,
) -> Option<&'static str> {
    if route_class != Some("ai_public") {
        return None;
    }

    if route_family == Some("openai")
        && route_kind == Some("video")
        && *method == Method::POST
        && path.starts_with("/v1/videos/")
        && path.ends_with("/cancel")
    {
        return Some(OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("video")
        && *method == Method::POST
        && path.starts_with("/v1/videos/")
        && path.ends_with("/remix")
    {
        return Some(OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("video")
        && *method == Method::POST
        && path == "/v1/videos"
    {
        return Some(OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("video")
        && *method == Method::DELETE
        && path.starts_with("/v1/videos/")
    {
        return Some(OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND);
    }

    if route_family == Some("gemini")
        && route_kind == Some("video")
        && *method == Method::POST
        && path.ends_with(":cancel")
    {
        return Some(GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND);
    }

    if route_family == Some("gemini")
        && route_kind == Some("video")
        && *method == Method::POST
        && path.ends_with(":predictLongRunning")
    {
        return Some(GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("chat")
        && *method == Method::POST
        && path == "/v1/chat/completions"
    {
        return Some(OPENAI_CHAT_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("image")
        && *method == Method::POST
        && matches!(
            path,
            "/v1/images/generations" | "/v1/images/edits" | "/v1/images/variations"
        )
    {
        return Some(OPENAI_IMAGE_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && is_openai_responses_route_kind(route_kind)
        && *method == Method::POST
        && path == "/v1/responses"
    {
        return Some(OPENAI_RESPONSES_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && is_openai_responses_compact_route_kind(route_kind)
        && *method == Method::POST
        && path == "/v1/responses/compact"
    {
        return Some(OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND);
    }

    if route_family == Some("claude")
        && is_claude_messages_route_kind(route_kind)
        && *method == Method::POST
        && path == "/v1/messages"
    {
        return Some(resolve_claude_messages_plan_kind(
            request_auth_channel,
            CLAUDE_CHAT_SYNC_PLAN_KIND,
            CLAUDE_CLI_SYNC_PLAN_KIND,
        ));
    }

    if route_family == Some("gemini")
        && is_gemini_generate_content_route_kind(route_kind)
        && *method == Method::POST
        && path.ends_with(":generateContent")
    {
        return Some(resolve_gemini_generate_content_plan_kind(
            request_auth_channel,
            GEMINI_CHAT_SYNC_PLAN_KIND,
            GEMINI_CLI_SYNC_PLAN_KIND,
        ));
    }

    if route_family == Some("gemini") && route_kind == Some("files") {
        if *method == Method::POST && path == "/upload/v1beta/files" {
            return Some(GEMINI_FILES_UPLOAD_PLAN_KIND);
        }
        if *method == Method::GET && path == "/v1beta/files" {
            return Some(GEMINI_FILES_LIST_PLAN_KIND);
        }
        if *method == Method::GET
            && path.starts_with("/v1beta/files/")
            && !path.ends_with(":download")
        {
            return Some(GEMINI_FILES_GET_PLAN_KIND);
        }
        if *method == Method::DELETE
            && path.starts_with("/v1beta/files/")
            && !path.ends_with(":download")
        {
            return Some(GEMINI_FILES_DELETE_PLAN_KIND);
        }
    }

    None
}

fn is_openai_responses_route_kind(route_kind: Option<&str>) -> bool {
    matches!(route_kind, Some("responses") | Some("cli"))
}

fn is_openai_responses_compact_route_kind(route_kind: Option<&str>) -> bool {
    matches!(route_kind, Some("responses:compact") | Some("compact"))
}

fn is_claude_messages_route_kind(route_kind: Option<&str>) -> bool {
    matches!(route_kind, Some("messages") | Some("chat"))
}

fn is_gemini_generate_content_route_kind(route_kind: Option<&str>) -> bool {
    matches!(route_kind, Some("generate_content") | Some("chat"))
}

fn resolve_claude_messages_plan_kind(
    request_auth_channel: Option<&str>,
    chat_plan_kind: &'static str,
    cli_plan_kind: &'static str,
) -> &'static str {
    if request_auth_channel == Some("bearer_like") {
        cli_plan_kind
    } else {
        chat_plan_kind
    }
}

fn resolve_gemini_generate_content_plan_kind(
    request_auth_channel: Option<&str>,
    chat_plan_kind: &'static str,
    cli_plan_kind: &'static str,
) -> &'static str {
    if request_auth_channel == Some("bearer_like") {
        cli_plan_kind
    } else {
        chat_plan_kind
    }
}

pub fn is_matching_stream_request(
    plan_kind: &str,
    path: &str,
    body_json: &serde_json::Value,
) -> bool {
    match plan_kind {
        OPENAI_CHAT_STREAM_PLAN_KIND
        | CLAUDE_CHAT_STREAM_PLAN_KIND
        | OPENAI_RESPONSES_STREAM_PLAN_KIND
        | OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND
        | CLAUDE_CLI_STREAM_PLAN_KIND
        | OPENAI_IMAGE_STREAM_PLAN_KIND => body_json
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        GEMINI_CHAT_STREAM_PLAN_KIND | GEMINI_CLI_STREAM_PLAN_KIND => {
            path.ends_with(":streamGenerateContent")
        }
        _ => true,
    }
}

pub fn is_matching_stream_http_request(
    plan_kind: &str,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
) -> bool {
    if plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND {
        return is_openai_image_stream_request(parts, body_json, body_base64);
    }

    is_matching_stream_request(plan_kind, parts.uri.path(), body_json)
}

pub fn supports_sync_execution_decision_kind(plan_kind: &str) -> bool {
    matches!(
        plan_kind,
        OPENAI_CHAT_SYNC_PLAN_KIND
            | OPENAI_IMAGE_SYNC_PLAN_KIND
            | OPENAI_RESPONSES_SYNC_PLAN_KIND
            | OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND
            | CLAUDE_CHAT_SYNC_PLAN_KIND
            | CLAUDE_CLI_SYNC_PLAN_KIND
            | GEMINI_CHAT_SYNC_PLAN_KIND
            | GEMINI_CLI_SYNC_PLAN_KIND
            | GEMINI_FILES_UPLOAD_PLAN_KIND
            | OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND
            | OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND
            | OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND
            | OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND
            | GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND
            | GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND
            | GEMINI_FILES_GET_PLAN_KIND
            | GEMINI_FILES_LIST_PLAN_KIND
            | GEMINI_FILES_DELETE_PLAN_KIND
    )
}

pub fn supports_stream_execution_decision_kind(plan_kind: &str) -> bool {
    matches!(
        plan_kind,
        OPENAI_CHAT_STREAM_PLAN_KIND
            | CLAUDE_CHAT_STREAM_PLAN_KIND
            | GEMINI_CHAT_STREAM_PLAN_KIND
            | OPENAI_RESPONSES_STREAM_PLAN_KIND
            | OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND
            | OPENAI_IMAGE_STREAM_PLAN_KIND
            | CLAUDE_CLI_STREAM_PLAN_KIND
            | GEMINI_CLI_STREAM_PLAN_KIND
            | GEMINI_FILES_DOWNLOAD_PLAN_KIND
            | OPENAI_VIDEO_CONTENT_PLAN_KIND
    )
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use http::Method;

    use super::{
        is_matching_stream_http_request, is_matching_stream_request,
        resolve_execution_runtime_stream_plan_kind, resolve_execution_runtime_sync_plan_kind,
        supports_stream_execution_decision_kind, supports_sync_execution_decision_kind,
    };
    use crate::contracts::{
        CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
        CLAUDE_CLI_SYNC_PLAN_KIND, GEMINI_CHAT_STREAM_PLAN_KIND, GEMINI_CHAT_SYNC_PLAN_KIND,
        GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND,
        OPENAI_CHAT_SYNC_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND,
        OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND, OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND,
        OPENAI_RESPONSES_STREAM_PLAN_KIND, OPENAI_RESPONSES_SYNC_PLAN_KIND,
    };

    #[test]
    fn resolves_openai_chat_plan_kinds() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("chat"),
                None,
                &Method::POST,
                "/v1/chat/completions",
            ),
            Some(OPENAI_CHAT_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("chat"),
                None,
                &Method::POST,
                "/v1/chat/completions",
            ),
            Some(OPENAI_CHAT_STREAM_PLAN_KIND)
        );
    }

    #[test]
    fn resolves_openai_responses_plan_kinds() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("responses"),
                None,
                &Method::POST,
                "/v1/responses",
            ),
            Some(OPENAI_RESPONSES_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("responses"),
                None,
                &Method::POST,
                "/v1/responses",
            ),
            Some(OPENAI_RESPONSES_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("cli"),
                None,
                &Method::POST,
                "/v1/responses",
            ),
            Some(OPENAI_RESPONSES_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_execution_decision_kind(
            OPENAI_RESPONSES_SYNC_PLAN_KIND
        ));
        assert!(supports_stream_execution_decision_kind(
            OPENAI_RESPONSES_STREAM_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_openai_responses_compact_plan_kinds() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("responses:compact"),
                None,
                &Method::POST,
                "/v1/responses/compact",
            ),
            Some(OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("responses:compact"),
                None,
                &Method::POST,
                "/v1/responses/compact",
            ),
            Some(OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("compact"),
                None,
                &Method::POST,
                "/v1/responses/compact",
            ),
            Some(OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_execution_decision_kind(
            OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND
        ));
        assert!(supports_stream_execution_decision_kind(
            OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_claude_messages_plan_kinds_by_request_auth_channel() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("claude"),
                Some("messages"),
                Some("api_key"),
                &Method::POST,
                "/v1/messages",
            ),
            Some(CLAUDE_CHAT_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("claude"),
                Some("messages"),
                Some("api_key"),
                &Method::POST,
                "/v1/messages",
            ),
            Some(CLAUDE_CHAT_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("claude"),
                Some("messages"),
                Some("bearer_like"),
                &Method::POST,
                "/v1/messages",
            ),
            Some(CLAUDE_CLI_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("claude"),
                Some("messages"),
                Some("bearer_like"),
                &Method::POST,
                "/v1/messages",
            ),
            Some(CLAUDE_CLI_STREAM_PLAN_KIND)
        );
    }

    #[test]
    fn resolves_gemini_generate_content_plan_kinds_by_request_auth_channel() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("generate_content"),
                Some("api_key"),
                &Method::POST,
                "/v1beta/models/gemini-2.5-pro:generateContent",
            ),
            Some(GEMINI_CHAT_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("generate_content"),
                Some("api_key"),
                &Method::POST,
                "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
            ),
            Some(GEMINI_CHAT_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("generate_content"),
                Some("bearer_like"),
                &Method::POST,
                "/v1beta/models/gemini-2.5-pro:generateContent",
            ),
            Some(GEMINI_CLI_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("generate_content"),
                Some("bearer_like"),
                &Method::POST,
                "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
            ),
            Some(GEMINI_CLI_STREAM_PLAN_KIND)
        );
    }

    #[test]
    fn stream_matching_requires_openai_stream_flag() {
        assert!(!is_matching_stream_request(
            OPENAI_CHAT_STREAM_PLAN_KIND,
            "/v1/chat/completions",
            &serde_json::json!({"stream": false}),
        ));
        assert!(is_matching_stream_request(
            OPENAI_CHAT_STREAM_PLAN_KIND,
            "/v1/chat/completions",
            &serde_json::json!({"stream": true}),
        ));
        assert!(supports_sync_execution_decision_kind(
            OPENAI_CHAT_SYNC_PLAN_KIND
        ));
        assert!(supports_stream_execution_decision_kind(
            OPENAI_CHAT_STREAM_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_openai_image_sync_plan_kind() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("image"),
                None,
                &Method::POST,
                "/v1/images/generations",
            ),
            Some(OPENAI_IMAGE_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("image"),
                None,
                &Method::POST,
                "/v1/images/edits",
            ),
            Some(OPENAI_IMAGE_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("image"),
                None,
                &Method::POST,
                "/v1/images/variations",
            ),
            Some(OPENAI_IMAGE_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_execution_decision_kind(
            OPENAI_IMAGE_SYNC_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_openai_image_stream_plan_kind() {
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("image"),
                None,
                &Method::POST,
                "/v1/images/generations",
            ),
            Some(OPENAI_IMAGE_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("image"),
                None,
                &Method::POST,
                "/v1/images/edits",
            ),
            Some(OPENAI_IMAGE_STREAM_PLAN_KIND)
        );
        assert!(supports_stream_execution_decision_kind(
            OPENAI_IMAGE_STREAM_PLAN_KIND
        ));
    }

    #[test]
    fn stream_matching_requires_openai_image_stream_flag() {
        assert!(!is_matching_stream_request(
            OPENAI_IMAGE_STREAM_PLAN_KIND,
            "/v1/images/generations",
            &serde_json::json!({"stream": false}),
        ));
        assert!(is_matching_stream_request(
            OPENAI_IMAGE_STREAM_PLAN_KIND,
            "/v1/images/generations",
            &serde_json::json!({"stream": true}),
        ));
    }

    #[test]
    fn http_stream_matching_detects_openai_image_multipart_stream_flag() {
        let request = http::Request::builder()
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

        assert!(is_matching_stream_http_request(
            OPENAI_IMAGE_STREAM_PLAN_KIND,
            &parts,
            &serde_json::json!({}),
            Some(body_base64.as_str()),
        ));
    }
}
