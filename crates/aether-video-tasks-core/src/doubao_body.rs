//! Doubao (Volcengine Ark) request/response body helpers.
//!
//! Ark accepts generation parameters as top-level JSON fields (`ratio`,
//! `duration`, `resolution`, ...). Older prompt-suffix syntax (`--rt 16:9`)
//! remains valid, so extraction falls back to parsing the text content when a
//! top-level field is absent. Only fields the gateway needs for storage,
//! display and billing are extracted; the rest of the body passes through
//! untouched.

use serde_json::Value;

/// Returns the first `text` entry of an Ark `content` array.
pub fn doubao_content_prompt(body: &Value) -> Option<String> {
    body.as_object()?
        .get("content")?
        .as_array()?
        .iter()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Reports whether the request supplies a reference video.
///
/// Video-to-video generation is materially more expensive upstream than text or
/// image driven generation, so billing prices it separately. Only `video_url`
/// entries count: image and audio references stay on the default price table.
pub fn doubao_content_has_video_input(body: &Value) -> bool {
    body.as_object()
        .and_then(|object| object.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_some_and(|value| value.eq_ignore_ascii_case("video_url"))
            })
        })
}

/// Reads a top-level string field, falling back to a `--flag value` prompt suffix.
pub fn doubao_string_parameter(body: &Value, field: &str, flags: &[&str]) -> Option<String> {
    if let Some(value) = body
        .as_object()
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_string());
    }
    let prompt = doubao_content_prompt(body)?;
    prompt_suffix_value(&prompt, flags)
}

/// Reads a top-level integer field, falling back to a `--flag value` prompt suffix.
pub fn doubao_u32_parameter(body: &Value, field: &str, flags: &[&str]) -> Option<u32> {
    if let Some(value) = body.as_object().and_then(|object| object.get(field)) {
        match value {
            Value::Number(number) => {
                // Ark uses -1 for "let the model choose"; that is not a duration.
                if let Some(value) = number.as_u64() {
                    return u32::try_from(value).ok();
                }
                return None;
            }
            Value::String(text) => {
                if let Ok(parsed) = text.trim().parse::<u32>() {
                    return Some(parsed);
                }
            }
            _ => {}
        }
    }
    let prompt = doubao_content_prompt(body)?;
    prompt_suffix_value(&prompt, flags)?.parse().ok()
}

/// Extracts the value following any of `flags` in a `--flag value` prompt suffix.
fn prompt_suffix_value(prompt: &str, flags: &[&str]) -> Option<String> {
    let mut tokens = prompt.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        let Some(name) = token.strip_prefix("--") else {
            continue;
        };
        if !flags.iter().any(|flag| flag.eq_ignore_ascii_case(name)) {
            continue;
        }
        let value = tokens.peek()?;
        if value.starts_with("--") {
            return None;
        }
        return Some((*value).to_string());
    }
    None
}

/// Strips `--flag value` suffixes so the stored prompt keeps only the instruction text.
pub fn doubao_prompt_text(prompt: &str) -> String {
    let mut words = Vec::new();
    let mut tokens = prompt.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        if token.starts_with("--") {
            // Consume the flag's value when present.
            if tokens.peek().is_some_and(|next| !next.starts_with("--")) {
                tokens.next();
            }
            continue;
        }
        words.push(token);
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        doubao_content_has_video_input, doubao_content_prompt, doubao_prompt_text,
        doubao_string_parameter, doubao_u32_parameter,
    };

    #[test]
    fn detects_a_reference_video_input() {
        let with_video = json!({
            "content": [
                {"type": "text", "text": "clip"},
                {"type": "image_url", "image_url": {"url": "https://e/a.jpg"}, "role": "reference_image"},
                {"type": "video_url", "video_url": {"url": "https://e/a.mp4"}, "role": "reference_video"}
            ]
        });
        assert!(doubao_content_has_video_input(&with_video));

        // Image and audio references are not video-to-video generation.
        let image_only = json!({
            "content": [
                {"type": "text", "text": "clip"},
                {"type": "image_url", "image_url": {"url": "https://e/a.jpg"}},
                {"type": "audio_url", "audio_url": {"url": "https://e/a.mp3"}}
            ]
        });
        assert!(!doubao_content_has_video_input(&image_only));

        assert!(!doubao_content_has_video_input(
            &json!({"content": [{"type": "text", "text": "clip"}]})
        ));
        // Other surfaces have no `content` array at all.
        assert!(!doubao_content_has_video_input(&json!({"prompt": "clip"})));
        assert!(!doubao_content_has_video_input(&json!({})));
    }

    fn top_level_body() -> serde_json::Value {
        json!({
            "model": "doubao-seedance-2-0-260128",
            "content": [
                {"type": "text", "text": "a cat yawning"},
                {"type": "image_url", "image_url": {"url": "https://example.com/a.jpg"}, "role": "reference_image"}
            ],
            "ratio": "16:9",
            "duration": 11,
            "generate_audio": true
        })
    }

    #[test]
    fn reads_top_level_parameters() {
        let body = top_level_body();
        assert_eq!(
            doubao_content_prompt(&body).as_deref(),
            Some("a cat yawning")
        );
        assert_eq!(
            doubao_string_parameter(&body, "ratio", &["rt", "ratio"]).as_deref(),
            Some("16:9")
        );
        assert_eq!(
            doubao_u32_parameter(&body, "duration", &["dur", "duration"]),
            Some(11)
        );
        // Absent optional field stays absent rather than guessing a default.
        assert_eq!(
            doubao_string_parameter(&body, "resolution", &["rs", "resolution"]),
            None
        );
    }

    #[test]
    fn falls_back_to_prompt_suffix_parameters() {
        let body = json!({
            "content": [
                {"type": "text", "text": "a cat yawning --rs 720p --rt 9:16 --dur 5"}
            ]
        });

        assert_eq!(
            doubao_string_parameter(&body, "resolution", &["rs", "resolution"]).as_deref(),
            Some("720p")
        );
        assert_eq!(
            doubao_string_parameter(&body, "ratio", &["rt", "ratio"]).as_deref(),
            Some("9:16")
        );
        assert_eq!(
            doubao_u32_parameter(&body, "duration", &["dur", "duration"]),
            Some(5)
        );
    }

    #[test]
    fn top_level_field_wins_over_prompt_suffix() {
        let body = json!({
            "content": [{"type": "text", "text": "clip --rt 1:1"}],
            "ratio": "16:9"
        });

        assert_eq!(
            doubao_string_parameter(&body, "ratio", &["rt", "ratio"]).as_deref(),
            Some("16:9")
        );
    }

    #[test]
    fn negative_duration_is_not_a_resolved_value() {
        let body = json!({
            "content": [{"type": "text", "text": "clip"}],
            "duration": -1
        });

        assert_eq!(
            doubao_u32_parameter(&body, "duration", &["dur", "duration"]),
            None
        );
    }

    #[test]
    fn flag_without_value_does_not_consume_the_next_flag() {
        let body = json!({
            "content": [{"type": "text", "text": "clip --rs --dur 5"}]
        });

        assert_eq!(
            doubao_string_parameter(&body, "resolution", &["rs", "resolution"]),
            None
        );
        assert_eq!(
            doubao_u32_parameter(&body, "duration", &["dur", "duration"]),
            Some(5)
        );
    }

    #[test]
    fn prompt_text_strips_parameter_suffixes() {
        assert_eq!(
            doubao_prompt_text("a cat yawning --rs 720p --dur 5"),
            "a cat yawning"
        );
        assert_eq!(doubao_prompt_text("a cat yawning"), "a cat yawning");
    }

    #[test]
    fn prompt_lookup_ignores_non_text_content_entries() {
        let body = json!({
            "content": [
                {"type": "image_url", "image_url": {"url": "https://example.com/a.jpg"}},
                {"type": "text", "text": "second entry wins only when first is not text"}
            ]
        });

        assert_eq!(
            doubao_content_prompt(&body).as_deref(),
            Some("second entry wins only when first is not text")
        );
    }
}
