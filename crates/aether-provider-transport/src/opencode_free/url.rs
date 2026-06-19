pub const OPENCODE_FREE_DEFAULT_BASE_PATH: &str = "/v1";

pub fn build_opencode_free_url(base_url: &str, custom_path: Option<&str>) -> Option<String> {
    let trimmed_base = base_url.trim();
    if trimmed_base.is_empty() {
        return None;
    }

    let path = custom_path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or(OPENCODE_FREE_DEFAULT_BASE_PATH);

    Some(format!("{}{}", trimmed_base.trim_end_matches('/'), path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_url_with_default_path() {
        let url = build_opencode_free_url("https://opencode.ai/zen", None);
        assert_eq!(url, Some("https://opencode.ai/zen/v1".to_string()));
    }

    #[test]
    fn builds_url_with_custom_path() {
        let url = build_opencode_free_url("https://opencode.ai/zen", Some("/v1/chat/completions"));
        assert_eq!(
            url,
            Some("https://opencode.ai/zen/v1/chat/completions".to_string())
        );
    }

    #[test]
    fn strips_trailing_slash_from_base() {
        let url = build_opencode_free_url("https://opencode.ai/zen/", None);
        assert_eq!(url, Some("https://opencode.ai/zen/v1".to_string()));
    }

    #[test]
    fn returns_none_for_empty_base() {
        assert_eq!(build_opencode_free_url("", None), None);
        assert_eq!(build_opencode_free_url("  ", None), None);
    }

    #[test]
    fn ignores_empty_custom_path() {
        let url = build_opencode_free_url("https://opencode.ai/zen", Some(""));
        assert_eq!(url, Some("https://opencode.ai/zen/v1".to_string()));
    }
}
