use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::control::GatewayPublicRequestContext;
use crate::AppState;

use super::super::{build_unhandled_public_support_response, resolve_authenticated_local_user};
use super::announcements_shared::{
    announcement_is_public_at, announcements_bad_request_response, announcements_internal_detail,
    announcements_internal_error_response, announcements_not_found_response,
    build_public_announcement_payload, read_status_announcement_id_from_path,
};

#[derive(Debug, serde::Deserialize)]
struct AnnouncementReadStatusRequest {
    is_read: bool,
}

fn parse_announcement_read_status_request(
    request_body: Option<&axum::body::Bytes>,
) -> Result<AnnouncementReadStatusRequest, Response<Body>> {
    match request_body {
        Some(body) if !body.is_empty() => {
            serde_json::from_slice::<AnnouncementReadStatusRequest>(body)
                .map_err(|_| announcements_bad_request_response("invalid read-status payload"))
        }
        _ => Ok(AnnouncementReadStatusRequest { is_read: true }),
    }
}

pub(crate) async fn maybe_build_local_announcement_user_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("announcement_user") {
        return None;
    }
    if !state.has_announcement_data_reader() {
        return None;
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return Some(response),
    };
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;

    match decision.route_kind.as_deref() {
        Some("unread_count")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/announcements/users/me/unread-count"
                        | "/api/announcements/users/me/unread-count/"
                ) =>
        {
            let unread_count = match state
                .count_unread_active_announcements(&auth.user.id, now_unix_secs)
                .await
            {
                Ok(value) => value,
                Err(err) => {
                    return Some(announcements_internal_error_response(
                        announcements_internal_detail(err),
                    ))
                }
            };
            Some(Json(json!({ "unread_count": unread_count })).into_response())
        }
        Some("required_unread")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/announcements/users/me/required-unread"
                        | "/api/announcements/users/me/required-unread/"
                ) =>
        {
            let items = match state
                .list_required_unread_active_announcements(&auth.user.id, now_unix_secs, 20)
                .await
            {
                Ok(value) => value,
                Err(err) => {
                    return Some(announcements_internal_error_response(
                        announcements_internal_detail(err),
                    ))
                }
            };
            let payload_items = items
                .iter()
                .map(build_public_announcement_payload)
                .collect::<Vec<_>>();
            Some(
                Json(json!({
                    "items": payload_items,
                    "total": payload_items.len(),
                }))
                .into_response(),
            )
        }
        Some("read_all")
            if request_context.request_method == http::Method::POST
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/announcements/read-all" | "/api/announcements/read-all/"
                ) =>
        {
            if let Err(response) =
                mark_all_announcements_read(state, &auth.user.id, now_unix_secs).await
            {
                return Some(response);
            }
            Some(Json(json!({ "message": "已全部标记为已读" })).into_response())
        }
        Some("read_status") if request_context.request_method == http::Method::PATCH => {
            let request = match parse_announcement_read_status_request(request_body) {
                Ok(value) => value,
                Err(response) => return Some(response),
            };
            if !request.is_read {
                return Some(announcements_bad_request_response(
                    "is_read must be true for read-status updates",
                ));
            }
            let announcement_id =
                match read_status_announcement_id_from_path(&request_context.request_path) {
                    Some(value) => value,
                    None => return Some(build_unhandled_public_support_response(request_context)),
                };
            match state.find_announcement_by_id(announcement_id).await {
                Ok(Some(announcement))
                    if announcement_is_public_at(&announcement, now_unix_secs) => {}
                Ok(Some(_)) => return Some(announcements_not_found_response()),
                Ok(None) => return Some(announcements_not_found_response()),
                Err(err) => {
                    return Some(announcements_internal_error_response(
                        announcements_internal_detail(err),
                    ))
                }
            }
            if let Err(err) = state
                .mark_announcement_as_read(&auth.user.id, announcement_id, now_unix_secs)
                .await
            {
                return Some(announcements_internal_error_response(
                    announcements_internal_detail(err),
                ));
            }
            Some(Json(json!({ "message": "公告已标记为已读" })).into_response())
        }
        _ => Some(build_unhandled_public_support_response(request_context)),
    }
}

async fn mark_all_announcements_read(
    state: &AppState,
    user_id: &str,
    now_unix_secs: u64,
) -> Result<(), Response<Body>> {
    let mut offset = 0usize;
    const PAGE_SIZE: usize = 200;

    loop {
        let page = state
            .list_announcements(
                &aether_data::repository::announcements::AnnouncementListQuery {
                    active_only: true,
                    offset,
                    limit: PAGE_SIZE,
                    now_unix_secs: Some(now_unix_secs),
                },
            )
            .await
            .map_err(|err| {
                announcements_internal_error_response(announcements_internal_detail(err))
            })?;

        if page.items.is_empty() {
            return Ok(());
        }

        for announcement in &page.items {
            state
                .mark_announcement_as_read(user_id, &announcement.id, now_unix_secs)
                .await
                .map_err(|err| {
                    announcements_internal_error_response(announcements_internal_detail(err))
                })?;
        }

        offset += page.items.len();
        if offset >= page.total as usize {
            return Ok(());
        }
    }
}
