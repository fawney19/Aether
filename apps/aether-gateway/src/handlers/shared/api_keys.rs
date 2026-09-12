const DEFAULT_API_KEY_PREFIX: &str = "sk";
const API_KEY_RANDOM_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const API_KEY_RANDOM_LEN: usize = 32;

fn configured_api_key_prefix_from_lookup<F>(lookup: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    lookup("API_KEY_PREFIX")
        .as_deref()
        .map(normalize_api_key_prefix)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_API_KEY_PREFIX.to_string())
}

fn normalize_api_key_prefix(value: &str) -> String {
    let normalized = value.trim().trim_end_matches('-').trim();
    if normalized.is_empty() {
        return DEFAULT_API_KEY_PREFIX.to_string();
    }
    normalized.to_string()
}

fn api_key_placeholder_display_with_prefix(prefix: &str) -> String {
    format!("{prefix}-****")
}

fn generate_gateway_secret_random_part() -> String {
    let mut random = String::with_capacity(API_KEY_RANDOM_LEN);
    while random.len() < API_KEY_RANDOM_LEN {
        for byte in uuid::Uuid::new_v4().as_bytes() {
            let index = usize::from(*byte) % API_KEY_RANDOM_ALPHABET.len();
            random.push(char::from(API_KEY_RANDOM_ALPHABET[index]));
            if random.len() == API_KEY_RANDOM_LEN {
                break;
            }
        }
    }
    random
}

pub(crate) fn generate_gateway_secret_plaintext(prefix: &str, separator: &str) -> String {
    format!(
        "{prefix}{separator}{}",
        generate_gateway_secret_random_part()
    )
}

fn generate_gateway_api_key_plaintext_with_prefix(prefix: &str) -> String {
    generate_gateway_secret_plaintext(prefix, "-")
}

pub(crate) fn configured_api_key_prefix() -> String {
    configured_api_key_prefix_from_lookup(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

pub(crate) fn api_key_placeholder_display() -> String {
    api_key_placeholder_display_with_prefix(&configured_api_key_prefix())
}

pub(crate) fn generate_gateway_api_key_plaintext() -> String {
    generate_gateway_api_key_plaintext_with_prefix(&configured_api_key_prefix())
}

pub(crate) fn masked_secret_display(
    value: &str,
    preferred_prefix_chars: usize,
    preferred_suffix_chars: usize,
    separator: &str,
) -> String {
    let char_count = value.chars().count();
    if char_count <= 4 {
        return "***".to_string();
    }

    // Never reveal more than half of a short secret. For normal generated keys,
    // the preferred prefix/suffix remains stable while a meaningful middle
    // section is always hidden.
    let visible_budget =
        (char_count / 2).min(preferred_prefix_chars.saturating_add(preferred_suffix_chars));
    if visible_budget == 0 {
        return "***".to_string();
    }
    let suffix_chars = preferred_suffix_chars.min(visible_budget / 2);
    let prefix_chars = preferred_prefix_chars.min(visible_budget.saturating_sub(suffix_chars));
    if prefix_chars == 0 && suffix_chars == 0 {
        return "***".to_string();
    }

    let prefix = value.chars().take(prefix_chars).collect::<String>();
    let suffix = value
        .chars()
        .skip(char_count.saturating_sub(suffix_chars))
        .collect::<String>();
    format!("{prefix}{separator}{suffix}")
}

pub(crate) fn masked_gateway_api_key_display(full_key: Option<&str>) -> String {
    let Some(full_key) = full_key.map(str::trim).filter(|value| !value.is_empty()) else {
        return api_key_placeholder_display();
    };
    masked_secret_display(full_key, 10, 4, "...")
}

pub(crate) fn normalize_optional_api_key_concurrent_limit(
    value: Option<i32>,
) -> Result<Option<i32>, String> {
    if value.is_some_and(|limit| limit < 0) {
        return Err("concurrent_limit 必须是非负整数".to_string());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        api_key_placeholder_display_with_prefix, configured_api_key_prefix_from_lookup,
        generate_gateway_api_key_plaintext_with_prefix, generate_gateway_secret_plaintext,
        masked_gateway_api_key_display, masked_secret_display,
    };

    #[test]
    fn defaults_api_key_prefix_to_sk() {
        assert_eq!(
            configured_api_key_prefix_from_lookup(|_| None),
            "sk".to_string()
        );
    }

    #[test]
    fn normalizes_api_key_prefix_whitespace_and_trailing_dash() {
        assert_eq!(
            configured_api_key_prefix_from_lookup(|_| Some("  ak-  ".to_string())),
            "ak".to_string()
        );
    }

    #[test]
    fn generates_plaintext_api_key_with_configured_prefix() {
        let value = generate_gateway_api_key_plaintext_with_prefix("ak");
        assert!(value.starts_with("ak-"));
        assert_eq!(value.len(), 3 + 32);
        assert!(value
            .trim_start_matches("ak-")
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric()));
    }

    #[test]
    fn generates_plaintext_secret_with_custom_separator() {
        let value = generate_gateway_secret_plaintext("ae", "-");
        assert!(value.starts_with("ae-"));
        assert_eq!(value.len(), 3 + 32);
        assert!(value
            .trim_start_matches("ae-")
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric()));
    }

    #[test]
    fn uses_configured_prefix_in_placeholder_display() {
        assert_eq!(
            api_key_placeholder_display_with_prefix("ak"),
            "ak-****".to_string()
        );
    }

    #[test]
    fn masks_plaintext_api_key_without_changing_prefix() {
        assert_eq!(
            masked_gateway_api_key_display(Some("ak-1234567890abcdef")),
            "ak-12...cdef".to_string()
        );
    }

    #[test]
    fn masking_never_discloses_an_entire_short_or_unicode_secret() {
        for secret in ["abc", "sk-short", "测试密钥一二三"] {
            let masked = masked_secret_display(secret, 10, 4, "...");
            assert_ne!(masked, secret);
            assert!(!masked.contains(secret));
            assert!(masked.contains('*') || masked.contains("..."));
        }
    }
}
