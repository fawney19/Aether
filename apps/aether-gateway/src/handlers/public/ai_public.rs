use crate::async_task::CancelVideoTaskError;
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, VideoTaskQueryFilter, VideoTaskStatus,
};
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

const CLAUDE_COUNT_TOKENS_INVALID_PAYLOAD_DETAIL: &str = "Invalid token count payload";
const CLAUDE_COUNT_TOKENS_MISSING_BODY_DETAIL: &str = "请求体不能为空";
const GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL: &str = "Video task not found";
const AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL: &str = "Method not allowed";
const AI_PUBLIC_UNAUTHORIZED_DETAIL: &str = "Unauthorized";
const OPENAI_CHAT_IMAGE_MODEL_DETAIL: &str =
    "gpt-image-2 仅支持通过 /v1/images/generations 或 /v1/images/edits 调用";
const OPENAI_IMAGE_MODEL_DETAIL: &str = "图片接口当前仅支持模型 gpt-image-2";
const OPENAI_IMAGE_N_DETAIL: &str = "图片接口当前仅支持 n=1";

pub(crate) fn ai_public_local_requires_buffered_body(
    request_context: &GatewayPublicRequestContext,
) -> bool {
    request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            decision.route_class.as_deref() == Some("ai_public")
                && decision.route_family.as_deref() == Some("claude")
                && decision.route_kind.as_deref() == Some("count_tokens")
                && request_context.request_method == http::Method::POST
        })
}

pub(crate) async fn maybe_build_local_ai_public_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    if let Some(response) = maybe_build_local_ai_public_route_guard_response(request_context) {
        return Some(response);
    }

    let decision = request_context.control_decision.as_ref()?;
    if decision.route_class.as_deref() != Some("ai_public") {
        return None;
    }

    if let Some(response) =
        maybe_build_local_openai_request_validation_response(request_context, request_body)
    {
        return Some(response);
    }

    if let Some(response) =
        maybe_build_local_claude_count_tokens_response(request_context, request_body)
    {
        return Some(response);
    }

    maybe_build_local_gemini_video_operations_response(state, request_context, decision).await
}

fn maybe_build_local_openai_request_validation_response(
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("openai")
        || request_context.request_method != http::Method::POST
    {
        return None;
    }

    let request_body = request_body?;
    let payload = serde_json::from_slice::<Value>(request_body).ok()?;

    if decision.route_kind.as_deref() == Some("chat")
        && request_context.request_path == "/v1/chat/completions"
    {
        let model = payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        if model.eq_ignore_ascii_case("gpt-image-2") {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_CHAT_IMAGE_MODEL_DETAIL,
            ));
        }
        return None;
    }

    if decision.route_kind.as_deref() != Some("image")
        || !matches!(
            request_context.request_path.as_str(),
            "/v1/images/generations" | "/v1/images/edits"
        )
    {
        return None;
    }

    if let Some(model) = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !model.eq_ignore_ascii_case("gpt-image-2") {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_IMAGE_MODEL_DETAIL,
            ));
        }
    }

    if let Some(n) = payload.get("n").and_then(image_request_count) {
        if n != 1 {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                OPENAI_IMAGE_N_DETAIL,
            ));
        }
    }

    None
}

fn image_request_count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
}

fn maybe_build_local_ai_public_route_guard_response(
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    if request_context.request_path == "/upload/v1beta/files"
        && request_context.request_method != http::Method::POST
    {
        return Some(build_ai_public_error_response(
            http::StatusCode::METHOD_NOT_ALLOWED,
            AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
        ));
    }

    None
}

fn maybe_build_local_claude_count_tokens_response(
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("claude")
        || decision.route_kind.as_deref() != Some("count_tokens")
        || request_context.request_method != http::Method::POST
        || request_context.request_path != "/v1/messages/count_tokens"
    {
        return None;
    }

    let Some(request_body) = request_body else {
        return Some(build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            CLAUDE_COUNT_TOKENS_MISSING_BODY_DETAIL,
        ));
    };

    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(payload) => payload,
        Err(_) => {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                CLAUDE_COUNT_TOKENS_INVALID_PAYLOAD_DETAIL,
            ));
        }
    };

    let input_tokens = match estimate_claude_count_tokens(&payload) {
        Ok(tokens) => tokens,
        Err(_) => {
            return Some(build_ai_public_error_response(
                http::StatusCode::BAD_REQUEST,
                CLAUDE_COUNT_TOKENS_INVALID_PAYLOAD_DETAIL,
            ));
        }
    };

    Some(Json(json!({ "input_tokens": input_tokens })).into_response())
}

async fn maybe_build_local_gemini_video_operations_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    decision: &GatewayControlDecision,
) -> Option<Response<Body>> {
    if decision.route_family.as_deref() != Some("gemini")
        || decision.route_kind.as_deref() != Some("video")
    {
        return None;
    }

    if request_context.request_path == "/v1beta/operations" {
        return Some(match request_context.request_method {
            http::Method::GET => {
                build_local_gemini_video_operations_list_response(state, decision).await
            }
            _ => build_ai_public_error_response(
                http::StatusCode::METHOD_NOT_ALLOWED,
                AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
            ),
        });
    }

    let Some(operation_path) = request_context
        .request_path
        .strip_prefix("/v1beta/operations/")
    else {
        return None;
    };

    Some(match request_context.request_method {
        http::Method::GET => {
            build_local_gemini_video_operation_detail_response(state, decision, operation_path)
                .await
        }
        http::Method::POST if operation_path.ends_with(":cancel") => {
            build_local_gemini_video_operation_cancel_response(state, decision, operation_path)
                .await
        }
        _ => build_ai_public_error_response(
            http::StatusCode::METHOD_NOT_ALLOWED,
            AI_PUBLIC_METHOD_NOT_ALLOWED_DETAIL,
        ),
    })
}

async fn build_local_gemini_video_operations_list_response(
    state: &AppState,
    decision: &GatewayControlDecision,
) -> Response<Body> {
    let Some(user_id) = decision
        .auth_context
        .as_ref()
        .map(|auth_context| auth_context.user_id.trim())
        .filter(|value| !value.is_empty())
    else {
        return build_ai_public_error_response(
            http::StatusCode::UNAUTHORIZED,
            AI_PUBLIC_UNAUTHORIZED_DETAIL,
        );
    };

    let filter = VideoTaskQueryFilter {
        user_id: Some(user_id.to_string()),
        status: None,
        model_substring: None,
        client_api_format: Some("gemini:video".to_string()),
    };
    let tasks = match state.list_video_task_page(&filter, 0, 100).await {
        Ok(tasks) => tasks,
        Err(err) => {
            return build_ai_public_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("{err:?}"),
            );
        }
    };
    let operations = tasks
        .into_iter()
        .filter(is_gemini_video_task)
        .map(|task| build_gemini_video_operation_payload(&task))
        .collect::<Vec<_>>();

    Json(json!({ "operations": operations })).into_response()
}

async fn build_local_gemini_video_operation_detail_response(
    state: &AppState,
    decision: &GatewayControlDecision,
    operation_path: &str,
) -> Response<Body> {
    let task =
        match find_user_gemini_video_task_for_operation(state, decision, operation_path).await {
            Ok(Some(task)) => task,
            Ok(None) => {
                return build_ai_public_error_response(
                    http::StatusCode::NOT_FOUND,
                    GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL,
                );
            }
            Err(err) => {
                return build_ai_public_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{err:?}"),
                );
            }
        };

    Json(build_gemini_video_operation_payload(&task)).into_response()
}

async fn build_local_gemini_video_operation_cancel_response(
    state: &AppState,
    decision: &GatewayControlDecision,
    operation_path: &str,
) -> Response<Body> {
    let task =
        match find_user_gemini_video_task_for_operation(state, decision, operation_path).await {
            Ok(Some(task)) => task,
            Ok(None) => {
                return build_ai_public_error_response(
                    http::StatusCode::NOT_FOUND,
                    GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL,
                );
            }
            Err(err) => {
                return build_ai_public_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{err:?}"),
                );
            }
        };

    match crate::async_task::cancel_video_task_record(state, &task.id).await {
        Ok(_) => Json(json!({})).into_response(),
        Err(CancelVideoTaskError::NotFound) => build_ai_public_error_response(
            http::StatusCode::NOT_FOUND,
            GEMINI_VIDEO_TASK_NOT_FOUND_DETAIL,
        ),
        Err(CancelVideoTaskError::InvalidStatus(status)) => build_ai_public_error_response(
            http::StatusCode::BAD_REQUEST,
            format!(
                "Cannot cancel task with status: {}",
                video_task_status_name(status)
            ),
        ),
        Err(CancelVideoTaskError::Response(response)) => response,
        Err(CancelVideoTaskError::Gateway(err)) => build_ai_public_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{err:?}"),
        ),
    }
}

async fn find_user_gemini_video_task_for_operation(
    state: &AppState,
    decision: &GatewayControlDecision,
    operation_path: &str,
) -> Result<Option<StoredVideoTask>, GatewayError> {
    let Some(user_id) = decision
        .auth_context
        .as_ref()
        .map(|auth_context| auth_context.user_id.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let Some(short_id) = extract_short_id_from_gemini_operation_path(operation_path) else {
        return Ok(None);
    };
    let Some(task) = state.find_video_task_by_short_id(short_id).await? else {
        return Ok(None);
    };
    if task.user_id.as_deref().map(str::trim) != Some(user_id) || !is_gemini_video_task(&task) {
        return Ok(None);
    }
    Ok(Some(task))
}

fn extract_short_id_from_gemini_operation_path(operation_path: &str) -> Option<&str> {
    let trimmed = operation_path.trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let short_id = trimmed
        .strip_suffix(":cancel")
        .unwrap_or(trimmed)
        .rsplit('/')
        .next()?;
    (!short_id.is_empty()).then_some(short_id)
}

fn is_gemini_video_task(task: &StoredVideoTask) -> bool {
    matches!(
        task.provider_api_format
            .as_deref()
            .or(task.client_api_format.as_deref())
            .map(str::trim),
        Some("gemini:video")
    )
}

fn build_gemini_video_operation_payload(task: &StoredVideoTask) -> serde_json::Value {
    match task.status {
        VideoTaskStatus::Completed => json!({
            "name": gemini_video_operation_name(task),
            "done": true,
            "response": {
                "generateVideoResponse": {
                    "generatedSamples": [
                        {
                            "video": {
                                "uri": format!(
                                    "/v1beta/files/aev_{}:download?alt=media",
                                    gemini_operation_short_id(task)
                                ),
                                "mimeType": "video/mp4",
                            }
                        }
                    ]
                }
            }
        }),
        VideoTaskStatus::Failed | VideoTaskStatus::Expired => json!({
            "name": gemini_video_operation_name(task),
            "done": true,
            "error": {
                "code": task.error_code.clone().unwrap_or_else(|| "UNKNOWN".to_string()),
                "message": task
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Video generation failed".to_string()),
            }
        }),
        _ => json!({
            "name": gemini_video_operation_name(task),
            "done": false,
            "metadata": gemini_video_operation_metadata(task),
        }),
    }
}

fn gemini_video_operation_name(task: &StoredVideoTask) -> String {
    format!(
        "models/{}/operations/{}",
        gemini_operation_model(task),
        gemini_operation_short_id(task)
    )
}

fn gemini_operation_model(task: &StoredVideoTask) -> String {
    task.model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            task.external_task_id.as_deref().and_then(|external_id| {
                let parts = external_id.split('/').collect::<Vec<_>>();
                if parts.len() >= 2 && parts[0] == "models" && !parts[1].trim().is_empty() {
                    Some(parts[1].trim().to_string())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn gemini_operation_short_id(task: &StoredVideoTask) -> String {
    task.short_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(task.id.as_str())
        .to_string()
}

fn gemini_video_operation_metadata(task: &StoredVideoTask) -> serde_json::Value {
    task.request_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("rust_local_snapshot"))
        .and_then(|snapshot| snapshot.get("Gemini"))
        .and_then(|gemini| gemini.get("metadata"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn video_task_status_name(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending => "pending",
        VideoTaskStatus::Submitted => "submitted",
        VideoTaskStatus::Queued => "queued",
        VideoTaskStatus::Processing => "processing",
        VideoTaskStatus::Completed => "completed",
        VideoTaskStatus::Failed => "failed",
        VideoTaskStatus::Cancelled => "cancelled",
        VideoTaskStatus::Expired => "expired",
        VideoTaskStatus::Deleted => "deleted",
    }
}

fn build_ai_public_error_response(
    status: http::StatusCode,
    detail: impl Into<String>,
) -> Response<Body> {
    (status, Json(json!({ "detail": detail.into() }))).into_response()
}

fn estimate_claude_count_tokens(payload: &serde_json::Value) -> Result<u64, ()> {
    let object = payload.as_object().ok_or(())?;
    let model = object
        .get("model")
        .and_then(serde_json::Value::as_str)
        .ok_or(())?;
    if model.trim().is_empty() {
        return Err(());
    }

    let messages = object
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .ok_or(())?;

    let system_tokens = estimate_claude_system_tokens(object.get("system"))?;
    let message_tokens = estimate_claude_message_tokens(messages)?;
    Ok(system_tokens.saturating_add(message_tokens))
}

fn estimate_claude_system_tokens(system: Option<&serde_json::Value>) -> Result<u64, ()> {
    let Some(system) = system else {
        return Ok(0);
    };

    match system {
        serde_json::Value::Null => Ok(0),
        serde_json::Value::String(text) => Ok(estimate_text_tokens(text)),
        serde_json::Value::Array(blocks) => {
            let mut total = 0_u64;
            for block in blocks {
                let block = block.as_object().ok_or(())?;
                if let Some(text) = block.get("text").and_then(serde_json::Value::as_str) {
                    total = total.saturating_add(estimate_text_tokens(text));
                }
            }
            Ok(total)
        }
        serde_json::Value::Object(_) => Ok(0),
        _ => Err(()),
    }
}

fn estimate_claude_message_tokens(messages: &[serde_json::Value]) -> Result<u64, ()> {
    let mut total = 0_u64;

    for message in messages {
        let message = message.as_object().ok_or(())?;
        let role = message
            .get("role")
            .and_then(serde_json::Value::as_str)
            .ok_or(())?;
        if !matches!(role, "user" | "assistant") {
            return Err(());
        }

        total = total.saturating_add(4);
        let content = message.get("content").ok_or(())?;
        match content {
            serde_json::Value::String(text) => {
                total = total.saturating_add(estimate_text_tokens(text));
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    let item = item.as_object().ok_or(())?;
                    if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                        total = total.saturating_add(estimate_text_tokens(text));
                    }
                }
            }
            _ => return Err(()),
        }
    }

    Ok(total)
}

fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }

    let char_count = text.chars().count() as u64;
    std::cmp::max(1, char_count / 4)
}

#[cfg(test)]
mod tests {
    use super::estimate_claude_count_tokens;
    use serde_json::json;

    #[test]
    fn estimates_claude_count_tokens_from_system_and_messages() {
        let payload = json!({
            "model": "claude-sonnet-4-5",
            "system": [{"type": "text", "text": "abcdefghijklmnop"}],
            "messages": [
                {
                    "role": "user",
                    "content": "abcdefghijkl"
                },
                {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "abcdefgh"},
                        {"type": "tool_use", "name": "ignored", "input": {"city": "SF"}}
                    ]
                }
            ]
        });

        assert_eq!(estimate_claude_count_tokens(&payload), Ok(17));
    }

    #[test]
    fn rejects_invalid_claude_count_tokens_payload() {
        let payload = json!({
            "model": "claude-sonnet-4-5",
            "messages": [{"role": "system", "content": "bad"}]
        });

        assert_eq!(estimate_claude_count_tokens(&payload), Err(()));
    }
}
