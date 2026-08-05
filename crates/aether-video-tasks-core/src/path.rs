use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::video_tasks::VideoTaskLookupKey;
use aether_data_contracts::repository::video_tasks::VideoTaskStatus as StoredVideoTaskStatus;
use serde_json::Value;
use uuid::Uuid;

use crate::{LocalVideoTaskRegistryMutation, LocalVideoTaskStatus, VideoTaskTruthSourceMode};

pub fn extract_openai_task_id_from_path(path: &str) -> Option<&str> {
    let suffix = path.strip_prefix("/v1/videos/")?;
    if suffix.is_empty()
        || suffix.contains('/')
        || suffix.ends_with(":cancel")
        || suffix.ends_with(":delete")
    {
        return None;
    }
    Some(suffix)
}

pub fn extract_gemini_short_id_from_path(path: &str) -> Option<&str> {
    let operations_index = path.find("/operations/")?;
    let suffix = &path[(operations_index + "/operations/".len())..];
    if suffix.is_empty() || suffix.contains('/') || suffix.ends_with(":cancel") {
        return None;
    }
    Some(suffix)
}

pub fn extract_openai_task_id_from_cancel_path(path: &str) -> Option<&str> {
    let suffix = path.strip_prefix("/v1/videos/")?;
    suffix
        .strip_suffix("/cancel")
        .filter(|value| !value.is_empty())
}

pub fn extract_openai_task_id_from_remix_path(path: &str) -> Option<&str> {
    let suffix = path.strip_prefix("/v1/videos/")?;
    suffix
        .strip_suffix("/remix")
        .filter(|value| !value.is_empty())
}

pub fn extract_openai_task_id_from_content_path(path: &str) -> Option<&str> {
    let suffix = path.strip_prefix("/v1/videos/")?;
    suffix
        .strip_suffix("/content")
        .filter(|value| !value.is_empty())
}

pub fn extract_gemini_short_id_from_cancel_path(path: &str) -> Option<&str> {
    let operations_index = path.find("/operations/")?;
    let suffix = &path[(operations_index + "/operations/".len())..];
    let short_id = suffix.strip_suffix(":cancel")?;
    if short_id.is_empty() || short_id.contains('/') {
        return None;
    }
    Some(short_id)
}

/// Client-facing root of the Doubao (Volcengine Ark) generation task surface.
pub const DOUBAO_VIDEO_TASKS_PATH: &str = "/v3/contents/generations/tasks";

pub fn extract_doubao_task_id_from_path(path: &str) -> Option<&str> {
    let suffix = path
        .strip_prefix(DOUBAO_VIDEO_TASKS_PATH)?
        .strip_prefix('/')?;
    (!suffix.is_empty() && !suffix.contains('/')).then_some(suffix)
}

pub fn extract_doubao_task_id_from_content_path(path: &str) -> Option<&str> {
    let suffix = path
        .strip_prefix(DOUBAO_VIDEO_TASKS_PATH)?
        .strip_prefix('/')?
        .strip_suffix("/content")?;
    (!suffix.is_empty() && !suffix.contains('/')).then_some(suffix)
}

pub fn resolve_video_task_read_lookup_key<'a>(
    route_family: Option<&str>,
    request_path: &'a str,
) -> Option<VideoTaskLookupKey<'a>> {
    match route_family {
        Some("openai") => {
            extract_openai_task_id_from_path(request_path).map(VideoTaskLookupKey::Id)
        }
        Some("gemini") => {
            extract_gemini_short_id_from_path(request_path).map(VideoTaskLookupKey::ShortId)
        }
        Some("doubao") => {
            extract_doubao_task_id_from_path(request_path).map(VideoTaskLookupKey::Id)
        }
        _ => None,
    }
}

pub fn resolve_video_task_hydration_lookup_key<'a>(
    route_family: Option<&str>,
    request_path: &'a str,
) -> Option<VideoTaskLookupKey<'a>> {
    match route_family {
        Some("openai") => extract_openai_task_id_from_path(request_path)
            .or_else(|| extract_openai_task_id_from_cancel_path(request_path))
            .or_else(|| extract_openai_task_id_from_remix_path(request_path))
            .or_else(|| extract_openai_task_id_from_content_path(request_path))
            .map(VideoTaskLookupKey::Id),
        Some("gemini") => extract_gemini_short_id_from_path(request_path)
            .or_else(|| extract_gemini_short_id_from_cancel_path(request_path))
            .map(VideoTaskLookupKey::ShortId),
        Some("doubao") => extract_doubao_task_id_from_path(request_path)
            .or_else(|| extract_doubao_task_id_from_content_path(request_path))
            .map(VideoTaskLookupKey::Id),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTaskReportLookup<'a> {
    Lookup(VideoTaskLookupKey<'a>),
    TaskIdOrExternal {
        task_id: &'a str,
        user_id: Option<&'a str>,
    },
}

pub fn resolve_video_task_report_lookup<'a>(
    context: &'a Value,
) -> Option<VideoTaskReportLookup<'a>> {
    if let Some(task_id) = non_empty_context_str(context, "local_task_id") {
        return Some(VideoTaskReportLookup::Lookup(VideoTaskLookupKey::Id(
            task_id,
        )));
    }
    if let Some(short_id) = non_empty_context_str(context, "local_short_id") {
        return Some(VideoTaskReportLookup::Lookup(VideoTaskLookupKey::ShortId(
            short_id,
        )));
    }

    let task_id = non_empty_context_str(context, "task_id")?;
    Some(VideoTaskReportLookup::TaskIdOrExternal {
        task_id,
        user_id: non_empty_context_str(context, "user_id"),
    })
}

pub fn build_local_sync_finalize_request_path(
    report_kind: &str,
    signature: &str,
    report_context: Option<&Value>,
) -> Option<String> {
    match report_kind {
        "openai_video_delete_sync_finalize" => {
            let task_id =
                report_context.and_then(|value| non_empty_context_str(value, "task_id"))?;
            Some(format!("/v1/videos/{task_id}"))
        }
        "openai_video_cancel_sync_finalize" => {
            let task_id =
                report_context.and_then(|value| non_empty_context_str(value, "task_id"))?;
            Some(format!("/v1/videos/{task_id}/cancel"))
        }
        "gemini_video_cancel_sync_finalize" => {
            let short_id = report_context
                .and_then(|value| non_empty_context_str(value, "task_id"))
                .or_else(|| {
                    report_context.and_then(|value| non_empty_context_str(value, "local_short_id"))
                })
                .or_else(|| {
                    report_context
                        .and_then(|value| non_empty_context_str(value, "operation_name"))
                        .and_then(|value| value.rsplit('/').next())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })?;
            let model = report_context
                .and_then(|value| non_empty_context_str(value, "model"))
                .or_else(|| {
                    report_context.and_then(|value| non_empty_context_str(value, "model_name"))
                })
                .unwrap_or(match signature {
                    "gemini:video" => "veo-3",
                    _ => "unknown",
                });
            Some(format!(
                "/v1beta/models/{model}/operations/{short_id}:cancel"
            ))
        }
        "doubao_video_delete_sync_finalize" => {
            let task_id =
                report_context.and_then(|value| non_empty_context_str(value, "task_id"))?;
            Some(format!("{DOUBAO_VIDEO_TASKS_PATH}/{task_id}"))
        }
        _ => None,
    }
}

pub fn resolve_local_video_registry_mutation(
    truth_source_mode: VideoTaskTruthSourceMode,
    request_path: &str,
    report_kind: &str,
) -> Option<LocalVideoTaskRegistryMutation> {
    if truth_source_mode != VideoTaskTruthSourceMode::RustAuthoritative {
        return None;
    }

    match report_kind {
        "openai_video_delete_sync_finalize" => {
            let task_id = extract_openai_task_id_from_path(request_path)?;
            Some(LocalVideoTaskRegistryMutation::OpenAiDeleted {
                task_id: task_id.to_string(),
            })
        }
        "openai_video_cancel_sync_finalize" => {
            let task_id = extract_openai_task_id_from_cancel_path(request_path)?;
            Some(LocalVideoTaskRegistryMutation::OpenAiCancelled {
                task_id: task_id.to_string(),
            })
        }
        "gemini_video_cancel_sync_finalize" => {
            let short_id = extract_gemini_short_id_from_cancel_path(request_path)?;
            Some(LocalVideoTaskRegistryMutation::GeminiCancelled {
                short_id: short_id.to_string(),
            })
        }
        // Ark has no separate cancel verb, so DELETE always retires the task.
        "doubao_video_delete_sync_finalize" => {
            let task_id = extract_doubao_task_id_from_path(request_path)?;
            Some(LocalVideoTaskRegistryMutation::DoubaoDeleted {
                task_id: task_id.to_string(),
            })
        }
        _ => None,
    }
}

pub fn current_unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn generate_local_short_id() -> String {
    Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(12)
        .collect()
}

pub fn local_status_from_stored(status: StoredVideoTaskStatus) -> LocalVideoTaskStatus {
    match status {
        StoredVideoTaskStatus::Pending | StoredVideoTaskStatus::Submitted => {
            LocalVideoTaskStatus::Submitted
        }
        StoredVideoTaskStatus::Queued => LocalVideoTaskStatus::Queued,
        StoredVideoTaskStatus::Processing => LocalVideoTaskStatus::Processing,
        StoredVideoTaskStatus::Completed => LocalVideoTaskStatus::Completed,
        StoredVideoTaskStatus::Failed => LocalVideoTaskStatus::Failed,
        StoredVideoTaskStatus::Cancelled => LocalVideoTaskStatus::Cancelled,
        StoredVideoTaskStatus::Expired => LocalVideoTaskStatus::Expired,
        StoredVideoTaskStatus::Deleted => LocalVideoTaskStatus::Deleted,
    }
}

fn non_empty_context_str<'a>(context: &'a Value, key: &str) -> Option<&'a str> {
    context
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        build_local_sync_finalize_request_path, extract_gemini_short_id_from_cancel_path,
        extract_gemini_short_id_from_path, extract_openai_task_id_from_cancel_path,
        extract_openai_task_id_from_content_path, extract_openai_task_id_from_path,
        extract_openai_task_id_from_remix_path, generate_local_short_id,
        resolve_video_task_hydration_lookup_key, resolve_video_task_read_lookup_key,
        resolve_video_task_report_lookup, VideoTaskReportLookup,
    };
    use aether_data_contracts::repository::video_tasks::VideoTaskLookupKey;
    use serde_json::json;

    #[test]
    fn path_extractors_handle_supported_video_routes() {
        assert_eq!(
            extract_openai_task_id_from_path("/v1/videos/task_123"),
            Some("task_123")
        );
        assert_eq!(
            extract_openai_task_id_from_cancel_path("/v1/videos/task_123/cancel"),
            Some("task_123")
        );
        assert_eq!(
            extract_openai_task_id_from_remix_path("/v1/videos/task_123/remix"),
            Some("task_123")
        );
        assert_eq!(
            extract_openai_task_id_from_content_path("/v1/videos/task_123/content"),
            Some("task_123")
        );
        assert_eq!(
            extract_gemini_short_id_from_path("/v1beta/models/foo/operations/abc123"),
            Some("abc123")
        );
        assert_eq!(
            extract_gemini_short_id_from_cancel_path("/v1beta/models/foo/operations/abc123:cancel"),
            Some("abc123")
        );
    }

    #[test]
    fn local_short_id_has_expected_length() {
        assert_eq!(generate_local_short_id().len(), 12);
    }

    #[test]
    fn resolves_video_task_read_lookup_key_for_supported_read_paths() {
        assert_eq!(
            resolve_video_task_read_lookup_key(Some("openai"), "/v1/videos/task_123"),
            Some(VideoTaskLookupKey::Id("task_123"))
        );
        assert_eq!(
            resolve_video_task_read_lookup_key(
                Some("gemini"),
                "/v1beta/models/foo/operations/abc123"
            ),
            Some(VideoTaskLookupKey::ShortId("abc123"))
        );
        assert_eq!(
            resolve_video_task_read_lookup_key(Some("openai"), "/v1/videos/task_123/cancel"),
            None
        );
    }

    #[test]
    fn resolves_video_task_hydration_lookup_key_for_supported_follow_up_paths() {
        assert_eq!(
            resolve_video_task_hydration_lookup_key(Some("openai"), "/v1/videos/task_123/cancel"),
            Some(VideoTaskLookupKey::Id("task_123"))
        );
        assert_eq!(
            resolve_video_task_hydration_lookup_key(Some("openai"), "/v1/videos/task_123/remix"),
            Some(VideoTaskLookupKey::Id("task_123"))
        );
        assert_eq!(
            resolve_video_task_hydration_lookup_key(
                Some("gemini"),
                "/v1beta/models/foo/operations/abc123:cancel"
            ),
            Some(VideoTaskLookupKey::ShortId("abc123"))
        );
    }

    #[test]
    fn resolves_video_task_report_lookup_for_supported_context_shapes() {
        assert_eq!(
            resolve_video_task_report_lookup(&json!({
                "local_task_id": "task-local-123"
            })),
            Some(VideoTaskReportLookup::Lookup(VideoTaskLookupKey::Id(
                "task-local-123"
            )))
        );
        assert_eq!(
            resolve_video_task_report_lookup(&json!({
                "local_short_id": "short-local-123"
            })),
            Some(VideoTaskReportLookup::Lookup(VideoTaskLookupKey::ShortId(
                "short-local-123"
            )))
        );
        assert_eq!(
            resolve_video_task_report_lookup(&json!({
                "task_id": "task-upstream-123",
                "user_id": "user-123"
            })),
            Some(VideoTaskReportLookup::TaskIdOrExternal {
                task_id: "task-upstream-123",
                user_id: Some("user-123"),
            })
        );
    }

    #[test]
    fn builds_local_sync_finalize_request_path_for_supported_video_finalize_kinds() {
        assert_eq!(
            build_local_sync_finalize_request_path(
                "openai_video_delete_sync_finalize",
                "openai:video",
                Some(&json!({"task_id": "task_123"})),
            ),
            Some("/v1/videos/task_123".to_string())
        );
        assert_eq!(
            build_local_sync_finalize_request_path(
                "openai_video_cancel_sync_finalize",
                "openai:video",
                Some(&json!({"task_id": "task_123"})),
            ),
            Some("/v1/videos/task_123/cancel".to_string())
        );
        assert_eq!(
            build_local_sync_finalize_request_path(
                "gemini_video_cancel_sync_finalize",
                "gemini:video",
                Some(&json!({"operation_name": "models/veo-3/operations/abc123"})),
            ),
            Some("/v1beta/models/veo-3/operations/abc123:cancel".to_string())
        );
    }

    #[test]
    fn doubao_path_extractors_reject_nested_and_root_paths() {
        use super::{extract_doubao_task_id_from_content_path, extract_doubao_task_id_from_path};

        assert_eq!(
            extract_doubao_task_id_from_path("/v3/contents/generations/tasks/cgt-123"),
            Some("cgt-123")
        );
        assert_eq!(
            extract_doubao_task_id_from_content_path(
                "/v3/contents/generations/tasks/cgt-123/content"
            ),
            Some("cgt-123")
        );
        // The collection root carries no task id.
        assert_eq!(
            extract_doubao_task_id_from_path("/v3/contents/generations/tasks"),
            None
        );
        // A sub-resource is not a task-level path.
        assert_eq!(
            extract_doubao_task_id_from_path("/v3/contents/generations/tasks/cgt-123/content"),
            None
        );
        assert_eq!(
            extract_doubao_task_id_from_content_path("/v3/contents/generations/tasks/cgt-123"),
            None
        );
        // Other surfaces must not be mistaken for Doubao paths.
        assert_eq!(
            extract_doubao_task_id_from_path("/v1/videos/task_123"),
            None
        );
    }

    #[test]
    fn resolves_doubao_lookup_keys_and_delete_mutation() {
        use super::{
            resolve_local_video_registry_mutation, LocalVideoTaskRegistryMutation,
            VideoTaskTruthSourceMode,
        };

        assert_eq!(
            resolve_video_task_read_lookup_key(
                Some("doubao"),
                "/v3/contents/generations/tasks/cgt-123"
            ),
            Some(VideoTaskLookupKey::Id("cgt-123"))
        );
        assert_eq!(
            resolve_video_task_hydration_lookup_key(
                Some("doubao"),
                "/v3/contents/generations/tasks/cgt-123/content"
            ),
            Some(VideoTaskLookupKey::Id("cgt-123"))
        );
        assert_eq!(
            resolve_local_video_registry_mutation(
                VideoTaskTruthSourceMode::RustAuthoritative,
                "/v3/contents/generations/tasks/cgt-123",
                "doubao_video_delete_sync_finalize",
            ),
            Some(LocalVideoTaskRegistryMutation::DoubaoDeleted {
                task_id: "cgt-123".to_string()
            })
        );
    }

    #[test]
    fn builds_doubao_delete_finalize_request_path() {
        assert_eq!(
            build_local_sync_finalize_request_path(
                "doubao_video_delete_sync_finalize",
                "doubao:video",
                Some(&json!({"task_id": "cgt-123"})),
            ),
            Some("/v3/contents/generations/tasks/cgt-123".to_string())
        );
    }

    #[test]
    fn rejects_local_sync_finalize_request_path_when_context_is_invalid() {
        assert_eq!(
            build_local_sync_finalize_request_path(
                "openai_video_delete_sync_finalize",
                "openai:video",
                Some(&json!({})),
            ),
            None
        );
        assert_eq!(
            build_local_sync_finalize_request_path(
                "gemini_video_cancel_sync_finalize",
                "gemini:video",
                Some(&json!({"task_id": ""})),
            ),
            None
        );
        assert_eq!(
            build_local_sync_finalize_request_path(
                "unknown_finalize_kind",
                "gemini:video",
                Some(&json!({"task_id": "abc123"})),
            ),
            None
        );
    }
}
