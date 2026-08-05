//! One-way request conversion from the OpenAI video surface to Doubao (Ark).
//!
//! Sora's request contract is a subset of Ark's, so a Sora client can be routed
//! to a Doubao provider. The reverse is deliberately not implemented: Ark
//! requests carry reference video/audio, multiple images and `generate_audio`,
//! none of which Sora can express, so the conversion would fail on most real
//! requests instead of only on exotic ones.

use serde_json::{json, Map, Value};

use crate::formats::context::FormatError;

const SOURCE_FORMAT: &str = "openai:video";
const TARGET_FORMAT: &str = "doubao:video";

/// Converts a Sora `POST /v1/videos` body into an Ark task creation body.
///
/// The model field is left untouched; candidate model mapping owns it.
pub fn convert_openai_video_request_to_doubao(body: &Value) -> Result<Value, FormatError> {
    let Some(source) = body.as_object() else {
        return Err(FormatError::RequestParseFailed {
            format: SOURCE_FORMAT.to_string(),
        });
    };

    for field in ["remix_video_id", "remixed_from_video_id"] {
        if source.contains_key(field) {
            return Err(lossy(
                field,
                "Doubao has no remix surface for an existing task",
            ));
        }
    }

    let prompt = source
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FormatError::InvalidTargetField {
            format: TARGET_FORMAT.to_string(),
            field: "prompt".to_string(),
            reason: "Doubao requires a non-empty text prompt".to_string(),
        })?;

    let mut content = vec![json!({"type": "text", "text": prompt})];
    if let Some(reference) = source.get("input_reference") {
        content.push(convert_input_reference(reference)?);
    }

    let mut target = Map::new();
    if let Some(model) = source.get("model") {
        target.insert("model".to_string(), model.clone());
    }
    target.insert("content".to_string(), Value::Array(content));

    if let Some(size) = source.get("size") {
        let size = size.as_str().map(str::trim).unwrap_or_default();
        let (ratio, resolution) = convert_size(size)?;
        target.insert("ratio".to_string(), Value::String(ratio.to_string()));
        if let Some(resolution) = resolution {
            target.insert(
                "resolution".to_string(),
                Value::String(resolution.to_string()),
            );
        }
    }

    if let Some(seconds) = source.get("seconds") {
        target.insert("duration".to_string(), convert_seconds(seconds)?);
    }

    Ok(Value::Object(target))
}

/// Maps Sora's pixel `size` onto Ark's aspect ratio plus optional resolution tier.
///
/// Ark treats `resolution` as optional and keys generation off `ratio`, so sizes
/// that only imply an aspect ratio omit the tier rather than inventing one.
fn convert_size(size: &str) -> Result<(&'static str, Option<&'static str>), FormatError> {
    match size {
        "1280x720" => Ok(("16:9", Some("720p"))),
        "720x1280" => Ok(("9:16", Some("720p"))),
        "1920x1080" => Ok(("16:9", Some("1080p"))),
        "1080x1920" => Ok(("9:16", Some("1080p"))),
        "854x480" => Ok(("16:9", Some("480p"))),
        "480x854" => Ok(("9:16", Some("480p"))),
        "1024x1024" => Ok(("1:1", None)),
        _ => Err(lossy(
            "size",
            "Doubao has no aspect ratio and resolution tier matching this pixel size",
        )),
    }
}

fn convert_seconds(seconds: &Value) -> Result<Value, FormatError> {
    let parsed = match seconds {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    parsed
        .map(Value::from)
        .ok_or_else(|| FormatError::InvalidTargetField {
            format: TARGET_FORMAT.to_string(),
            field: "seconds".to_string(),
            reason: "Doubao duration must be a positive whole number of seconds".to_string(),
        })
}

/// Converts Sora's single `input_reference` image into an Ark content entry.
fn convert_input_reference(reference: &Value) -> Result<Value, FormatError> {
    let url = match reference {
        Value::String(url) => Some(url.trim().to_string()),
        Value::Object(object) => object
            .get("url")
            .or_else(|| {
                object
                    .get("image_url")
                    .and_then(Value::as_object)
                    .and_then(|image_url| image_url.get("url"))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .map(ToOwned::to_owned),
        _ => None,
    }
    .filter(|url| !url.is_empty())
    .ok_or_else(|| {
        lossy(
            "input_reference",
            "Doubao reference images must be addressable by URL",
        )
    })?;

    Ok(json!({
        "type": "image_url",
        "image_url": {"url": url},
        "role": "first_frame",
    }))
}

fn lossy(field: &str, reason: &str) -> FormatError {
    FormatError::LossyConversionBlocked {
        source_format: SOURCE_FORMAT.to_string(),
        target_format: TARGET_FORMAT.to_string(),
        field: field.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::convert_openai_video_request_to_doubao;
    use crate::formats::context::FormatError;

    #[test]
    fn converts_text_to_video_request() {
        let converted = convert_openai_video_request_to_doubao(&json!({
            "model": "sora-2",
            "prompt": "a cat yawning",
            "size": "1280x720",
            "seconds": "8"
        }))
        .expect("conversion should succeed");

        assert_eq!(converted["model"], "sora-2");
        assert_eq!(converted["content"][0]["type"], "text");
        assert_eq!(converted["content"][0]["text"], "a cat yawning");
        assert_eq!(converted["ratio"], "16:9");
        assert_eq!(converted["resolution"], "720p");
        assert_eq!(converted["duration"], 8);
    }

    #[test]
    fn converts_image_to_video_request_with_first_frame_role() {
        let converted = convert_openai_video_request_to_doubao(&json!({
            "prompt": "pan across the scene",
            "input_reference": {"url": "https://example.com/a.jpg"}
        }))
        .expect("conversion should succeed");

        assert_eq!(converted["content"][1]["type"], "image_url");
        assert_eq!(
            converted["content"][1]["image_url"]["url"],
            "https://example.com/a.jpg"
        );
        assert_eq!(converted["content"][1]["role"], "first_frame");
    }

    #[test]
    fn square_size_maps_to_ratio_without_resolution_tier() {
        let converted = convert_openai_video_request_to_doubao(&json!({
            "prompt": "a clip",
            "size": "1024x1024"
        }))
        .expect("conversion should succeed");

        assert_eq!(converted["ratio"], "1:1");
        assert!(converted.get("resolution").is_none());
    }

    #[test]
    fn unmapped_size_is_blocked_rather_than_silently_dropped() {
        let error = convert_openai_video_request_to_doubao(&json!({
            "prompt": "a clip",
            "size": "1337x42"
        }))
        .expect_err("unmapped size should be rejected");

        assert!(matches!(
            error,
            FormatError::LossyConversionBlocked { ref field, .. } if field == "size"
        ));
    }

    #[test]
    fn remix_requests_are_blocked() {
        let error = convert_openai_video_request_to_doubao(&json!({
            "prompt": "a clip",
            "remix_video_id": "video_123"
        }))
        .expect_err("remix should be rejected");

        assert!(matches!(
            error,
            FormatError::LossyConversionBlocked { ref field, .. } if field == "remix_video_id"
        ));
    }

    #[test]
    fn missing_prompt_is_rejected() {
        let error = convert_openai_video_request_to_doubao(&json!({"size": "1280x720"}))
            .expect_err("missing prompt should be rejected");

        assert!(matches!(
            error,
            FormatError::InvalidTargetField { ref field, .. } if field == "prompt"
        ));
    }

    #[test]
    fn non_numeric_seconds_is_rejected() {
        let error = convert_openai_video_request_to_doubao(&json!({
            "prompt": "a clip",
            "seconds": "eight"
        }))
        .expect_err("non-numeric seconds should be rejected");

        assert!(matches!(
            error,
            FormatError::InvalidTargetField { ref field, .. } if field == "seconds"
        ));
    }
}
