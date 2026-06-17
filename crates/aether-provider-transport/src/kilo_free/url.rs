pub const KILO_FREE_DEFAULT_BASE_URL: &str = "https://api.kilo.ai/api/gateway";

pub fn build_kilo_free_url(base_url: &str) -> Option<String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_url_from_base() {
        let url = build_kilo_free_url(KILO_FREE_DEFAULT_BASE_URL);
        assert_eq!(url, Some("https://api.kilo.ai/api/gateway".to_string()));
    }

    #[test]
    fn rejects_empty_base_url() {
        let url = build_kilo_free_url("");
        assert_eq!(url, None);
    }

    #[test]
    fn trims_whitespace_from_base_url() {
        let url = build_kilo_free_url("  https://api.kilo.ai/api/gateway  ");
        assert_eq!(url, Some("https://api.kilo.ai/api/gateway".to_string()));
    }
}
