#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiFamily {
    OpenAi,
    Claude,
    Gemini,
    Unknown,
}

fn parse_api_family(api_format: Option<&str>) -> ApiFamily {
    let Some(api_format) = api_format else {
        return ApiFamily::Unknown;
    };
    let family = api_format
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match family.as_str() {
        "openai" => ApiFamily::OpenAi,
        "claude" | "anthropic" => ApiFamily::Claude,
        "gemini" | "google" => ApiFamily::Gemini,
        _ => ApiFamily::Unknown,
    }
}

pub fn normalize_input_tokens_for_billing(
    api_format: Option<&str>,
    input_tokens: i64,
    cache_read_tokens: i64,
) -> i64 {
    if input_tokens <= 0 {
        return input_tokens.max(0);
    }
    if cache_read_tokens <= 0 {
        return input_tokens;
    }

    match parse_api_family(api_format) {
        ApiFamily::Claude => input_tokens,
        ApiFamily::OpenAi | ApiFamily::Gemini => (input_tokens - cache_read_tokens).max(0),
        ApiFamily::Unknown => input_tokens,
    }
}

pub fn normalize_total_input_context_for_cache_hit_rate(
    api_format: Option<&str>,
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> i64 {
    let normalized_input_tokens = input_tokens.max(0);
    let normalized_cache_creation_tokens = cache_creation_tokens.max(0);
    let normalized_cache_read_tokens = cache_read_tokens.max(0);

    let fresh_input_tokens = match parse_api_family(api_format) {
        ApiFamily::Claude => {
            normalized_input_tokens.saturating_add(normalized_cache_creation_tokens)
        }
        ApiFamily::OpenAi | ApiFamily::Gemini => normalize_input_tokens_for_billing(
            api_format,
            normalized_input_tokens,
            normalized_cache_read_tokens,
        ),
        ApiFamily::Unknown => {
            if normalized_cache_creation_tokens > 0 {
                normalized_input_tokens.saturating_add(normalized_cache_creation_tokens)
            } else {
                normalized_input_tokens
            }
        }
    };

    fresh_input_tokens.saturating_add(normalized_cache_read_tokens)
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_input_tokens_for_billing, normalize_total_input_context_for_cache_hit_rate,
    };

    #[test]
    fn subtracts_cache_tokens_for_openai_and_gemini() {
        assert_eq!(
            normalize_input_tokens_for_billing(Some("openai:chat"), 100, 20),
            80
        );
        assert_eq!(
            normalize_input_tokens_for_billing(Some("gemini:chat"), 100, 20),
            80
        );
    }

    #[test]
    fn keeps_input_tokens_for_claude() {
        assert_eq!(
            normalize_input_tokens_for_billing(Some("claude:chat"), 100, 20),
            100
        );
    }

    #[test]
    fn normalizes_cache_hit_context_for_openai_and_gemini() {
        assert_eq!(
            normalize_total_input_context_for_cache_hit_rate(Some("openai:chat"), 120, 10, 15),
            120
        );
        assert_eq!(
            normalize_total_input_context_for_cache_hit_rate(Some("gemini:chat"), 120, 10, 15),
            120
        );
    }

    #[test]
    fn includes_cache_creation_for_claude_cache_hit_context() {
        assert_eq!(
            normalize_total_input_context_for_cache_hit_rate(Some("claude:chat"), 60, 15, 5),
            80
        );
    }

    #[test]
    fn falls_back_to_creation_aware_context_for_unknown_formats() {
        assert_eq!(
            normalize_total_input_context_for_cache_hit_rate(None, 20, 10, 5),
            35
        );
        assert_eq!(
            normalize_total_input_context_for_cache_hit_rate(None, 20, 0, 5),
            25
        );
    }
}
