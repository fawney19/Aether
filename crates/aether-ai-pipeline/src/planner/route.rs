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

pub fn resolve_execution_runtime_stream_plan_kind(
    route_class: Option<&str>,
    route_family: Option<&str>,
    route_kind: Option<&str>,
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
        && route_kind == Some("chat")
        && *method == Method::POST
        && path == "/v1/messages"
    {
        return Some(CLAUDE_CHAT_STREAM_PLAN_KIND);
    }

    if route_family == Some("claude")
        && route_kind == Some("cli")
        && *method == Method::POST
        && path == "/v1/messages"
    {
        return Some(CLAUDE_CLI_STREAM_PLAN_KIND);
    }

    if route_family == Some("gemini")
        && route_kind == Some("chat")
        && *method == Method::POST
        && path.ends_with(":streamGenerateContent")
    {
        return Some(GEMINI_CHAT_STREAM_PLAN_KIND);
    }

    if route_family == Some("gemini")
        && route_kind == Some("cli")
        && *method == Method::POST
        && path.ends_with(":streamGenerateContent")
    {
        return Some(GEMINI_CLI_STREAM_PLAN_KIND);
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
        && route_kind == Some("chat")
        && *method == Method::POST
        && path == "/v1/messages"
    {
        return Some(CLAUDE_CHAT_SYNC_PLAN_KIND);
    }

    if route_family == Some("claude")
        && route_kind == Some("cli")
        && *method == Method::POST
        && path == "/v1/messages"
    {
        return Some(CLAUDE_CLI_SYNC_PLAN_KIND);
    }

    if route_family == Some("gemini")
        && route_kind == Some("chat")
        && *method == Method::POST
        && path.ends_with(":generateContent")
    {
        return Some(GEMINI_CHAT_SYNC_PLAN_KIND);
    }

    if route_family == Some("gemini")
        && route_kind == Some("cli")
        && *method == Method::POST
        && path.ends_with(":generateContent")
    {
        return Some(GEMINI_CLI_SYNC_PLAN_KIND);
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

pub fn supports_sync_scheduler_decision_kind(plan_kind: &str) -> bool {
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

pub fn supports_stream_scheduler_decision_kind(plan_kind: &str) -> bool {
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
    use http::Method;

    use super::{
        is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
        resolve_execution_runtime_sync_plan_kind, supports_stream_scheduler_decision_kind,
        supports_sync_scheduler_decision_kind,
    };
    use crate::contracts::{
        OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND,
        OPENAI_IMAGE_SYNC_PLAN_KIND, OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND,
        OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND, OPENAI_RESPONSES_STREAM_PLAN_KIND,
        OPENAI_RESPONSES_SYNC_PLAN_KIND,
    };

    #[test]
    fn resolves_openai_chat_plan_kinds() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("chat"),
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
                &Method::POST,
                "/v1/responses",
            ),
            Some(OPENAI_RESPONSES_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_scheduler_decision_kind(
            OPENAI_RESPONSES_SYNC_PLAN_KIND
        ));
        assert!(supports_stream_scheduler_decision_kind(
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
                &Method::POST,
                "/v1/responses/compact",
            ),
            Some(OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_scheduler_decision_kind(
            OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND
        ));
        assert!(supports_stream_scheduler_decision_kind(
            OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND
        ));
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
        assert!(supports_sync_scheduler_decision_kind(
            OPENAI_CHAT_SYNC_PLAN_KIND
        ));
        assert!(supports_stream_scheduler_decision_kind(
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
                &Method::POST,
                "/v1/images/variations",
            ),
            Some(OPENAI_IMAGE_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_scheduler_decision_kind(
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
                &Method::POST,
                "/v1/images/edits",
            ),
            Some(OPENAI_IMAGE_STREAM_PLAN_KIND)
        );
        assert!(supports_stream_scheduler_decision_kind(
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
}
