use std::sync::OnceLock;

use axum::http::HeaderMap;
use url::Url;

use crate::constants::{
    EXECUTION_RUNTIME_LOOP_GUARD_HEADER, EXECUTION_RUNTIME_LOOP_GUARD_VALUE,
    EXECUTION_RUNTIME_LOOP_GUARD_VIA_TOKEN,
};
use crate::headers::header_value_str;

const DEFAULT_APP_PORT: u16 = 8084;

static GATEWAY_FRONTDOOR_APP_PORT: OnceLock<u16> = OnceLock::new();

pub(crate) fn request_has_execution_runtime_loop_guard(headers: &HeaderMap) -> bool {
    header_value_str(headers, EXECUTION_RUNTIME_LOOP_GUARD_HEADER)
        .is_some_and(|value| value.eq_ignore_ascii_case(EXECUTION_RUNTIME_LOOP_GUARD_VALUE))
        || request_has_execution_runtime_via_guard(headers)
}

fn request_has_execution_runtime_via_guard(headers: &HeaderMap) -> bool {
    headers
        .get_all("via")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value
                .to_ascii_lowercase()
                .contains(EXECUTION_RUNTIME_LOOP_GUARD_VIA_TOKEN)
        })
}

pub(crate) fn frontdoor_self_loop_public_ai_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/messages"
            | "/v1/messages/count_tokens"
            | "/v1/chat/completions"
            | "/v1/embeddings"
            | "/v1/rerank"
            | "/v1/responses"
            | "/v1/responses/compact"
            | "/v1/alpha/search"
            | "/v1beta/files"
            | "/upload/v1beta/files"
            | "/v1beta/operations"
            | "/v1/videos"
    ) || path.starts_with("/v1/videos/")
        || path.starts_with("/v1beta/files/")
        || path.starts_with("/v1beta/operations/")
        || path.starts_with("/v1internal:")
        || is_gemini_generation_path(path)
}

pub fn set_gateway_frontdoor_app_port(app_port: u16) {
    let _ = GATEWAY_FRONTDOOR_APP_PORT.set(app_port);
}

pub(crate) fn configured_gateway_frontdoor_base_url() -> String {
    format!(
        "http://127.0.0.1:{}",
        configured_gateway_frontdoor_app_port()
    )
}

pub(crate) fn gateway_frontdoor_self_loop_guard_error(url: &str) -> Option<String> {
    gateway_frontdoor_self_loop_guard_error_with_port(configured_gateway_frontdoor_app_port(), url)
}

pub(crate) fn gateway_frontdoor_self_loop_guard_error_with_port(
    app_port: u16,
    url: &str,
) -> Option<String> {
    gateway_frontdoor_self_loop_guard_matches_with_port(app_port, url).then(|| {
        format!(
            "upstream execution target resolves back to the local aether-gateway frontdoor: {url}"
        )
    })
}

pub(crate) fn gateway_frontdoor_self_loop_guard_matches_with_port(
    app_port: u16,
    url: &str,
) -> bool {
    if app_port == 0 {
        return false;
    }
    let Some(target_url) = Url::parse(url).ok() else {
        return false;
    };
    if !frontdoor_self_loop_public_ai_path(target_url.path()) {
        return false;
    }

    let Some(target_host) = target_url.host_str() else {
        return false;
    };
    let Some(target_port) = target_url.port_or_known_default() else {
        return false;
    };
    if target_port != app_port {
        return false;
    }

    is_loopbackish_host(normalize_host_for_frontdoor_loop_guard(target_host).as_str())
}

fn is_gemini_generation_path(path: &str) -> bool {
    path.strip_prefix("/v1/models/")
        .or_else(|| path.strip_prefix("/v1beta/models/"))
        .is_some_and(|suffix| {
            suffix.contains(":generateContent")
                || suffix.contains(":streamGenerateContent")
                || suffix.contains(":predictLongRunning")
        })
}

fn configured_gateway_frontdoor_app_port() -> u16 {
    GATEWAY_FRONTDOOR_APP_PORT
        .get()
        .copied()
        .or_else(|| {
            std::env::var("APP_PORT")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|value| *value > 0)
        })
        .unwrap_or(DEFAULT_APP_PORT)
}

fn normalize_host_for_frontdoor_loop_guard(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

fn is_loopbackish_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "::")
}
