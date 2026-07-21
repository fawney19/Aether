use std::collections::BTreeMap;

use url::form_urlencoded;

pub fn build_claude_code_messages_url(upstream_base_url: &str, query: Option<&str>) -> String {
    let (trimmed_base_url, base_query) = split_query(upstream_base_url.trim());
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    let mut url =
        if trimmed_base_url.ends_with("/v1/messages") || trimmed_base_url.ends_with("/messages") {
            trimmed_base_url.to_string()
        } else if trimmed_base_url.ends_with("/v1") {
            format!("{trimmed_base_url}/messages")
        } else {
            format!("{trimmed_base_url}/v1/messages")
        };
    append_merged_query(&mut url, base_query, query);
    url
}

fn split_query(value: &str) -> (&str, Option<&str>) {
    value
        .split_once('?')
        .map(|(base, query)| (base, Some(query)))
        .unwrap_or((value, None))
}

fn append_merged_query(url: &mut String, base_query: Option<&str>, request_query: Option<&str>) {
    let Some(query) = merge_query_layers(base_query, request_query) else {
        return;
    };
    if url.contains('?') {
        url.push('&');
    } else {
        url.push('?');
    }
    url.push_str(&query);
}

fn merge_query_layers(base_query: Option<&str>, request_query: Option<&str>) -> Option<String> {
    let mut merged = BTreeMap::new();
    for source in [base_query, request_query] {
        let Some(source) = source.map(str::trim).filter(|value| !value.is_empty()) else {
            continue;
        };
        for (key, value) in form_urlencoded::parse(source.as_bytes()) {
            merged.insert(key.into_owned(), value.into_owned());
        }
    }
    if merged.is_empty() {
        return None;
    }
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in merged {
        serializer.append_pair(&key, &value);
    }
    Some(serializer.finish())
}

#[cfg(test)]
mod tests {
    use super::build_claude_code_messages_url;

    #[test]
    fn keeps_existing_messages_suffix_without_duplication() {
        assert_eq!(
            build_claude_code_messages_url("https://api.anthropic.com/v1/messages", None),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn appends_messages_and_merges_query() {
        assert_eq!(
            build_claude_code_messages_url(
                "https://api.anthropic.com/v1?beta=true",
                Some("foo=bar"),
            ),
            "https://api.anthropic.com/v1/messages?beta=true&foo=bar"
        );
    }
}
