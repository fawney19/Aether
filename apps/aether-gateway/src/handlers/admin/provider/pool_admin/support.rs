use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::shared::query_param_value;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

pub(super) const ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL: &str =
    "Admin pool overview requires provider catalog reader";
pub(super) const ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL: &str =
    "Admin pool cleanup requires provider catalog writer";
pub(super) const ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE: &str = "未发现可清理的异常账号";

#[derive(Debug, Default, Deserialize)]
pub(super) struct AdminPoolResolveSelectionRequest {
    #[serde(default)]
    pub(crate) search: String,
    #[serde(default)]
    pub(crate) quick_selectors: Vec<String>,
}

pub(super) fn build_admin_pool_error_response(
    status: http::StatusCode,
    detail: impl Into<String>,
) -> Response<Body> {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

pub(super) fn parse_admin_pool_page(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "page") {
        None => Ok(1),
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "page must be an integer between 1 and 10000".to_string())?;
            if (1..=10_000).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("page must be an integer between 1 and 10000".to_string())
            }
        }
    }
}

pub(super) fn parse_admin_pool_page_size(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "page_size") {
        None => Ok(50),
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "page_size must be an integer between 1 and 200".to_string())?;
            if (1..=200).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("page_size must be an integer between 1 and 200".to_string())
            }
        }
    }
}

pub(super) fn parse_admin_pool_search(query: Option<&str>) -> Option<String> {
    query_param_value(query, "search")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn parse_admin_pool_status_filter(query: Option<&str>) -> Result<String, String> {
    let value = query_param_value(query, "status")
        .unwrap_or_else(|| "all".to_string())
        .trim()
        .to_ascii_lowercase();
    match value.as_str() {
        "all" | "active" | "inactive" | "cooldown" => Ok(value),
        _ => Err("status must be one of: all, active, cooldown, inactive".to_string()),
    }
}

pub(super) fn admin_pool_provider_id_from_path(request_path: &str) -> Option<String> {
    let raw = request_path.strip_prefix("/api/admin/pool/")?;
    let mut segments = raw.split('/');
    let provider_id = segments.next()?.trim();
    let keys_segment = segments.next()?.trim();
    if provider_id.is_empty() || keys_segment != "keys" {
        None
    } else {
        Some(provider_id.to_string())
    }
}

pub(super) fn is_admin_pool_route(request_context: &GatewayPublicRequestContext) -> bool {
    let normalized_path = request_context.request_path.trim_end_matches('/');
    let path = if normalized_path.is_empty() {
        request_context.request_path.as_str()
    } else {
        normalized_path
    };

    (request_context.request_method == http::Method::GET && path == "/api/admin/pool/overview")
        || (request_context.request_method == http::Method::GET
            && path == "/api/admin/pool/scheduling-presets")
        || (request_context.request_method == http::Method::GET
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys")
            && path.matches('/').count() == 5)
        || (request_context.request_method == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/batch-import")
            && path.matches('/').count() == 6)
        || (request_context.request_method == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/batch-action")
            && path.matches('/').count() == 6)
        || (request_context.request_method == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/resolve-selection")
            && path.matches('/').count() == 6)
        || (request_context.request_method == http::Method::GET
            && path.starts_with("/api/admin/pool/")
            && path.contains("/keys/batch-delete-task/")
            && path.matches('/').count() == 7)
        || (request_context.request_method == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/cleanup-banned")
            && path.matches('/').count() == 6)
}
