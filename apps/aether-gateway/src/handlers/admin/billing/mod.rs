use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_value, unix_secs_to_rfc3339};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use regex::Regex;
use serde_json::json;
use sqlx::Row;

const ADMIN_BILLING_DATA_UNAVAILABLE_DETAIL: &str = "Admin billing data unavailable";

mod collectors;
mod payments;
mod presets;
mod routes;
mod rules;
mod wallets;

pub(super) use self::payments::maybe_build_local_admin_payments_response;
pub(super) use self::routes::maybe_build_local_admin_billing_routes_response;
pub(super) use self::wallets::maybe_build_local_admin_wallets_response;

fn default_admin_billing_true() -> bool {
    true
}

fn default_admin_billing_json_object() -> serde_json::Value {
    json!({})
}

fn build_admin_billing_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_BILLING_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

fn build_admin_billing_bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn build_admin_billing_read_only_response(detail: &'static str) -> Response<Body> {
    (
        http::StatusCode::CONFLICT,
        Json(json!({
            "detail": detail,
            "error_code": "read_only_mode",
        })),
    )
        .into_response()
}

fn build_admin_billing_not_found_response(detail: &'static str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

fn admin_billing_parse_page(query: Option<&str>) -> Result<u32, String> {
    match query_param_value(query, "page") {
        None => Ok(1),
        Some(value) => {
            let parsed = value
                .parse::<u32>()
                .map_err(|_| "page must be between 1 and 100000".to_string())?;
            if !(1..=100_000).contains(&parsed) {
                return Err("page must be between 1 and 100000".to_string());
            }
            Ok(parsed)
        }
    }
}

fn admin_billing_parse_page_size(query: Option<&str>) -> Result<u32, String> {
    match query_param_value(query, "page_size") {
        None => Ok(50),
        Some(value) => {
            let parsed = value
                .parse::<u32>()
                .map_err(|_| "page_size must be between 1 and 200".to_string())?;
            if !(1..=200).contains(&parsed) {
                return Err("page_size must be between 1 and 200".to_string());
            }
            Ok(parsed)
        }
    }
}

fn admin_billing_optional_filter(query: Option<&str>, key: &str) -> Option<String> {
    query_param_value(query, key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn admin_billing_optional_bool_filter(
    query: Option<&str>,
    key: &str,
) -> Result<Option<bool>, String> {
    match query_param_value(query, key) {
        None => Ok(None),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(Some(true)),
            "false" | "0" | "no" => Ok(Some(false)),
            _ => Err(format!("{key} must be a boolean")),
        },
    }
}

fn admin_billing_pages(total: u64, page_size: u32) -> u64 {
    if total == 0 {
        0
    } else {
        total.div_ceil(u64::from(page_size))
    }
}

fn normalize_admin_billing_required_text(
    value: &str,
    field: &str,
    max_len: usize,
) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field} must be a non-empty string"));
    }
    if value.len() > max_len {
        return Err(format!("{field} exceeds maximum length {max_len}"));
    }
    Ok(value.to_string())
}

fn normalize_admin_billing_optional_text(
    value: Option<String>,
    max_len: usize,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > max_len {
        return Err(format!("field exceeds maximum length {max_len}"));
    }
    Ok(Some(trimmed.to_string()))
}

fn admin_billing_validate_safe_expression(expression: &str) -> Result<(), String> {
    let expression = expression.trim();
    if expression.is_empty() {
        return Err("expression must not be empty".to_string());
    }
    if expression.contains("__") {
        return Err("Dunder names are not allowed".to_string());
    }

    let allowed_chars = Regex::new(r"^[A-Za-z0-9_+\-*/%().,\s]+$").expect("regex should compile");
    if !allowed_chars.is_match(expression) {
        return Err("Expression contains unsupported characters".to_string());
    }

    let identifier = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("regex should compile");
    const ALLOWED_FUNCTIONS: &[&str] = &["min", "max", "abs", "round", "int", "float"];
    for matched in identifier.find_iter(expression) {
        let name = matched.as_str();
        let next_non_ws = expression[matched.end()..]
            .chars()
            .find(|value| !value.is_whitespace());
        if next_non_ws == Some('(') && !ALLOWED_FUNCTIONS.iter().any(|value| value == &name) {
            return Err(format!("Function not allowed: {name}"));
        }
    }
    Ok(())
}

fn admin_billing_optional_epoch_value(
    row: &sqlx::postgres::PgRow,
    field: &str,
) -> Result<Option<String>, GatewayError> {
    let value = row
        .try_get::<Option<i64>, _>(field)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    match value {
        None => Ok(None),
        Some(value) if value < 0 => Ok(None),
        Some(value) => Ok(unix_secs_to_rfc3339(value as u64)),
    }
}

pub(crate) async fn maybe_build_local_admin_billing_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("billing_manage") {
        return Ok(None);
    }

    let path = request_context.path();
    let is_billing_route = (request_context.method() == http::Method::GET
        && matches!(
            path,
            "/api/admin/billing/presets" | "/api/admin/billing/presets/"
        ))
        || (request_context.method() == http::Method::POST
            && matches!(
                path,
                "/api/admin/billing/presets/apply" | "/api/admin/billing/presets/apply/"
            ))
        || (request_context.method() == http::Method::GET
            && matches!(
                path,
                "/api/admin/billing/rules" | "/api/admin/billing/rules/"
            ))
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/billing/rules/")
            && path.matches('/').count() == 5)
        || (request_context.method() == http::Method::POST
            && matches!(
                path,
                "/api/admin/billing/rules" | "/api/admin/billing/rules/"
            ))
        || (request_context.method() == http::Method::PUT
            && path.starts_with("/api/admin/billing/rules/")
            && path.matches('/').count() == 5)
        || (request_context.method() == http::Method::GET
            && matches!(
                path,
                "/api/admin/billing/collectors" | "/api/admin/billing/collectors/"
            ))
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/billing/collectors/")
            && path.matches('/').count() == 5)
        || (request_context.method() == http::Method::POST
            && matches!(
                path,
                "/api/admin/billing/collectors" | "/api/admin/billing/collectors/"
            ))
        || (request_context.method() == http::Method::PUT
            && path.starts_with("/api/admin/billing/collectors/")
            && path.matches('/').count() == 5);

    if !is_billing_route {
        return Ok(None);
    }

    if let Some(response) = presets::maybe_build_local_admin_billing_presets_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) =
        rules::maybe_build_local_admin_billing_rules_response(state, request_context, request_body)
            .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) = collectors::maybe_build_local_admin_billing_collectors_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }

    let _ = decision.route_kind.as_deref();
    Ok(Some(build_admin_billing_data_unavailable_response()))
}
