use crate::handlers::admin::request::{
    AdminAppState, AdminCancelVideoTaskError, AdminRequestContext,
};
use crate::handlers::admin::shared::{
    attach_admin_audit_response, build_proxy_error_response, query_param_value,
};
use crate::GatewayError;
use aether_data_contracts::repository::video_tasks::{VideoTaskQueryFilter, VideoTaskStatus};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::builders::{
    admin_video_task_detail_id_from_path, admin_video_task_nested_id_from_path,
    admin_video_task_status_name, admin_video_task_timestamp, build_admin_video_task_list_item,
    build_admin_video_task_provider_names, current_admin_video_task_unix_secs,
};

pub(super) async fn maybe_build_local_admin_video_tasks_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_family() != Some("video_tasks_manage") {
        return Ok(None);
    }

    if request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/video-tasks" | "/api/admin/video-tasks/"
        )
    {
        let status = match query_param_value(request_context.query_string(), "status") {
            Some(value) => match VideoTaskStatus::from_database(&value) {
                Ok(status) => Some(status),
                Err(err) => {
                    return Ok(Some(build_proxy_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid_request",
                        err.to_string(),
                        None,
                    )));
                }
            },
            None => None,
        };
        let filter = VideoTaskQueryFilter {
            user_id: query_param_value(request_context.query_string(), "user_id"),
            status,
            model_substring: query_param_value(request_context.query_string(), "model"),
            client_api_format: None,
        };
        let page = query_param_value(request_context.query_string(), "page")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let page_size = query_param_value(request_context.query_string(), "page_size")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(20);
        let response = state
            .read_video_task_page_summary(&filter, page, page_size)
            .await?;
        let provider_names = build_admin_video_task_provider_names(state, &response.items).await?;
        return Ok(Some(
            Json(json!({
                "items": response
                    .items
                    .iter()
                    .map(|task| build_admin_video_task_list_item(task, &provider_names))
                    .collect::<Vec<_>>(),
                "total": response.total,
                "page": response.page,
                "page_size": response.page_size,
                "pages": response.pages,
            }))
            .into_response(),
        ));
    }

    if request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/video-tasks/stats" | "/api/admin/video-tasks/stats/"
        )
    {
        let filter = VideoTaskQueryFilter {
            user_id: None,
            status: None,
            model_substring: None,
            client_api_format: None,
        };
        let stats = state
            .read_video_task_stats(&filter, current_admin_video_task_unix_secs())
            .await?;
        let active_users = state.count_distinct_video_task_users(&filter).await?;
        return Ok(Some(
            Json(json!({
                "total": stats.total,
                "by_status": stats.by_status,
                "by_model": stats.by_model,
                "today_count": stats.today_count,
                "active_users": active_users,
                "processing_count": stats.processing_count,
            }))
            .into_response(),
        ));
    }

    if request_context.method() == http::Method::POST {
        let Some(task_id) = admin_video_task_nested_id_from_path(request_context.path(), "/cancel")
        else {
            return Ok(None);
        };
        let stored = match state.cancel_video_task_record(task_id).await {
            Ok(stored) => stored,
            Err(AdminCancelVideoTaskError::NotFound) => {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": "Video task not found" })),
                    )
                        .into_response(),
                ));
            }
            Err(AdminCancelVideoTaskError::InvalidStatus(status)) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({
                            "detail": format!(
                                "Cannot cancel task with status: {}",
                                admin_video_task_status_name(status),
                            ),
                        })),
                    )
                        .into_response(),
                ));
            }
            Err(AdminCancelVideoTaskError::Response(response)) => {
                return Ok(Some(response));
            }
            Err(AdminCancelVideoTaskError::Gateway(err)) => {
                return Err(err);
            }
        };
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "id": stored.id,
                "status": "cancelled",
                "message": "Task cancelled successfully",
            }))
            .into_response(),
            "admin_video_task_cancelled",
            "cancel_video_task",
            "video_task",
            &stored.id,
        )));
    }

    if request_context.method() == http::Method::GET {
        let Some(task_id) = admin_video_task_nested_id_from_path(request_context.path(), "/video")
        else {
            let Some(task_id) = admin_video_task_detail_id_from_path(request_context.path()) else {
                return Ok(None);
            };
            let Some(task) = state.read_video_task_detail(task_id).await? else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": "Video task not found" })),
                    )
                        .into_response(),
                ));
            };
            let provider_names =
                build_admin_video_task_provider_names(state, std::slice::from_ref(&task)).await?;
            let provider_name = task
                .provider_id
                .as_deref()
                .and_then(|provider_id| provider_names.get(provider_id))
                .cloned()
                .unwrap_or_else(|| "Unknown".to_string());
            let endpoint = if let Some(endpoint_id) = task.endpoint_id.as_ref() {
                let endpoints = state
                    .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(endpoint_id))
                    .await?;
                endpoints.into_iter().next()
            } else {
                None
            };
            let endpoint_payload = endpoint.map(|endpoint| {
                json!({
                    "id": endpoint.id,
                    "base_url": endpoint.base_url,
                    "api_format": endpoint.api_format,
                })
            });
            let mut payload = serde_json::Map::new();
            payload.insert("id".to_string(), json!(task.id));
            payload.insert("external_task_id".to_string(), json!(task.external_task_id));
            payload.insert("user_id".to_string(), json!(task.user_id));
            payload.insert(
                "username".to_string(),
                json!(task
                    .username
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string())),
            );
            payload.insert("api_key_id".to_string(), json!(task.api_key_id));
            payload.insert("provider_id".to_string(), json!(task.provider_id));
            payload.insert("provider_name".to_string(), json!(provider_name));
            payload.insert("endpoint_id".to_string(), json!(task.endpoint_id));
            payload.insert(
                "endpoint".to_string(),
                endpoint_payload.unwrap_or(serde_json::Value::Null),
            );
            payload.insert("key_id".to_string(), json!(task.key_id));
            payload.insert(
                "client_api_format".to_string(),
                json!(task.client_api_format),
            );
            payload.insert(
                "provider_api_format".to_string(),
                json!(task.provider_api_format),
            );
            payload.insert("format_converted".to_string(), json!(task.format_converted));
            payload.insert("model".to_string(), json!(task.model));
            payload.insert("prompt".to_string(), json!(task.prompt));
            payload.insert(
                "original_request_body".to_string(),
                json!(task.original_request_body),
            );
            payload.insert(
                "converted_request_body".to_string(),
                serde_json::Value::Null,
            );
            payload.insert("duration_seconds".to_string(), json!(task.duration_seconds));
            payload.insert("resolution".to_string(), json!(task.resolution));
            payload.insert("aspect_ratio".to_string(), json!(task.aspect_ratio));
            payload.insert("size".to_string(), json!(task.size));
            payload.insert(
                "status".to_string(),
                json!(admin_video_task_status_name(task.status)),
            );
            payload.insert("progress_percent".to_string(), json!(task.progress_percent));
            payload.insert("progress_message".to_string(), json!(task.progress_message));
            payload.insert("video_url".to_string(), json!(task.video_url));
            payload.insert("video_urls".to_string(), serde_json::Value::Null);
            payload.insert("thumbnail_url".to_string(), serde_json::Value::Null);
            payload.insert("video_size_bytes".to_string(), serde_json::Value::Null);
            payload.insert(
                "video_duration_seconds".to_string(),
                serde_json::Value::Null,
            );
            payload.insert("video_expires_at".to_string(), serde_json::Value::Null);
            payload.insert("stored_video_path".to_string(), serde_json::Value::Null);
            payload.insert("storage_provider".to_string(), serde_json::Value::Null);
            payload.insert("error_code".to_string(), json!(task.error_code));
            payload.insert("error_message".to_string(), json!(task.error_message));
            payload.insert("retry_count".to_string(), json!(task.retry_count));
            payload.insert("max_retries".to_string(), serde_json::Value::Null);
            payload.insert(
                "poll_interval_seconds".to_string(),
                json!(task.poll_interval_seconds),
            );
            payload.insert(
                "next_poll_at".to_string(),
                json!(admin_video_task_timestamp(task.next_poll_at_unix_secs)),
            );
            payload.insert("poll_count".to_string(), json!(task.poll_count));
            payload.insert("max_poll_count".to_string(), json!(task.max_poll_count));
            payload.insert(
                "created_at".to_string(),
                json!(admin_video_task_timestamp(Some(task.created_at_unix_ms))),
            );
            payload.insert(
                "updated_at".to_string(),
                json!(admin_video_task_timestamp(Some(task.updated_at_unix_secs))),
            );
            payload.insert(
                "submitted_at".to_string(),
                json!(admin_video_task_timestamp(task.submitted_at_unix_secs)),
            );
            payload.insert(
                "completed_at".to_string(),
                json!(admin_video_task_timestamp(task.completed_at_unix_secs)),
            );
            payload.insert("request_metadata".to_string(), json!(task.request_metadata));

            return Ok(Some(attach_admin_audit_response(
                Json(serde_json::Value::Object(payload)).into_response(),
                "admin_video_task_detail_viewed",
                "view_video_task_detail",
                "video_task",
                &task.id,
            )));
        };
        let Some(task) = state.read_video_task_detail(task_id).await? else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Video task not found" })),
                )
                    .into_response(),
            ));
        };
        if task
            .video_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Video not available" })),
                )
                    .into_response(),
            ));
        }
        let Some(source) = state.read_video_task_video_source(task_id).await? else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Video not available" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(attach_admin_audit_response(
            state
                .build_video_task_video_response(task_id, source)
                .await?,
            "admin_video_task_video_viewed",
            "view_video_task_video",
            "video_task_video",
            task_id,
        )));
    }

    Ok(None)
}
