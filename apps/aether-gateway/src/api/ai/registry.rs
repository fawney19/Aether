use axum::routing::{any, post};
use axum::Router;

use super::{claude, gemini, openai};
use crate::{handlers::proxy::proxy_request, state::AppState};

// Router registration patterns live here so AI public ingress has a single mount registry.
// They intentionally stay separate from manifest-facing route inventories in constants.rs,
// which describe operational compatibility surfaces rather than the concrete axum mount list.
const AI_POST_ROUTE_PATTERNS: &[&str] = &[
    "/v1/chat/completions",
    "/v1/messages",
    "/v1/messages/count_tokens",
    "/v1/responses",
    "/v1/responses/compact",
    "/v1/images/generations",
    "/v1/images/edits",
    "/v1/images/variations",
];

const AI_ANY_ROUTE_PATTERNS: &[&str] = &[
    "/v1/models/{*gemini_path}",
    "/v1beta/models/{*gemini_path}",
    "/v1beta/operations",
    "/v1beta/operations/{*operation_path}",
    "/v1/videos",
    "/v1/videos/{*video_path}",
    "/upload/v1beta/files",
    "/v1beta/files",
    "/v1beta/files/{*file_path}",
];

pub(crate) fn mount_ai_routes(mut router: Router<AppState>) -> Router<AppState> {
    for path in AI_POST_ROUTE_PATTERNS {
        router = router.route(path, post(proxy_request));
    }
    for path in AI_ANY_ROUTE_PATTERNS {
        router = router.route(path, any(proxy_request));
    }
    router
}

pub(crate) fn public_api_format_local_path(api_format: &str) -> &'static str {
    let normalized = api_format.trim().to_ascii_lowercase();
    openai::local_path(&normalized)
        .or_else(|| claude::local_path(&normalized))
        .or_else(|| gemini::local_path(&normalized))
        .unwrap_or("/")
}

pub(crate) fn normalize_admin_endpoint_signature(api_format: &str) -> Option<&'static str> {
    let normalized = api_format.trim().to_ascii_lowercase();
    openai::normalized_signature(&normalized)
        .or_else(|| claude::normalized_signature(&normalized))
        .or_else(|| gemini::normalized_signature(&normalized))
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
