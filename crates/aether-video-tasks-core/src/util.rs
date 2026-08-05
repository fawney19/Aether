pub fn non_empty_owned(value: Option<&String>) -> Option<String> {
    value
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Maximum width of `video_tasks.short_id`.
const SHORT_ID_MAX_LEN: usize = 16;

/// Derives the stored `short_id` for surfaces that have no short-id concept.
///
/// `video_tasks.short_id` is `NOT NULL`, but only the Gemini surface exposes a
/// short operation id to clients. OpenAI and Doubao task ids are longer than the
/// column allows, so a deterministic prefix is derived instead: the same task
/// always yields the same value, which keeps repeated upserts idempotent.
pub fn derive_video_task_short_id(local_task_id: &str) -> String {
    let compact = local_task_id
        .chars()
        .filter(|value| value.is_ascii_alphanumeric())
        .take(SHORT_ID_MAX_LEN)
        .collect::<String>();
    if compact.is_empty() {
        // Never leave the column empty, even for an unexpected id shape.
        return "video".to_string();
    }
    compact
}

#[cfg(test)]
mod tests {
    use super::{derive_video_task_short_id, non_empty_owned};

    #[test]
    fn discards_blank_strings() {
        let empty = String::from("   ");
        let value = String::from(" hello ");

        assert_eq!(non_empty_owned(Some(&empty)), None);
        assert_eq!(non_empty_owned(Some(&value)).as_deref(), Some("hello"));
    }

    #[test]
    fn derives_a_column_sized_short_id() {
        // A Doubao local id: `cgt-` plus a 32-char uuid.
        let short_id = derive_video_task_short_id("cgt-0f9a1b2c3d4e5f60718293a4b5c6d7e8");
        assert_eq!(short_id.len(), 16);
        assert_eq!(short_id, "cgt0f9a1b2c3d4e5");

        // A bare uuid, as used by the OpenAI surface.
        let openai = derive_video_task_short_id("0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8");
        assert_eq!(openai.len(), 16);
    }

    #[test]
    fn derivation_is_stable_for_the_same_task() {
        let first = derive_video_task_short_id("cgt-abc123");
        let second = derive_video_task_short_id("cgt-abc123");

        assert_eq!(first, second);
    }

    #[test]
    fn short_ids_stay_non_empty_for_unexpected_shapes() {
        assert_eq!(derive_video_task_short_id("---"), "video");
        assert_eq!(derive_video_task_short_id(""), "video");
    }
}
