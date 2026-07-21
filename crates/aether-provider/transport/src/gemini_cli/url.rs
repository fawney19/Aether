use std::collections::BTreeMap;

use url::form_urlencoded;

pub const GEMINI_CLI_USER_AGENT: &str = "GeminiCLI/0.1.5 (Windows; AMD64)";
pub const GEMINI_CLI_V1INTERNAL_PATH_TEMPLATE: &str = "/v1internal:{action}";
pub const GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH: &str = "/v1internal:retrieveUserQuota";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiCliRequestUrlAction {
    GenerateContent,
    StreamGenerateContent,
}

impl GeminiCliRequestUrlAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::GenerateContent => "generateContent",
            Self::StreamGenerateContent => "streamGenerateContent",
        }
    }

    fn is_stream(self) -> bool {
        matches!(self, Self::StreamGenerateContent)
    }
}

pub fn build_gemini_cli_v1internal_url(
    base_url: &str,
    action: GeminiCliRequestUrlAction,
    query: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    let trimmed_base = base_url.trim();
    if trimmed_base.is_empty() {
        return None;
    }

    let path = GEMINI_CLI_V1INTERNAL_PATH_TEMPLATE.replace("{action}", action.as_str());
    let mut url = format!("{}{}", trimmed_base.trim_end_matches('/'), path);

    let mut params = BTreeMap::new();
    if let Some(query) = query {
        for (key, value) in query {
            let key = key.trim();
            let value = value.trim();
            if key.is_empty()
                || value.is_empty()
                || key.eq_ignore_ascii_case("beta")
                || key.eq_ignore_ascii_case("key")
            {
                continue;
            }
            params.insert(key.to_string(), value.to_string());
        }
    }
    if action.is_stream() {
        params
            .entry(String::from("alt"))
            .or_insert_with(|| String::from("sse"));
    }

    if !params.is_empty() {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        for (key, value) in params {
            serializer.append_pair(key.as_str(), value.as_str());
        }
        let query_string = serializer.finish();
        if !query_string.is_empty() {
            url.push('?');
            url.push_str(&query_string);
        }
    }

    Some(url)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        build_gemini_cli_v1internal_url, GeminiCliRequestUrlAction,
        GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH,
    };

    #[test]
    fn builds_gemini_cli_stream_url_with_alt_sse() {
        let query = BTreeMap::from([
            ("foo".to_string(), "bar".to_string()),
            ("key".to_string(), "blocked".to_string()),
        ]);

        assert_eq!(
            build_gemini_cli_v1internal_url(
                "https://cloudcode-pa.googleapis.com/",
                GeminiCliRequestUrlAction::StreamGenerateContent,
                Some(&query),
            )
            .as_deref(),
            Some("https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse&foo=bar")
        );
    }

    #[test]
    fn builds_gemini_cli_sync_url_without_stream_query() {
        assert_eq!(
            build_gemini_cli_v1internal_url(
                "https://cloudcode-pa.googleapis.com",
                GeminiCliRequestUrlAction::GenerateContent,
                None,
            )
            .as_deref(),
            Some("https://cloudcode-pa.googleapis.com/v1internal:generateContent")
        );
        assert_eq!(
            GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH,
            "/v1internal:retrieveUserQuota"
        );
    }
}
