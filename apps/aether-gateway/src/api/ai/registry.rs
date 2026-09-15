use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode, Uri};
use axum::routing::{any, get, post};
use axum::Router;

use super::{aliyun, claude, doubao, gemini, jina, openai};
use crate::api::response::build_local_http_error_response_with_request_path;
use crate::headers::extract_or_generate_trace_id;
use crate::{
    handlers::proxy::{live_websocket, proxy_request, realtime_websocket, responses_websocket},
    state::AppState,
    GatewayError,
};

// Router registration patterns live here so AI public ingress has a single mount registry.
// They intentionally stay separate from manifest-facing route inventories in constants.rs,
// which describe operational compatibility surfaces rather than the concrete axum mount list.
const AI_POST_ROUTE_PATTERNS: &[&str] = &[
    "/v1/chat/completions",
    "/v1/embeddings",
    "/v1/rerank",
    "/v1/responses",
    "/v1/responses/compact",
    "/v1/live",
    "/v1/realtime/calls",
    "/v1/alpha/search",
    "/v1/images/generations",
    "/v1/images/edits",
    "/v1/interactions",
    "/v1beta/interactions",
    "/v1internal:loadCodeAssist",
    "/v1internal:fetchAvailableModels",
    "/v1internal:retrieveUserQuotaSummary",
    "/v1internal:fetchUserInfo",
    "/v1internal:fetchAdminControls",
    "/v1internal:setUserSettings",
    "/v1internal:listExperiments",
    "/v1internal:recordCodeAssistMetrics",
    "/v1internal:writeTrajectoryAcls",
    "/v1internal:streamGenerateContent",
];

const CLAUDE_POST_ROUTE_PATTERNS: &[&str] = &["/v1/messages", "/v1/messages/count_tokens"];

const AI_ANY_ROUTE_PATTERNS: &[&str] = &[
    "/v1/models/{*gemini_path}",
    "/v1beta/models/{*gemini_path}",
    "/v1beta/operations",
    "/v1beta/operations/{*operation_path}",
    "/v1/videos",
    "/v1/videos/{*video_path}",
    "/openai/v1/videos",
    "/openai/v1/videos/{*video_path}",
    "/upload/v1beta/files",
    "/v1beta/files",
    "/v1beta/files/{*file_path}",
];

pub(crate) fn mount_ai_routes(mut router: Router<AppState>) -> Router<AppState> {
    for path in AI_POST_ROUTE_PATTERNS {
        router = if *path == "/v1/responses" {
            router.route(path, get(responses_websocket).post(proxy_request))
        } else if *path == "/v1/live" {
            router.route(path, get(live_websocket).post(proxy_request))
        } else {
            router.route(path, post(proxy_request))
        };
    }
    router = router.route("/v1/live/{call_id}", get(live_websocket));
    router = router.route("/v1/realtime", get(dispatch_realtime_websocket));
    for path in CLAUDE_POST_ROUTE_PATTERNS {
        router = router.route(
            path,
            post(proxy_request).fallback(claude_method_not_allowed),
        );
    }
    for path in AI_ANY_ROUTE_PATTERNS {
        router = router.route(path, any(proxy_request));
    }
    router
}

async fn dispatch_realtime_websocket(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response<Body>, GatewayError> {
    if realtime_query_is_codex_live(uri.query(), &headers) {
        live_websocket(State(state), ConnectInfo(remote_addr), ws, headers, uri).await
    } else {
        realtime_websocket(State(state), ConnectInfo(remote_addr), ws, headers, uri).await
    }
}

fn realtime_query_is_codex_live(query: Option<&str>, headers: &HeaderMap) -> bool {
    let mut has_call_id = false;
    let mut has_live_intent = false;
    let mut duplicate_or_conflicting_intent = false;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key.eq_ignore_ascii_case("call_id") {
            has_call_id = true;
        } else if key.eq_ignore_ascii_case("intent") {
            if has_live_intent || !value.eq_ignore_ascii_case("quicksilver") {
                duplicate_or_conflicting_intent = true;
            }
            has_live_intent = true;
        }
    }
    if has_live_intent {
        return !duplicate_or_conflicting_intent;
    }
    // `call_id` is part of the ordinary OpenAI Realtime WebRTC sideband
    // contract too. Without Codex's explicit v1 intent it must remain on the
    // generic Realtime handler instead of being authorized as `codex:live`.
    if has_call_id {
        return false;
    }
    // Realtime v2 has no intent selector. A malformed or conflicting intent
    // must not be reclassified as v2 merely because a Codex originator is
    // present.
    let has_model = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .any(|(key, value)| key.eq_ignore_ascii_case("model") && !value.trim().is_empty());
    let Some(originator) = crate::headers::header_value_str(headers, "originator") else {
        return false;
    };
    has_model
        && originator.split_whitespace().next().is_some_and(|value| {
            value.eq_ignore_ascii_case("codex_cli_rs")
                || value.to_ascii_lowercase().starts_with("codex_cli_rs/")
                || value.eq_ignore_ascii_case("codex_work_desktop")
                || value.eq_ignore_ascii_case("codex_work_web")
                || value.eq_ignore_ascii_case("codex_work_mobile")
        })
}

async fn claude_method_not_allowed(request: Request) -> Result<Response<Body>, GatewayError> {
    let trace_id = extract_or_generate_trace_id(request.headers());
    let mut response = build_local_http_error_response_with_request_path(
        &trace_id,
        None,
        Some(request.uri().path()),
        StatusCode::METHOD_NOT_ALLOWED,
        "Method not allowed",
    )?;
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    Ok(response)
}

pub(crate) fn public_api_format_local_path(api_format: &str) -> &'static str {
    let normalized = api_format.trim().to_ascii_lowercase();
    openai::local_path(&normalized)
        .or_else(|| claude::local_path(&normalized))
        .or_else(|| gemini::local_path(&normalized))
        .or_else(|| jina::local_path(&normalized))
        .or_else(|| doubao::local_path(&normalized))
        .or_else(|| aliyun::local_path(&normalized))
        .unwrap_or("/")
}

pub(crate) fn normalize_admin_endpoint_signature(api_format: &str) -> Option<&'static str> {
    let normalized = api_format.trim().to_ascii_lowercase();
    openai::normalized_signature(&normalized)
        .or_else(|| claude::normalized_signature(&normalized))
        .or_else(|| gemini::normalized_signature(&normalized))
        .or_else(|| jina::normalized_signature(&normalized))
        .or_else(|| doubao::normalized_signature(&normalized))
        .or_else(|| aliyun::normalized_signature(&normalized))
}

pub(crate) fn admin_endpoint_signature_parts(
    api_format: &str,
) -> Option<(&'static str, &'static str, &'static str)> {
    let normalized = normalize_admin_endpoint_signature(api_format)?;
    let (api_family, endpoint_kind) = normalized.split_once(':')?;
    Some((normalized, api_family, endpoint_kind))
}

pub(crate) fn admin_default_body_rules_for_signature(
    api_format: &str,
    provider_type: Option<&str>,
) -> Option<(String, Vec<serde_json::Value>)> {
    let normalized_api_format = normalize_admin_endpoint_signature(api_format)?.to_string();
    let _ = provider_type;
    Some((normalized_api_format, Vec::new()))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{
        admin_endpoint_signature_parts, public_api_format_local_path, realtime_query_is_codex_live,
        AI_POST_ROUTE_PATTERNS,
    };

    #[test]
    fn registers_and_dispatches_realtime_live_aliases() {
        assert!(AI_POST_ROUTE_PATTERNS.contains(&"/v1/realtime/calls"));
        let no_headers = HeaderMap::new();
        assert!(realtime_query_is_codex_live(
            Some("intent=quicksilver&call_id=rtc_opaque"),
            &no_headers
        ));
        assert!(realtime_query_is_codex_live(
            Some("intent=quicksilver&c%61ll_id=rtc_encoded"),
            &no_headers
        ));
        assert!(!realtime_query_is_codex_live(
            Some("c%61ll_id=rtc_encoded"),
            &no_headers
        ));
        assert!(!realtime_query_is_codex_live(None, &no_headers));
        assert!(!realtime_query_is_codex_live(Some("call_id="), &no_headers));
        assert!(!realtime_query_is_codex_live(
            Some("call_id=%20"),
            &no_headers
        ));
        assert!(realtime_query_is_codex_live(
            Some("intent=quicksilver&model=gpt-realtime-1.5"),
            &no_headers
        ));
        assert!(!realtime_query_is_codex_live(
            Some("model=gpt-realtime-1.5"),
            &no_headers
        ));
        assert!(!realtime_query_is_codex_live(
            Some("intent=other&model=gpt-realtime-1.5"),
            &no_headers
        ));
        assert!(!realtime_query_is_codex_live(
            Some("intent=quicksilver&intent=other&model=gpt-realtime-1.5"),
            &no_headers
        ));
        let mut codex_v2_headers = HeaderMap::new();
        codex_v2_headers.insert("originator", HeaderValue::from_static("codex_work_desktop"));
        assert!(realtime_query_is_codex_live(
            Some("model=gpt-live-1-codex"),
            &codex_v2_headers
        ));
        assert!(!realtime_query_is_codex_live(
            Some("call_id=rtc_ordinary&model=gpt-live-1-codex"),
            &codex_v2_headers
        ));
        assert!(!realtime_query_is_codex_live(
            Some("call_id=rtc_one&call_id=rtc_two"),
            &codex_v2_headers
        ));
        let mut ordinary_headers = HeaderMap::new();
        ordinary_headers.insert("originator", HeaderValue::from_static("openai-python"));
        assert!(!realtime_query_is_codex_live(
            Some("model=gpt-realtime-1.5"),
            &ordinary_headers
        ));
    }

    #[test]
    fn supports_data_api_endpoint_signatures_and_public_paths() {
        for (api_format, family, kind, path) in [
            ("openai:embedding", "openai", "embedding", "/v1/embeddings"),
            (
                "gemini:interactions",
                "gemini",
                "interactions",
                "/v1/interactions",
            ),
            (
                "gemini:embedding",
                "gemini",
                "embedding",
                "/v1beta/models/{model}:{action}",
            ),
            ("jina:embedding", "jina", "embedding", "/v1/embeddings"),
            ("doubao:embedding", "doubao", "embedding", "/v1/embeddings"),
            (
                "aliyun:multimodal_embedding",
                "aliyun",
                "multimodal_embedding",
                "/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding",
            ),
            ("openai:rerank", "openai", "rerank", "/v1/rerank"),
            ("openai:search", "openai", "search", "/v1/alpha/search"),
            ("openai:realtime", "openai", "realtime", "/v1/realtime"),
            ("codex:live", "codex", "live", "/v1/live"),
            ("jina:rerank", "jina", "rerank", "/v1/rerank"),
        ] {
            assert_eq!(
                admin_endpoint_signature_parts(api_format),
                Some((api_format, family, kind))
            );
            assert_eq!(public_api_format_local_path(api_format), path);
        }
    }
}
