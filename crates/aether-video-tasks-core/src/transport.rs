use serde_json::Value;

use crate::LocalVideoTaskStatus;

pub fn parse_video_content_variant(query_string: Option<&str>) -> Option<&'static str> {
    let mut variant = "video";
    if let Some(query_string) = query_string {
        for (key, value) in url::form_urlencoded::parse(query_string.as_bytes()) {
            if key == "variant" {
                variant = match value.as_ref() {
                    "video" => "video",
                    "thumbnail" => "thumbnail",
                    "spritesheet" => "spritesheet",
                    _ => return None,
                };
            }
        }
    }
    Some(variant)
}

/// Doubao only produces the rendered video plus an optional last frame image.
pub fn parse_doubao_video_content_variant(query_string: Option<&str>) -> Option<&'static str> {
    let mut variant = "video";
    if let Some(query_string) = query_string {
        for (key, value) in url::form_urlencoded::parse(query_string.as_bytes()) {
            if key == "variant" {
                variant = match value.as_ref() {
                    "video" => "video",
                    "last_frame" => "last_frame",
                    _ => return None,
                };
            }
        }
    }
    Some(variant)
}

/// Builds an Ark content generation task URL by appending the task resource
/// path to the configured base URL.
///
/// The base URL is used exactly as configured — nothing is inferred or
/// repaired. The appended path is the same one clients call, so the upstream
/// URL is always `<base>/v3/contents/generations/tasks[/<id>]`.
pub fn doubao_video_tasks_url(upstream_base_url: &str, task_id: Option<&str>) -> String {
    let base = upstream_base_url.trim().trim_end_matches('/');
    let (base, query) = base
        .split_once('?')
        .map(|(base, query)| (base.trim_end_matches('/'), Some(query)))
        .unwrap_or((base, None));

    let mut url = format!("{base}{}", crate::DOUBAO_VIDEO_TASKS_PATH);
    if let Some(task_id) = task_id.map(str::trim).filter(|value| !value.is_empty()) {
        url.push('/');
        url.push_str(task_id);
    }
    if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

pub fn gemini_metadata_video_url(metadata: &Value) -> Option<String> {
    metadata
        .get("response")
        .and_then(|value| value.get("generateVideoResponse"))
        .and_then(|value| value.get("generatedSamples"))
        .and_then(Value::as_array)
        .and_then(|value| value.first())
        .and_then(|value| value.get("video"))
        .and_then(|value| value.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn map_openai_task_status(status: LocalVideoTaskStatus) -> &'static str {
    match status {
        LocalVideoTaskStatus::Submitted | LocalVideoTaskStatus::Queued => "queued",
        LocalVideoTaskStatus::Processing => "processing",
        LocalVideoTaskStatus::Completed => "completed",
        LocalVideoTaskStatus::Failed
        | LocalVideoTaskStatus::Cancelled
        | LocalVideoTaskStatus::Expired => "failed",
        LocalVideoTaskStatus::Deleted => "deleted",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::LocalVideoTaskStatus;

    use super::{gemini_metadata_video_url, map_openai_task_status, parse_video_content_variant};

    #[test]
    fn builds_doubao_task_urls_by_appending_to_the_configured_base() {
        use super::doubao_video_tasks_url;

        // Ark's API root is the provider base; the version segment belongs to
        // the resource path, matching the client-facing route.
        assert_eq!(
            doubao_video_tasks_url("https://ark.cn-beijing.volces.com/api", None),
            "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks"
        );
        assert_eq!(
            doubao_video_tasks_url("https://ark.cn-beijing.volces.com/api/", Some("cgt-1")),
            "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks/cgt-1"
        );
        // Any base is honored verbatim, including proxy roots.
        assert_eq!(
            doubao_video_tasks_url("https://proxy.example.com/ark", Some("cgt-1")),
            "https://proxy.example.com/ark/v3/contents/generations/tasks/cgt-1"
        );
        // A misconfigured base is not repaired; the resulting URL reflects it
        // exactly so the operator sees their own configuration in the error.
        assert_eq!(
            doubao_video_tasks_url("https://ark.cn-beijing.volces.com/api/v3", None),
            "https://ark.cn-beijing.volces.com/api/v3/v3/contents/generations/tasks"
        );
        // Configured base query parameters survive path composition.
        assert_eq!(
            doubao_video_tasks_url("https://proxy.example.com/api?tenant=demo", Some("cgt-1")),
            "https://proxy.example.com/api/v3/contents/generations/tasks/cgt-1?tenant=demo"
        );
    }

    #[test]
    fn parses_supported_doubao_content_variants() {
        use super::parse_doubao_video_content_variant;

        assert_eq!(parse_doubao_video_content_variant(None), Some("video"));
        assert_eq!(
            parse_doubao_video_content_variant(Some("variant=last_frame")),
            Some("last_frame")
        );
        // Sora-only variants have no Doubao equivalent.
        assert_eq!(
            parse_doubao_video_content_variant(Some("variant=spritesheet")),
            None
        );
    }

    #[test]
    fn parses_supported_video_content_variants() {
        assert_eq!(parse_video_content_variant(None), Some("video"));
        assert_eq!(
            parse_video_content_variant(Some("variant=thumbnail")),
            Some("thumbnail")
        );
        assert_eq!(
            parse_video_content_variant(Some("variant=spritesheet")),
            Some("spritesheet")
        );
        assert_eq!(parse_video_content_variant(Some("variant=invalid")), None);
    }

    #[test]
    fn extracts_gemini_metadata_video_url() {
        let metadata = json!({
            "response": {
                "generateVideoResponse": {
                    "generatedSamples": [
                        {
                            "video": {
                                "uri": "https://example.com/video.mp4"
                            }
                        }
                    ]
                }
            }
        });

        assert_eq!(
            gemini_metadata_video_url(&metadata).as_deref(),
            Some("https://example.com/video.mp4")
        );
    }

    #[test]
    fn maps_openai_task_status() {
        assert_eq!(
            map_openai_task_status(LocalVideoTaskStatus::Queued),
            "queued"
        );
        assert_eq!(
            map_openai_task_status(LocalVideoTaskStatus::Completed),
            "completed"
        );
        assert_eq!(
            map_openai_task_status(LocalVideoTaskStatus::Failed),
            "failed"
        );
    }
}
