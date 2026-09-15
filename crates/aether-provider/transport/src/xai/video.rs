use serde_json::{json, Value};

/// Native xAI video requests live under /v1; the OpenAI-compatible adapter under /openai/v1.
pub fn is_native_video_request(provider_type: &str, path: &str) -> bool {
    provider_type.trim().eq_ignore_ascii_case("xai")
        && matches!(
            path,
            "/v1/videos" | "/v1/videos/generations" | "/v1/videos/edits" | "/v1/videos/extensions"
        )
}

pub fn is_explicit_native_video_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/videos/generations" | "/v1/videos/edits" | "/v1/videos/extensions"
    )
}

/// Convert the OpenAI video request contract to xAI's native contract.
/// Native requests bypass this adapter so provider-specific fields remain intact.
pub fn convert_openai_video_request(body: &Value) -> Result<Value, &'static str> {
    let prompt = text(&body["prompt"]).ok_or("prompt is required")?;
    let seconds = match &body["seconds"] {
        Value::Null => 4,
        Value::String(value) if value.trim().is_empty() => 4,
        Value::String(value) => value
            .trim()
            .parse::<i64>()
            .map_err(|_| "seconds must be an integer")?,
        value => value.as_i64().ok_or("seconds must be an integer")?,
    }
    .clamp(1, 15);
    let size = text(&body["size"]).unwrap_or("720x1280");
    let default_ratio = match size {
        "720x1280" | "1024x1792" => "9:16",
        "1280x720" | "1792x1024" => "16:9",
        _ => return Err("size must be one of 720x1280, 1280x720, 1024x1792, or 1792x1024"),
    };
    let ratio = match text(&body["aspect_ratio"])
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "square" | "1:1" => "1:1",
        "landscape" | "16:9" => "16:9",
        "portrait" | "9:16" => "9:16",
        "4:3" => "4:3",
        "3:4" => "3:4",
        "3:2" => "3:2",
        "2:3" => "2:3",
        _ => default_ratio,
    };
    let resolution = if text(&body["resolution"]).is_some_and(|v| v.eq_ignore_ascii_case("480p")) {
        "480p"
    } else {
        "720p"
    };
    if text(&body["input_reference"]["file_id"]).is_some() {
        return Err("input_reference.file_id is not supported for xAI video generation; use input_reference.image_url");
    }
    let image = text(&body["input_reference"]["image_url"])
        .or_else(|| image_url(&body["image"]))
        .or_else(|| text(&body["image_url"]));
    let references: Vec<_> = ["reference_images", "reference_image_urls"]
        .into_iter()
        .filter_map(|key| body[key].as_array())
        .flatten()
        .filter_map(image_url)
        .map(|url| json!({"url":url}))
        .collect();
    if references.len() > 7 {
        return Err("reference_images supports at most 7 images on xAI");
    }
    if image.is_some() && !references.is_empty() {
        return Err("image and reference_images cannot be combined on xAI");
    }
    let mut result = json!({"model":body["model"], "prompt":prompt, "duration":seconds, "aspect_ratio":ratio, "resolution":resolution});
    if let Some(url) = image {
        result["image"] = json!({"url":url});
    }
    if !references.is_empty() {
        result["reference_images"] = json!(references);
    }
    Ok(result)
}

fn text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn image_url(value: &Value) -> Option<&str> {
    text(value)
        .or_else(|| text(&value["url"]))
        .or_else(|| text(&value["image_url"]))
        .or_else(|| text(&value["image_url"]["url"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xai_video_compatibility_maps_duration_size_and_references() {
        let converted = convert_openai_video_request(&json!({
            "model":"grok-imagine-video", "prompt":"A cat", "seconds":"8", "size":"1280x720",
            "reference_images":[{"image_url":{"url":"https://example.com/a.png"}}],
            "reference_image_urls":["https://example.com/b.png"]
        }))
        .unwrap();
        assert_eq!(
            converted,
            json!({"model":"grok-imagine-video", "prompt":"A cat", "duration":8,
            "aspect_ratio":"16:9", "resolution":"720p", "reference_images":[{"url":"https://example.com/a.png"},{"url":"https://example.com/b.png"}]})
        );
        let defaults = convert_openai_video_request(&json!({"prompt":"A cat"})).unwrap();
        assert_eq!(defaults["duration"], 4);
        assert_eq!(defaults["aspect_ratio"], "9:16");
        for (seconds, expected) in [(-1, 1), (30, 15)] {
            assert_eq!(
                convert_openai_video_request(&json!({"prompt":"A cat", "seconds":seconds}))
                    .unwrap()["duration"],
                expected
            );
        }
    }

    #[test]
    fn xai_video_compatibility_validates_requests_and_maps_image_input() {
        for invalid in [
            json!({}),
            json!({"prompt":"cat","seconds":"1.5"}),
            json!({"prompt":"cat","size":"foo"}),
            json!({"prompt":"cat","input_reference":{"file_id":"file-1"}}),
            json!({"prompt":"cat","image":"https://example.com/a.png","reference_images":["https://example.com/b.png"]}),
            json!({"prompt":"cat","reference_images":vec!["https://example.com/a.png";8]}),
        ] {
            assert!(convert_openai_video_request(&invalid).is_err(), "{invalid}");
        }
        let body = convert_openai_video_request(&json!({"prompt":"cat","input_reference":{"image_url":"https://example.com/a.png"},"aspect_ratio":"square","resolution":"480p"})).unwrap();
        assert_eq!(body["image"]["url"], "https://example.com/a.png");
        assert_eq!(body["aspect_ratio"], "1:1");
        assert_eq!(body["resolution"], "480p");
    }
}
