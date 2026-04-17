const DEFAULT_API_KEY_PREFIX: &str = "sk";

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

fn generate_gateway_api_key_plaintext_with_prefix(prefix: &str) -> String {
    let first = uuid::Uuid::new_v4().simple().to_string();
    let second = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}{}", first, &second[..16])
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

pub(crate) fn masked_gateway_api_key_display(full_key: Option<&str>) -> String {
    let Some(full_key) = full_key.map(str::trim).filter(|value| !value.is_empty()) else {
        return api_key_placeholder_display();
    };
    let prefix_len = full_key.len().min(10);
    let prefix = &full_key[..prefix_len];
    let suffix = if full_key.len() >= 4 {
        &full_key[full_key.len().saturating_sub(4)..]
    } else {
        ""
    };
    format!("{prefix}...{suffix}")
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
        generate_gateway_api_key_plaintext_with_prefix, masked_gateway_api_key_display,
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
        assert_eq!(value.len(), 3 + 32 + 16);
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
            "ak-1234567...cdef".to_string()
        );
    }
}
