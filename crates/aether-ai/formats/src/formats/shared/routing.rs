use http::Method;
use url::form_urlencoded;

use crate::contracts::{
    ClientSurface, CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND,
    CLAUDE_CLI_STREAM_PLAN_KIND, CLAUDE_CLI_SYNC_PLAN_KIND, CLAUDE_COUNT_TOKENS_SYNC_PLAN_KIND,
    CODEX_LIVE_STREAM_PLAN_KIND, GEMINI_CHAT_STREAM_PLAN_KIND, GEMINI_CHAT_SYNC_PLAN_KIND,
    GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND, GEMINI_EMBEDDING_SYNC_PLAN_KIND,
    GEMINI_FILES_DELETE_PLAN_KIND, GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND,
    GEMINI_FILES_LIST_PLAN_KIND, GEMINI_FILES_UPLOAD_PLAN_KIND,
    GEMINI_INTERACTIONS_STREAM_PLAN_KIND, GEMINI_INTERACTIONS_SYNC_PLAN_KIND,
    GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND, GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND,
    OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND, OPENAI_EMBEDDING_SYNC_PLAN_KIND,
    OPENAI_IMAGE_STREAM_PLAN_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND, OPENAI_REALTIME_STREAM_PLAN_KIND,
    OPENAI_RERANK_SYNC_PLAN_KIND, OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND,
    OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND, OPENAI_RESPONSES_STREAM_PLAN_KIND,
    OPENAI_RESPONSES_SYNC_PLAN_KIND, OPENAI_SEARCH_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_CONTENT_PLAN_KIND,
    OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::formats::openai::image::request::is_openai_image_stream_request;

pub fn resolve_execution_runtime_stream_plan_kind(
    route_class: Option<&str>,
    route_family: Option<&str>,
    route_kind: Option<&str>,
    request_auth_channel: Option<&str>,
    method: &Method,
    path: &str,
) -> Option<&'static str> {
    resolve_execution_runtime_stream_plan_kind_with_client_surface(
        route_class,
        route_family,
        route_kind,
        legacy_client_surface_from_auth_channel(request_auth_channel),
        request_auth_channel,
        method,
        path,
    )
}

pub fn resolve_execution_runtime_stream_plan_kind_with_client_surface(
    route_class: Option<&str>,
    route_family: Option<&str>,
    route_kind: Option<&str>,
    client_surface: Option<ClientSurface>,
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
            client_surface,
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

    if route_family == Some("gemini")
        && route_kind == Some("interactions")
        && *method == Method::POST
        && matches!(path, "/v1/interactions" | "/v1beta/interactions")
    {
        return Some(GEMINI_INTERACTIONS_STREAM_PLAN_KIND);
    }

    if route_family == Some("antigravity")
        && route_kind == Some("stream_generate_content")
        && *method == Method::POST
        && path == "/v1internal:streamGenerateContent"
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
        && route_kind == Some("realtime")
        && *method == Method::GET
        && path == "/v1/realtime"
    {
        return Some(OPENAI_REALTIME_STREAM_PLAN_KIND);
    }

    if route_family == Some("codex")
        && route_kind == Some("live")
        && ((*method == Method::GET
            && (path == "/v1/live" || path.starts_with("/v1/live/") || path == "/v1/realtime"))
            || (*method == Method::POST && matches!(path, "/v1/live" | "/v1/realtime/calls")))
    {
        return Some(CODEX_LIVE_STREAM_PLAN_KIND);
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
    resolve_execution_runtime_sync_plan_kind_with_client_surface(
        route_class,
        route_family,
        route_kind,
        legacy_client_surface_from_auth_channel(request_auth_channel),
        request_auth_channel,
        method,
        path,
    )
}

pub fn resolve_execution_runtime_sync_plan_kind_with_client_surface(
    route_class: Option<&str>,
    route_family: Option<&str>,
    route_kind: Option<&str>,
    client_surface: Option<ClientSurface>,
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

    if route_family == Some("gemini")
        && route_kind == Some("embedding")
        && *method == Method::POST
        && (path.ends_with(":embedContent") || path.ends_with(":batchEmbedContents"))
    {
        return Some(GEMINI_EMBEDDING_SYNC_PLAN_KIND);
    }

    if route_family == Some("gemini")
        && route_kind == Some("interactions")
        && *method == Method::POST
        && matches!(path, "/v1/interactions" | "/v1beta/interactions")
    {
        return Some(GEMINI_INTERACTIONS_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("chat")
        && *method == Method::POST
        && path == "/v1/chat/completions"
    {
        return Some(OPENAI_CHAT_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("embedding")
        && *method == Method::POST
        && path == "/v1/embeddings"
    {
        return Some(OPENAI_EMBEDDING_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("rerank")
        && *method == Method::POST
        && path == "/v1/rerank"
    {
        return Some(OPENAI_RERANK_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("search")
        && *method == Method::POST
        && path == "/v1/alpha/search"
    {
        return Some(OPENAI_SEARCH_SYNC_PLAN_KIND);
    }

    if route_family == Some("openai")
        && route_kind == Some("image")
        && *method == Method::POST
        && matches!(path, "/v1/images/generations" | "/v1/images/edits")
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
        && route_kind == Some("count_tokens")
        && *method == Method::POST
        && path == "/v1/messages/count_tokens"
    {
        return Some(CLAUDE_COUNT_TOKENS_SYNC_PLAN_KIND);
    }

    if route_family == Some("claude")
        && is_claude_messages_route_kind(route_kind)
        && *method == Method::POST
        && path == "/v1/messages"
    {
        return Some(resolve_claude_messages_plan_kind(
            client_surface,
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
    client_surface: Option<ClientSurface>,
    chat_plan_kind: &'static str,
    cli_plan_kind: &'static str,
) -> &'static str {
    if client_surface == Some(ClientSurface::ClaudeCode) {
        cli_plan_kind
    } else {
        chat_plan_kind
    }
}

fn legacy_client_surface_from_auth_channel(
    request_auth_channel: Option<&str>,
) -> Option<ClientSurface> {
    Some(if request_auth_channel == Some("bearer_like") {
        ClientSurface::ClaudeCode
    } else {
        ClientSurface::GenericCompatible
    })
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

pub fn request_path_implies_stream_request(path: &str) -> bool {
    let trimmed = path.trim();
    let path = trimmed
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(trimmed);
    path.ends_with(":streamGenerateContent")
}

pub fn sanitize_request_path(path: &str) -> Option<String> {
    let path = path
        .trim()
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or_else(|| path.trim())
        .trim();
    if path.is_empty() {
        return None;
    }
    if path
        .strip_prefix("/v1/live/")
        .is_some_and(|call_id| !call_id.is_empty())
    {
        // A Live call id identifies an in-progress WebRTC session. Keep this
        // bearer-like capability out of logs and persisted request metadata,
        // including for malformed routes that will later be rejected.
        return Some("/v1/live/{call_id}".to_string());
    }
    Some(sanitize_sensitive_request_path(path))
}

fn sanitize_sensitive_request_path(path: &str) -> String {
    for prefix in ["/install-tunnel/", "/install/", "/i/"] {
        if path
            .strip_prefix(prefix)
            .is_some_and(|secret| !secret.is_empty())
        {
            return format!("{prefix}[redacted]");
        }
    }
    path.to_string()
}

pub fn sanitize_request_query_string(query: &str) -> Option<String> {
    let query = query.trim().trim_start_matches('?').trim();
    if query.is_empty() {
        return None;
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if request_query_key_is_safe_to_trace(key.as_ref()) {
            serializer.append_pair(key.as_ref(), value.as_ref());
        }
    }
    let sanitized = serializer.finish();
    (!sanitized.is_empty()).then_some(sanitized)
}

pub fn sanitize_request_path_and_query(path: &str, query: Option<&str>) -> Option<String> {
    let trimmed = path.trim();
    let (path, embedded_query) = trimmed
        .split_once('?')
        .map(|(path, query)| (path.trim(), Some(query)))
        .unwrap_or((trimmed, None));
    if path.is_empty() {
        return None;
    }

    let sanitized_path = sanitize_request_path(path)?;
    let sanitized_query = query
        .and_then(|query| sanitize_request_query_string_for_path(path, query))
        .or_else(|| {
            embedded_query.and_then(|query| sanitize_request_query_string_for_path(path, query))
        });
    Some(match sanitized_query {
        Some(query) => format!("{sanitized_path}?{query}"),
        None => sanitized_path,
    })
}

fn sanitize_request_query_string_for_path(path: &str, query: &str) -> Option<String> {
    if path != "/v1/realtime" {
        return sanitize_request_query_string(query);
    }

    let query = query.trim().trim_start_matches('?').trim();
    if query.is_empty() {
        return None;
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    let mut redacted_call_id = false;
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if key.eq_ignore_ascii_case("call_id") {
            if !value.trim().is_empty() && !redacted_call_id {
                serializer.append_pair("call_id", "{call_id}");
                redacted_call_id = true;
            }
        } else if request_query_key_is_safe_to_trace(key.as_ref()) {
            serializer.append_pair(key.as_ref(), value.as_ref());
        }
    }
    let sanitized = serializer.finish();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn request_query_key_is_safe_to_trace(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "alt" | "view" | "pagesize" | "page_size" | "limit" | "offset" | "model"
    )
}

pub fn is_matching_stream_request(
    plan_kind: &str,
    path: &str,
    body_json: &serde_json::Value,
) -> bool {
    if plan_kind == OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND {
        return false;
    }
    match plan_kind {
        OPENAI_CHAT_STREAM_PLAN_KIND
        | CLAUDE_CHAT_STREAM_PLAN_KIND
        | OPENAI_RESPONSES_STREAM_PLAN_KIND
        | CLAUDE_CLI_STREAM_PLAN_KIND
        | OPENAI_IMAGE_STREAM_PLAN_KIND
        | GEMINI_INTERACTIONS_STREAM_PLAN_KIND => body_json
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        GEMINI_CHAT_STREAM_PLAN_KIND | GEMINI_CLI_STREAM_PLAN_KIND => {
            request_path_implies_stream_request(path)
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
            | OPENAI_EMBEDDING_SYNC_PLAN_KIND
            | OPENAI_RERANK_SYNC_PLAN_KIND
            | OPENAI_SEARCH_SYNC_PLAN_KIND
            | OPENAI_IMAGE_SYNC_PLAN_KIND
            | OPENAI_RESPONSES_SYNC_PLAN_KIND
            | OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND
            | CLAUDE_CHAT_SYNC_PLAN_KIND
            | CLAUDE_CLI_SYNC_PLAN_KIND
            | CLAUDE_COUNT_TOKENS_SYNC_PLAN_KIND
            | GEMINI_CHAT_SYNC_PLAN_KIND
            | GEMINI_CLI_SYNC_PLAN_KIND
            | GEMINI_EMBEDDING_SYNC_PLAN_KIND
            | GEMINI_INTERACTIONS_SYNC_PLAN_KIND
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
            | CODEX_LIVE_STREAM_PLAN_KIND
            | OPENAI_REALTIME_STREAM_PLAN_KIND
            | CLAUDE_CHAT_STREAM_PLAN_KIND
            | GEMINI_CHAT_STREAM_PLAN_KIND
            | OPENAI_RESPONSES_STREAM_PLAN_KIND
            | OPENAI_IMAGE_STREAM_PLAN_KIND
            | CLAUDE_CLI_STREAM_PLAN_KIND
            | GEMINI_CLI_STREAM_PLAN_KIND
            | GEMINI_INTERACTIONS_STREAM_PLAN_KIND
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
        request_path_implies_stream_request, resolve_execution_runtime_stream_plan_kind,
        resolve_execution_runtime_sync_plan_kind, sanitize_request_path,
        sanitize_request_path_and_query, sanitize_request_query_string,
        supports_stream_execution_decision_kind, supports_sync_execution_decision_kind,
    };
    use crate::contracts::{
        CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
        CLAUDE_CLI_SYNC_PLAN_KIND, CODEX_LIVE_STREAM_PLAN_KIND, GEMINI_CHAT_STREAM_PLAN_KIND,
        GEMINI_CHAT_SYNC_PLAN_KIND, GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND,
        GEMINI_EMBEDDING_SYNC_PLAN_KIND, GEMINI_INTERACTIONS_STREAM_PLAN_KIND,
        GEMINI_INTERACTIONS_SYNC_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND,
        OPENAI_CHAT_SYNC_PLAN_KIND, OPENAI_EMBEDDING_SYNC_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND,
        OPENAI_IMAGE_SYNC_PLAN_KIND, OPENAI_REALTIME_STREAM_PLAN_KIND,
        OPENAI_RERANK_SYNC_PLAN_KIND, OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND,
        OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND, OPENAI_RESPONSES_STREAM_PLAN_KIND,
        OPENAI_RESPONSES_SYNC_PLAN_KIND, OPENAI_SEARCH_SYNC_PLAN_KIND,
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
    fn resolves_openai_realtime_as_websocket_stream_only() {
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("realtime"),
                None,
                &Method::GET,
                "/v1/realtime",
            ),
            Some(OPENAI_REALTIME_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("realtime"),
                None,
                &Method::GET,
                "/v1/realtime",
            ),
            None
        );
        assert!(supports_stream_execution_decision_kind(
            OPENAI_REALTIME_STREAM_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_codex_live_call_create_and_websocket_routes() {
        for (method, path) in [
            (Method::POST, "/v1/live"),
            (Method::POST, "/v1/realtime/calls"),
            (Method::GET, "/v1/live"),
            (Method::GET, "/v1/live/rtc_opaque"),
            (Method::GET, "/v1/realtime"),
        ] {
            assert_eq!(
                resolve_execution_runtime_stream_plan_kind(
                    Some("ai_public"),
                    Some("codex"),
                    Some("live"),
                    None,
                    &method,
                    path,
                ),
                Some(CODEX_LIVE_STREAM_PLAN_KIND)
            );
        }
        assert!(supports_stream_execution_decision_kind(
            CODEX_LIVE_STREAM_PLAN_KIND
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
            None
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
        assert!(!supports_stream_execution_decision_kind(
            OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_openai_search_as_sync_only() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("search"),
                None,
                &Method::POST,
                "/v1/alpha/search",
            ),
            Some(OPENAI_SEARCH_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("search"),
                None,
                &Method::POST,
                "/v1/alpha/search",
            ),
            None
        );
        assert!(supports_sync_execution_decision_kind(
            OPENAI_SEARCH_SYNC_PLAN_KIND
        ));
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("search"),
                None,
                &Method::POST,
                "/backend-api/codex/alpha/search",
            ),
            None
        );
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
    fn resolves_antigravity_v1internal_stream_plan_kind_as_gemini_cli_stream() {
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("antigravity"),
                Some("stream_generate_content"),
                Some("bearer_like"),
                &Method::POST,
                "/v1internal:streamGenerateContent",
            ),
            Some(GEMINI_CLI_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("antigravity"),
                Some("stream_generate_content"),
                Some("bearer_like"),
                &Method::POST,
                "/v1internal:streamGenerateContent",
            ),
            None
        );
    }

    #[test]
    fn stream_path_detection_handles_gemini_method_paths_with_query() {
        assert!(request_path_implies_stream_request(
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        ));
        assert!(request_path_implies_stream_request(
            " /v1internal:streamGenerateContent?alt=sse "
        ));
        assert!(!request_path_implies_stream_request(
            "/v1beta/models/gemini-2.5-pro:generateContent?alt=sse"
        ));
    }

    #[test]
    fn request_path_metadata_sanitizer_drops_sensitive_query_parameters() {
        assert_eq!(
            sanitize_request_path("/v1beta/models/gemini-2.5-pro:generateContent?key=secret")
                .as_deref(),
            Some("/v1beta/models/gemini-2.5-pro:generateContent")
        );
        assert_eq!(
            sanitize_request_query_string("?key=secret&alt=sse&pageSize=10&token=hidden")
                .as_deref(),
            Some("alt=sse&pageSize=10")
        );
        assert_eq!(
            sanitize_request_path_and_query(
                "/v1beta/models/gemini-2.5-pro:streamGenerateContent?key=secret&alt=sse",
                None
            )
            .as_deref(),
            Some("/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse")
        );
        assert_eq!(
            sanitize_request_path_and_query(
                "/v1/live/rtc_secret_opaque?alt=sse&token=hidden",
                None
            )
            .as_deref(),
            Some("/v1/live/{call_id}?alt=sse")
        );
        assert_eq!(
            sanitize_request_path_and_query(
                "/v1/realtime?model=gpt-realtime-2.1&api_key=secret&token=hidden",
                None
            )
            .as_deref(),
            Some("/v1/realtime?model=gpt-realtime-2.1")
        );
        assert_eq!(
            sanitize_request_path_and_query(
                "/v1/realtime?intent=quicksilver&call_id=rtc_secret_opaque&model=gpt-realtime-1.5&api_key=secret",
                None
            )
            .as_deref(),
            Some("/v1/realtime?call_id=%7Bcall_id%7D&model=gpt-realtime-1.5")
        );
        assert_eq!(
            sanitize_request_path_and_query(
                "/v1/realtime?c%61ll_id=rtc_secret_encoded&call_id=rtc_duplicate",
                None
            )
            .as_deref(),
            Some("/v1/realtime?call_id=%7Bcall_id%7D")
        );
    }

    #[test]
    fn request_path_metadata_sanitizer_redacts_install_session_codes() {
        for (raw, expected) in [
            ("/install/secret-code", "/install/[redacted]"),
            ("/install/secret-code.ps1", "/install/[redacted]"),
            ("/i/secret-code", "/i/[redacted]"),
            (
                "/install-tunnel/secret-code.ps1?token=also-secret",
                "/install-tunnel/[redacted]",
            ),
        ] {
            assert_eq!(sanitize_request_path(raw).as_deref(), Some(expected));
            assert_eq!(
                sanitize_request_path_and_query(raw, None).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(
            sanitize_request_path("/install/").as_deref(),
            Some("/install/")
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
            None
        );
        assert!(supports_sync_execution_decision_kind(
            OPENAI_IMAGE_SYNC_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_openai_embedding_sync_plan_kind() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("embedding"),
                None,
                &Method::POST,
                "/v1/embeddings",
            ),
            Some(OPENAI_EMBEDDING_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_execution_decision_kind(
            OPENAI_EMBEDDING_SYNC_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_gemini_embedding_sync_plan_kind() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("embedding"),
                Some("api_key"),
                &Method::POST,
                "/v1beta/models/gemini-embedding-2-preview:embedContent",
            ),
            Some(GEMINI_EMBEDDING_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("embedding"),
                Some("api_key"),
                &Method::POST,
                "/v1beta/models/gemini-embedding-2-preview:batchEmbedContents",
            ),
            Some(GEMINI_EMBEDDING_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_execution_decision_kind(
            GEMINI_EMBEDDING_SYNC_PLAN_KIND
        ));
    }

    #[test]
    fn resolves_gemini_interactions_plan_kinds() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("interactions"),
                Some("api_key"),
                &Method::POST,
                "/v1/interactions",
            ),
            Some(GEMINI_INTERACTIONS_SYNC_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_stream_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("interactions"),
                Some("api_key"),
                &Method::POST,
                "/v1/interactions",
            ),
            Some(GEMINI_INTERACTIONS_STREAM_PLAN_KIND)
        );
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("gemini"),
                Some("interactions"),
                Some("api_key"),
                &Method::POST,
                "/v1beta/interactions",
            ),
            Some(GEMINI_INTERACTIONS_SYNC_PLAN_KIND)
        );
    }

    #[test]
    fn gemini_interactions_stream_plan_requires_body_stream_true() {
        assert!(is_matching_stream_request(
            GEMINI_INTERACTIONS_STREAM_PLAN_KIND,
            "/v1/interactions",
            &serde_json::json!({"stream": true})
        ));
        assert!(!is_matching_stream_request(
            GEMINI_INTERACTIONS_STREAM_PLAN_KIND,
            "/v1/interactions",
            &serde_json::json!({"stream": false})
        ));
        assert!(!is_matching_stream_request(
            GEMINI_INTERACTIONS_STREAM_PLAN_KIND,
            "/v1/interactions",
            &serde_json::json!({})
        ));
    }

    #[test]
    fn resolves_openai_rerank_sync_plan_kind() {
        assert_eq!(
            resolve_execution_runtime_sync_plan_kind(
                Some("ai_public"),
                Some("openai"),
                Some("rerank"),
                None,
                &Method::POST,
                "/v1/rerank",
            ),
            Some(OPENAI_RERANK_SYNC_PLAN_KIND)
        );
        assert!(supports_sync_execution_decision_kind(
            OPENAI_RERANK_SYNC_PLAN_KIND
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
