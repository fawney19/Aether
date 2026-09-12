use serde_json::{json, Value};

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

pub fn gemini_metadata_video_url(metadata: &Value) -> Option<String> {
    metadata.get("response").and_then(gemini_response_video_url)
}

pub(crate) fn gemini_response_video_url(response: &Value) -> Option<String> {
    response
        .get("generateVideoResponse")
        .and_then(|value| value.get("generatedSamples"))
        .and_then(Value::as_array)
        .and_then(|value| value.first())
        .and_then(|value| value.get("video"))
        .and_then(|value| value.get("uri"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn gemini_video_metadata(video_url: Option<&str>) -> Value {
    match video_url {
        Some(video_url) => json!({
            "response": {
                "generateVideoResponse": {
                    "generatedSamples": [{"video": {"uri": video_url}}]
                }
            }
        }),
        None => json!({}),
    }
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
