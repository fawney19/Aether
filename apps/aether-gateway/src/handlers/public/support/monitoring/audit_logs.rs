use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde_json::{json, Value};

use crate::handlers::shared::query_param_value;

use super::{
    build_auth_error_response, resolve_authenticated_local_user, AppState,
    GatewayPublicRequestContext,
};

fn parse_user_monitoring_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer between 1 and 200".to_string())?;
            if (1..=200).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("limit must be an integer between 1 and 200".to_string())
            }
        }
        None => Ok(50),
    }
}

fn parse_user_monitoring_offset(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "offset") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "offset must be a non-negative integer".to_string()),
        None => Ok(0),
    }
}

fn parse_user_monitoring_days(query: Option<&str>) -> Result<i64, String> {
    match query_param_value(query, "days") {
        Some(value) => {
            let parsed = value
                .parse::<i64>()
                .map_err(|_| "days must be an integer between 1 and 365".to_string())?;
            if (1..=365).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("days must be an integer between 1 and 365".to_string())
            }
        }
        None => Ok(30),
    }
}

fn build_user_monitoring_audit_logs_payload(
    items: Vec<Value>,
    total: usize,
    limit: usize,
    offset: usize,
    event_type: Option<String>,
    days: i64,
) -> Response<Body> {
    Json(json!({
        "items": items,
        "meta": {
            "total": total,
            "limit": limit,
            "offset": offset,
            "count": items.len(),
        },
        "filters": {
            "event_type": event_type,
            "days": days,
        }
    }))
    .into_response()
}

pub(super) async fn handle_user_audit_logs(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };

    let query = request_context.request_query_string.as_deref();
    let event_type = query_param_value(query, "event_type").map(|value| value.trim().to_string());
    let event_type = event_type.filter(|value| !value.is_empty());
    let limit = match parse_user_monitoring_limit(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let offset = match parse_user_monitoring_offset(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let days = match parse_user_monitoring_days(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };

    let cutoff_time = Utc::now() - chrono::Duration::days(days);
    let (items, total) = match state
        .list_user_audit_logs(
            &auth.user.id,
            cutoff_time,
            event_type.as_deref(),
            limit,
            offset,
        )
        .await
    {
        Ok(value) => value,
        Err(_err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                "audit logs unavailable",
                false,
            );
        }
    };

    build_user_monitoring_audit_logs_payload(items, total, limit, offset, event_type, days)
}
