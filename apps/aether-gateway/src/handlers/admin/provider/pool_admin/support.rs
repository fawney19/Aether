use crate::handlers::admin::request::AdminRequestContext;
use crate::handlers::admin::shared::query_param_value;
pub(crate) use aether_admin::provider::pool::AdminPoolResolveSelectionRequest;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) const ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL: &str =
    "Admin pool overview requires provider catalog reader";
pub(crate) const ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL: &str =
    "Admin pool cleanup requires provider catalog writer";
pub(crate) const ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE: &str = "未发现可清理的异常账号";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdminPoolKeySortField {
    Default,
    ImportedAt,
    LastUsedAt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdminPoolKeySortDirection {
    Asc,
    Desc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AdminPoolKeySort {
    pub field: AdminPoolKeySortField,
    pub direction: AdminPoolKeySortDirection,
}

impl Default for AdminPoolKeySort {
    fn default() -> Self {
        Self {
            field: AdminPoolKeySortField::Default,
            direction: AdminPoolKeySortDirection::Desc,
        }
    }
}

pub(crate) fn build_admin_pool_error_response(
    status: http::StatusCode,
    detail: impl Into<String>,
) -> Response<Body> {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

pub(crate) fn parse_admin_pool_page(query: Option<&str>) -> Result<usize, String> {
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

pub(crate) fn parse_admin_pool_page_size(query: Option<&str>) -> Result<usize, String> {
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

pub(crate) fn parse_admin_pool_search(query: Option<&str>) -> Option<String> {
    query_param_value(query, "search")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn parse_admin_pool_quick_selectors(query: Option<&str>) -> Vec<String> {
    query_param_value(query, "quick_selectors")
        .map(|value| {
            value
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(crate) fn parse_admin_pool_status_filter(query: Option<&str>) -> Result<String, String> {
    let value = query_param_value(query, "status")
        .unwrap_or_else(|| "all".to_string())
        .trim()
        .to_ascii_lowercase();
    match value.as_str() {
        "all" | "active" | "inactive" | "cooldown" => Ok(value),
        _ => Err("status must be one of: all, active, cooldown, inactive".to_string()),
    }
}

pub(crate) fn parse_admin_pool_key_sort(query: Option<&str>) -> Result<AdminPoolKeySort, String> {
    let field = match query_param_value(query, "sort_by")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .as_deref()
    {
        None | Some("default") | Some("name") => AdminPoolKeySortField::Default,
        Some("imported_at") | Some("created_at") => AdminPoolKeySortField::ImportedAt,
        Some("last_used_at") | Some("last_used") => AdminPoolKeySortField::LastUsedAt,
        Some(_) => {
            return Err("sort_by must be one of: name, imported_at, last_used_at".to_string());
        }
    };
    let direction = match query_param_value(query, "sort_order")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .as_deref()
    {
        None | Some("desc") => AdminPoolKeySortDirection::Desc,
        Some("asc") => AdminPoolKeySortDirection::Asc,
        Some(_) => return Err("sort_order must be one of: asc, desc".to_string()),
    };
    Ok(AdminPoolKeySort { field, direction })
}

pub(crate) fn admin_pool_provider_id_from_path(request_path: &str) -> Option<String> {
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

pub(crate) fn is_admin_pool_route(request_context: &AdminRequestContext<'_>) -> bool {
    let normalized_path = request_context.path().trim_end_matches('/');
    let path = if normalized_path.is_empty() {
        request_context.path()
    } else {
        normalized_path
    };

    (request_context.method() == http::Method::GET && path == "/api/admin/pool/overview")
        || (request_context.method() == http::Method::GET
            && path == "/api/admin/pool/scheduling-presets")
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys")
            && path.matches('/').count() == 5)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/batch-import")
            && path.matches('/').count() == 6)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/batch-action")
            && path.matches('/').count() == 6)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/resolve-selection")
            && path.matches('/').count() == 6)
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/pool/")
            && path.contains("/keys/batch-delete-task/")
            && path.matches('/').count() == 7)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/pool/")
            && path.ends_with("/keys/cleanup-banned")
            && path.matches('/').count() == 6)
}
