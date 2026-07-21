use std::collections::BTreeMap;

use url::form_urlencoded;

pub const ANTIGRAVITY_V1INTERNAL_PATH_TEMPLATE: &str = "/v1internal:{action}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntigravityRequestUrlAction {
    GenerateContent,
    StreamGenerateContent,
}

impl AntigravityRequestUrlAction {
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

pub fn build_antigravity_v1internal_url(
    base_url: &str,
    action: AntigravityRequestUrlAction,
    query: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    let trimmed_base = base_url.trim();
    if trimmed_base.is_empty() {
        return None;
    }

    let path = ANTIGRAVITY_V1INTERNAL_PATH_TEMPLATE.replace("{action}", action.as_str());
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
