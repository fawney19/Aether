use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use aether_data::repository::announcements::{
    AnnouncementListQuery, StoredAnnouncement, StoredAnnouncementPage,
};

use crate::handlers::shared::{query_param_optional_bool, query_param_value};
use crate::GatewayError;

pub(super) fn parse_public_announcements_query(
    query: Option<&str>,
    default_active_only: bool,
    default_limit: usize,
) -> Result<AnnouncementListQuery, String> {
    let active_only =
        query_param_optional_bool(query, "active_only").unwrap_or(default_active_only);
    let offset = query_param_value(query, "offset")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "offset must be a non-negative integer".to_string())
        })
        .transpose()?
        .unwrap_or(0);
    let limit = query_param_value(query, "limit")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "limit must be between 1 and 100".to_string())
                .and_then(|parsed| {
                    if (1..=100).contains(&parsed) {
                        Ok(parsed)
                    } else {
                        Err("limit must be between 1 and 100".to_string())
                    }
                })
        })
        .transpose()?
        .unwrap_or(default_limit);
    let now_unix_secs = active_only.then(|| chrono::Utc::now().timestamp().max(0) as u64);

    Ok(AnnouncementListQuery {
        active_only,
        offset,
        limit,
        now_unix_secs,
    })
}

pub(super) fn build_public_announcement_list_payload(
    page: StoredAnnouncementPage,
) -> serde_json::Value {
    let items = page
        .items
        .iter()
        .map(build_public_announcement_payload)
        .collect::<Vec<_>>();
    json!({
        "items": items,
        "total": page.total,
    })
}

pub(super) fn build_public_announcement_payload(
    announcement: &StoredAnnouncement,
) -> serde_json::Value {
    json!({
        "id": announcement.id,
        "title": announcement.title,
        "content": announcement.content,
        "type": announcement.kind,
        "priority": announcement.priority,
        "is_active": announcement.is_active,
        "is_pinned": announcement.is_pinned,
        "author": {
            "id": announcement.author_id,
            "username": announcement.author_username,
        },
        "start_time": format_optional_unix_datetime(announcement.start_time_unix_secs),
        "end_time": format_optional_unix_datetime(announcement.end_time_unix_secs),
        "created_at": format_required_unix_datetime(announcement.created_at_unix_ms),
        "updated_at": format_required_unix_datetime(announcement.updated_at_unix_secs),
    })
}

pub(super) fn public_announcement_id_from_path(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    let announcement_id = trimmed.strip_prefix("/api/announcements/")?;
    if announcement_id.is_empty() || announcement_id.contains('/') {
        None
    } else {
        Some(announcement_id)
    }
}

pub(super) fn read_status_announcement_id_from_path(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix("/read-status")?;
    let announcement_id = trimmed.strip_prefix("/api/announcements/")?;
    if announcement_id.is_empty() || announcement_id.contains('/') {
        None
    } else {
        Some(announcement_id)
    }
}

pub(super) fn announcements_bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn announcements_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Announcement not found" })),
    )
        .into_response()
}

pub(super) fn announcements_internal_error_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(super) fn announcements_internal_detail(err: GatewayError) -> String {
    match err {
        GatewayError::UpstreamUnavailable { message, .. }
        | GatewayError::ControlUnavailable { message, .. }
        | GatewayError::Client { message, .. }
        | GatewayError::Internal(message) => message,
    }
}

pub(super) fn parse_optional_rfc3339_unix_secs(
    value: Option<&str>,
    field_name: &str,
) -> Result<Option<u64>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|datetime| datetime.timestamp().max(0) as u64)
        .map(Some)
        .map_err(|_| format!("{field_name} must be a valid RFC3339 datetime"))
}

fn format_required_unix_datetime(unix_secs: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_secs as i64, 0)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

fn format_optional_unix_datetime(unix_secs: Option<u64>) -> Option<String> {
    unix_secs.map(format_required_unix_datetime)
}
