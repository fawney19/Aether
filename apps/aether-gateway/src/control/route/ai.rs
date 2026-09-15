use super::{
    classified, classified_with_request_auth_channel, detect_claude_client_surface,
    is_gemini_cli_request, is_gemini_models_route, is_gemini_operation_route, ClassifiedRoute,
};
use crate::ai_serving::ApiOperation;

pub(super) fn classify_ai_public_route(
    method: &http::Method,
    normalized_path: &str,
    query: Option<&str>,
    headers: &http::HeaderMap,
) -> Option<ClassifiedRoute> {
    if let Some(route) = classify_antigravity_v1internal_route(method, normalized_path) {
        Some(route)
    } else if method == http::Method::POST && normalized_path == "/v1/chat/completions" {
        Some(classified(
            "ai_public",
            "openai",
            "chat",
            "openai:chat",
            true,
        ))
    } else if method == http::Method::POST && normalized_path == "/v1/embeddings" {
        Some(classified(
            "ai_public",
            "openai",
            "embedding",
            "openai:embedding",
            true,
        ))
    } else if method == http::Method::POST && normalized_path == "/v1/rerank" {
        Some(classified(
            "ai_public",
            "openai",
            "rerank",
            "openai:rerank",
            true,
        ))
    } else if (method == http::Method::POST && normalized_path == "/v1/live")
        || (method == http::Method::POST
            && normalized_path == "/v1/realtime/calls"
            && realtime_query_has_codex_live_intent(query))
        || (method == http::Method::GET
            && ((normalized_path == "/v1/live" || normalized_path.starts_with("/v1/live/"))
                || (normalized_path == "/v1/realtime"
                    && (realtime_query_has_codex_live_intent(query)
                        || realtime_query_is_codex_v2(query, headers))))
            && is_websocket_upgrade_request(headers))
    {
        // Codex Live has an independent wire contract and permission surface;
        // it must never be authorized as an OpenAI Responses request.
        Some(classified("ai_public", "codex", "live", "codex:live", true))
    } else if method == http::Method::GET
        && normalized_path == "/v1/realtime"
        && is_websocket_upgrade_request(headers)
    {
        // Ordinary OpenAI Realtime WebSockets retain their independent
        // permission surface. The GA WebRTC call-creation endpoint is not
        // handled by this WebSocket-only implementation; only Codex AVAS is
        // accepted above when it carries the explicit quicksilver intent.
        Some(classified(
            "ai_public",
            "openai",
            "realtime",
            "openai:realtime",
            true,
        ))
    } else if (method == http::Method::POST
        || (method == http::Method::GET
            && normalized_path == "/v1/responses"
            && is_websocket_upgrade_request(headers)))
        && matches!(normalized_path, "/v1/responses" | "/v1/responses/compact")
    {
        if normalized_path.ends_with("/compact") {
            Some(classified(
                "ai_public",
                "openai",
                "responses:compact",
                "openai:responses:compact",
                true,
            ))
        } else {
            Some(classified(
                "ai_public",
                "openai",
                "responses",
                "openai:responses",
                true,
            ))
        }
    } else if method == http::Method::POST && normalized_path == "/v1/alpha/search" {
        Some(classified(
            "ai_public",
            "openai",
            "search",
            "openai:search",
            true,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/v1/images/generations" | "/v1/images/edits"
        )
    {
        Some(classified(
            "ai_public",
            "openai",
            "image",
            "openai:image",
            true,
        ))
    } else if method == http::Method::POST && normalized_path == "/v1/messages/count_tokens" {
        let request_auth_channel = claude_request_auth_channel(headers);
        Some(
            classified_with_request_auth_channel(
                "ai_public",
                "claude",
                "count_tokens",
                request_auth_channel,
                "claude:messages",
                true,
            )
            .with_client_surface(detect_claude_client_surface(headers))
            .with_api_operation(ApiOperation::ClaudeCountTokens),
        )
    } else if method == http::Method::POST && normalized_path == "/v1/messages" {
        let request_auth_channel = claude_request_auth_channel(headers);
        Some(
            classified_with_request_auth_channel(
                "ai_public",
                "claude",
                "messages",
                request_auth_channel,
                "claude:messages",
                true,
            )
            .with_client_surface(detect_claude_client_surface(headers))
            .with_api_operation(ApiOperation::ClaudeMessagesCreate),
        )
    } else if normalized_path == "/v1/videos"
        || normalized_path.starts_with("/v1/videos/")
        || normalized_path == "/openai/v1/videos"
        || normalized_path.starts_with("/openai/v1/videos/")
    {
        Some(classified(
            "ai_public",
            "openai",
            "video",
            "openai:video",
            true,
        ))
    } else if method == http::Method::POST
        && matches!(normalized_path, "/v1/interactions" | "/v1beta/interactions")
    {
        Some(classified_with_request_auth_channel(
            "ai_public",
            "gemini",
            "interactions",
            "api_key",
            "gemini:interactions",
            true,
        ))
    } else if method == http::Method::POST && is_gemini_models_route(normalized_path) {
        if normalized_path.ends_with(":predictLongRunning") {
            Some(classified(
                "ai_public",
                "gemini",
                "video",
                "gemini:video",
                true,
            ))
        } else if normalized_path.ends_with(":embedContent")
            || normalized_path.ends_with(":batchEmbedContents")
        {
            Some(classified_with_request_auth_channel(
                "ai_public",
                "gemini",
                "embedding",
                "api_key",
                "gemini:embedding",
                true,
            ))
        } else if is_gemini_cli_request(headers) {
            Some(classified_with_request_auth_channel(
                "ai_public",
                "gemini",
                "generate_content",
                "bearer_like",
                "gemini:generate_content",
                true,
            ))
        } else {
            Some(classified_with_request_auth_channel(
                "ai_public",
                "gemini",
                "generate_content",
                "api_key",
                "gemini:generate_content",
                true,
            ))
        }
    } else if is_gemini_operation_method(method, normalized_path)
        && is_gemini_operation_route(normalized_path)
    {
        Some(classified(
            "ai_public",
            "gemini",
            "video",
            "gemini:video",
            true,
        ))
    } else if is_gemini_files_method(method, normalized_path) {
        Some(classified(
            "ai_public",
            "gemini",
            "files",
            "gemini:files",
            true,
        ))
    } else {
        None
    }
}

/// Codex's standalone realtime WebSocket is distinguished from the ordinary
/// OpenAI Realtime API by its `intent=quicksilver` selector.  Keep this check
/// deliberately narrow: `call_id` is also used by ordinary OpenAI Realtime
/// sideband sockets, and neither it nor a model query may select the
/// `codex:live` permission surface on its own.
fn realtime_query_has_codex_live_intent(query: Option<&str>) -> bool {
    let mut seen = false;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key.eq_ignore_ascii_case("intent") {
            if seen || !value.eq_ignore_ascii_case("quicksilver") {
                return false;
            }
            seen = true;
        }
    }
    seen
}

/// Codex realtime v2 intentionally omits the v1 `intent=quicksilver` query
/// marker.  Its default headers still carry the stable Codex originator, so
/// use that identity plus a model query to select the Codex Live permission
/// surface without stealing ordinary OpenAI Realtime sockets.
fn realtime_query_is_codex_v2(query: Option<&str>, headers: &http::HeaderMap) -> bool {
    let mut has_model = false;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        // V2 has no intent marker. If one is present, let the normal
        // Realtime/V1 classifier handle it instead of silently treating a
        // conflicting request as Codex V2.
        if key.eq_ignore_ascii_case("intent") || key.eq_ignore_ascii_case("call_id") {
            return false;
        }
        if key.eq_ignore_ascii_case("model") && !value.trim().is_empty() {
            has_model = true;
        }
    }
    if !has_model {
        return false;
    }
    let Some(originator) = crate::headers::header_value_str(headers, "originator") else {
        return false;
    };
    originator.split_whitespace().next().is_some_and(|value| {
        value.eq_ignore_ascii_case("codex_cli_rs")
            || value.to_ascii_lowercase().starts_with("codex_cli_rs/")
            || value.eq_ignore_ascii_case("codex_work_desktop")
            || value.eq_ignore_ascii_case("codex_work_web")
            || value.eq_ignore_ascii_case("codex_work_mobile")
    })
}

fn claude_request_auth_channel(headers: &http::HeaderMap) -> &'static str {
    if crate::headers::header_value_str(headers, "x-api-key").is_some()
        || crate::headers::header_value_str(headers, "api-key").is_some()
    {
        "api_key"
    } else if crate::headers::header_value_str(headers, http::header::AUTHORIZATION.as_str())
        .is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("bearer "))
    {
        "bearer_like"
    } else {
        "api_key"
    }
}

fn is_websocket_upgrade_request(headers: &http::HeaderMap) -> bool {
    let has_upgrade_connection = headers
        .get(http::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|value| value.eq_ignore_ascii_case("upgrade"))
        });
    let has_websocket_upgrade = headers
        .get(http::header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));

    has_upgrade_connection && has_websocket_upgrade
}

fn is_gemini_operation_method(method: &http::Method, normalized_path: &str) -> bool {
    method == http::Method::GET
        || (method == http::Method::POST && normalized_path.ends_with(":cancel"))
}

fn is_gemini_files_method(method: &http::Method, normalized_path: &str) -> bool {
    (method == http::Method::POST && normalized_path == "/upload/v1beta/files")
        || ((method == http::Method::GET || method == http::Method::DELETE)
            && normalized_path.starts_with("/v1beta/files"))
}

fn classify_antigravity_v1internal_route(
    method: &http::Method,
    normalized_path: &str,
) -> Option<ClassifiedRoute> {
    if method != http::Method::POST {
        return None;
    }

    let action = normalized_path.strip_prefix("/v1internal:")?;
    let (route_kind, execution_runtime_candidate) = match action {
        "loadCodeAssist" => ("load_code_assist", false),
        "fetchAvailableModels" => ("fetch_available_models", false),
        "retrieveUserQuotaSummary" => ("retrieve_user_quota_summary", false),
        "fetchUserInfo" => ("fetch_user_info", false),
        "fetchAdminControls" => ("fetch_admin_controls", false),
        "setUserSettings" => ("set_user_settings", false),
        "listExperiments" => ("list_experiments", false),
        "recordCodeAssistMetrics" => ("record_code_assist_metrics", false),
        "writeTrajectoryAcls" => ("write_trajectory_acls", false),
        "streamGenerateContent" => ("stream_generate_content", true),
        _ => return None,
    };

    Some(classified_with_request_auth_channel(
        "ai_public",
        "antigravity",
        route_kind,
        "bearer_like",
        "antigravity:v1internal",
        execution_runtime_candidate,
    ))
}

#[cfg(test)]
mod tests {
    use axum::http::header::{CONNECTION, UPGRADE};
    use axum::http::{HeaderMap, HeaderValue, Method};

    use super::classify_ai_public_route;

    #[test]
    fn classifies_websocket_upgrade_on_responses_route() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));

        let route = classify_ai_public_route(&Method::GET, "/v1/responses", None, &headers)
            .expect("Responses WebSocket should be an AI public route");
        assert_eq!(route.route_class, "ai_public");
        assert_eq!(route.route_family, "openai");
        assert_eq!(route.route_kind, "responses");
        assert_eq!(route.auth_endpoint_signature, "openai:responses");
    }

    #[test]
    fn does_not_classify_plain_get_as_responses_websocket() {
        assert!(
            classify_ai_public_route(&Method::GET, "/v1/responses", None, &HeaderMap::new())
                .is_none()
        );
    }

    #[test]
    fn classifies_only_websocket_upgrade_on_realtime_route() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));

        let route = classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("model=gpt-realtime-2.1"),
            &headers,
        )
        .expect("Realtime WebSocket should be an AI public route");
        assert_eq!(route.route_class, "ai_public");
        assert_eq!(route.route_family, "openai");
        assert_eq!(route.route_kind, "realtime");
        assert_eq!(route.auth_endpoint_signature, "openai:realtime");
        assert!(route.execution_runtime_candidate);

        let mut codex_v2_headers = headers.clone();
        codex_v2_headers.insert("originator", HeaderValue::from_static("codex_work_desktop"));
        let codex_v2 = classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("model=gpt-live-1-codex"),
            &codex_v2_headers,
        )
        .expect("Codex realtime v2 should use the Live route");
        assert_eq!(codex_v2.route_family, "codex");
        assert_eq!(codex_v2.auth_endpoint_signature, "codex:live");

        let mut codex_cli_headers = headers.clone();
        codex_cli_headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        let codex_cli_v2 = classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("model=gpt-live-1-codex"),
            &codex_cli_headers,
        )
        .expect("Codex CLI realtime v2 should use the Live route");
        assert_eq!(codex_cli_v2.auth_endpoint_signature, "codex:live");

        let mut ordinary_headers = headers.clone();
        ordinary_headers.insert("originator", HeaderValue::from_static("openai-python"));
        let ordinary = classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("model=gpt-realtime-2.1"),
            &ordinary_headers,
        )
        .expect("ordinary Realtime should remain available");
        assert_eq!(ordinary.auth_endpoint_signature, "openai:realtime");

        assert!(classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("model=gpt-realtime-2.1"),
            &HeaderMap::new(),
        )
        .is_none());
        assert!(classify_ai_public_route(&Method::POST, "/v1/realtime", None, &headers).is_none());
    }

    #[test]
    fn classifies_live_http_and_websocket_routes_as_codex_live() {
        let legacy_post =
            classify_ai_public_route(&Method::POST, "/v1/live", None, &HeaderMap::new())
                .expect("legacy Live call creation should be an AI public route");
        assert_eq!(legacy_post.route_family, "codex");
        assert_eq!(legacy_post.route_kind, "live");
        assert_eq!(legacy_post.auth_endpoint_signature, "codex:live");

        let avas_post = classify_ai_public_route(
            &Method::POST,
            "/v1/realtime/calls",
            Some("intent=quicksilver&architecture=avas"),
            &HeaderMap::new(),
        )
        .expect("Codex AVAS call creation should be an AI public route");
        assert_eq!(avas_post.route_family, "codex");
        assert_eq!(avas_post.route_kind, "live");
        assert_eq!(avas_post.auth_endpoint_signature, "codex:live");

        // The same endpoint is part of the ordinary OpenAI Realtime API, but
        // Aether's OpenAI Realtime implementation currently supports direct
        // WebSockets only. A request without Codex's explicit AVAS intent must
        // neither be captured by the Codex Live planner nor be advertised as a
        // supported ordinary call-create request.
        for query in [None, Some("model=gpt-realtime"), Some("architecture=avas")] {
            assert!(classify_ai_public_route(
                &Method::POST,
                "/v1/realtime/calls",
                query,
                &HeaderMap::new(),
            )
            .is_none());
        }

        for query in [
            Some("intent=other&architecture=avas"),
            Some("intent=quicksilver&intent=other&architecture=avas"),
        ] {
            assert!(classify_ai_public_route(
                &Method::POST,
                "/v1/realtime/calls",
                query,
                &HeaderMap::new(),
            )
            .is_none());
        }

        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
        headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
        for path in ["/v1/live", "/v1/live/rtc_opaque"] {
            let route = classify_ai_public_route(&Method::GET, path, None, &headers)
                .expect("Live WebSocket should be an AI public route");
            assert_eq!(route.route_family, "codex");
            assert_eq!(route.route_kind, "live");
            assert_eq!(route.auth_endpoint_signature, "codex:live");
        }

        assert!(classify_ai_public_route(
            &Method::GET,
            "/v1/live/rtc_opaque",
            None,
            &HeaderMap::new(),
        )
        .is_none());

        let sideband = classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("intent=quicksilver&call_id=rtc_opaque"),
            &headers,
        )
        .expect("Realtime sideband WebSocket should be a Codex Live route");
        assert_eq!(sideband.route_family, "codex");
        assert_eq!(sideband.route_kind, "live");
        assert_eq!(sideband.auth_endpoint_signature, "codex:live");

        for query in [None, Some("model=gpt-realtime")] {
            let route = classify_ai_public_route(&Method::GET, "/v1/realtime", query, &headers)
                .expect("Realtime WebSocket without a call_id key should remain OpenAI Realtime");
            assert_eq!(route.auth_endpoint_signature, "openai:realtime");
        }
        let codex_direct = classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("intent=quicksilver&model=gpt-realtime-1.5"),
            &headers,
        )
        .expect("Codex direct realtime WebSocket should use the Live route");
        assert_eq!(codex_direct.route_family, "codex");
        assert_eq!(codex_direct.route_kind, "live");
        assert_eq!(codex_direct.auth_endpoint_signature, "codex:live");
        for query in [
            Some("intent=other&model=gpt-realtime-1.5"),
            Some("intent=quicksilver&intent=other&model=gpt-realtime-1.5"),
        ] {
            let route = classify_ai_public_route(&Method::GET, "/v1/realtime", query, &headers)
                .expect("non-Codex realtime intent should remain OpenAI Realtime");
            assert_eq!(route.auth_endpoint_signature, "openai:realtime");
        }
        // `call_id` is shared by ordinary OpenAI Realtime WebRTC sideband
        // sockets. It must not select Codex Live without Codex's explicit
        // `intent=quicksilver` signal.
        for query in [
            Some("call_id=rtc_ordinary"),
            Some("c%61ll_id=rtc_encoded"),
            Some("call_id="),
            Some("call_id=%20"),
            Some("call_id=rtc_one&call_id=rtc_two"),
            Some("call_id=rtc_ordinary&model=gpt-realtime-1.5"),
        ] {
            let route = classify_ai_public_route(&Method::GET, "/v1/realtime", query, &headers)
                .expect("ordinary Realtime sideband should remain routable");
            assert_eq!(route.route_family, "openai");
            assert_eq!(route.route_kind, "realtime");
            assert_eq!(route.auth_endpoint_signature, "openai:realtime");
        }

        let codex_sideband_with_encoded_call_id = classify_ai_public_route(
            &Method::GET,
            "/v1/realtime",
            Some("intent=quicksilver&c%61ll_id=rtc_encoded"),
            &headers,
        )
        .expect("encoded call_id must not hide Codex's explicit Live intent");
        assert_eq!(
            codex_sideband_with_encoded_call_id.auth_endpoint_signature,
            "codex:live"
        );
    }
}
