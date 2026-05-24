use std::collections::BTreeMap;

use super::provider_types::is_codex_cli_backend_url;
use url::form_urlencoded;
use url::Url;

pub fn build_openai_chat_url(upstream_base_url: &str, query: Option<&str>) -> String {
    let (trimmed, base_query) = split_base_url_query(upstream_base_url);
    let trimmed = trimmed.trim_end_matches('/');
    let mut url =
        if trimmed.ends_with("/v1") || google_openai_compat_base_includes_api_root(trimmed) {
            format!("{trimmed}/chat/completions")
        } else {
            format!("{trimmed}/v1/chat/completions")
        };
    append_merged_query(&mut url, base_query, None, query, &[]);
    url
}

pub fn build_openai_responses_url(
    upstream_base_url: &str,
    query: Option<&str>,
    compact: bool,
) -> String {
    let (trimmed, base_query) = split_base_url_query(upstream_base_url);
    let trimmed = trimmed.trim_end_matches('/');
    let suffix = if compact {
        "/responses/compact"
    } else {
        "/responses"
    };
    let mut url = if is_codex_cli_backend_url(trimmed)
        || trimmed.ends_with("/codex")
        || trimmed.ends_with("/v1")
    {
        format!("{trimmed}{suffix}")
    } else {
        format!("{trimmed}/v1{suffix}")
    };
    append_merged_query(&mut url, base_query, None, query, &[]);
    url
}

pub fn build_openai_image_url(
    upstream_base_url: &str,
    request_path: Option<&str>,
    query: Option<&str>,
) -> String {
    let (trimmed, base_query) = split_base_url_query(upstream_base_url);
    let trimmed = trimmed.trim_end_matches('/');
    let suffix = openai_image_path_suffix(request_path);
    let mut url = if openai_image_base_includes_operation_path(trimmed) {
        trimmed.to_string()
    } else if trimmed.ends_with("/v1") || google_openai_compat_base_includes_api_root(trimmed) {
        format!("{trimmed}{suffix}")
    } else {
        format!("{trimmed}/v1{suffix}")
    };
    append_merged_query(&mut url, base_query, None, query, &[]);
    url
}

fn openai_image_path_suffix(request_path: Option<&str>) -> &'static str {
    match request_path
        .map(str::trim)
        .map(|value| value.trim_end_matches('/'))
    {
        Some("/v1/images/edits") | Some("/images/edits") => "/images/edits",
        _ => "/images/generations",
    }
}

fn openai_image_base_includes_operation_path(base_url: &str) -> bool {
    let path = Url::parse(base_url)
        .ok()
        .map(|url| url.path().trim_end_matches('/').to_string())
        .unwrap_or_else(|| base_url.trim_end_matches('/').to_string());
    path.ends_with("/images/generations") || path.ends_with("/images/edits")
}

pub fn build_claude_messages_url(upstream_base_url: &str, query: Option<&str>) -> String {
    let (trimmed, base_query) = split_base_url_query(upstream_base_url);
    let trimmed = trimmed.trim_end_matches('/');
    let mut url = if trimmed.ends_with("/v1") {
        format!("{trimmed}/messages")
    } else {
        format!("{trimmed}/v1/messages")
    };
    append_merged_query(&mut url, base_query, None, query, &[]);
    url
}

pub fn build_gemini_content_url(
    upstream_base_url: &str,
    model: &str,
    stream: bool,
    query: Option<&str>,
) -> Option<String> {
    let (trimmed_base_url, base_query) = split_base_url_query(upstream_base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    let trimmed_model = model.trim();
    if trimmed_base_url.is_empty() || trimmed_model.is_empty() {
        return None;
    }

    let operation = if stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    let mut url = if trimmed_base_url.ends_with("/v1") || trimmed_base_url.ends_with("/v1beta") {
        format!("{trimmed_base_url}/models/{trimmed_model}:{operation}")
    } else if gemini_content_base_url_contains_model_path(trimmed_base_url) {
        let trimmed_base_url = strip_gemini_content_action(trimmed_base_url);
        format!("{trimmed_base_url}:{operation}")
    } else {
        format!("{trimmed_base_url}/v1beta/models/{trimmed_model}:{operation}")
    };
    append_merged_query(&mut url, base_query, None, query, &["key"]);
    Some(url)
}

pub fn normalize_gemini_content_action_path(path: &str, stream: bool) -> String {
    let trimmed = path.trim();
    let (path, query) = split_path_query(trimmed);
    let action = if stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    let normalized = strip_gemini_content_action(path);
    let normalized = if normalized.len() == path.len() {
        path.to_string()
    } else {
        format!("{normalized}:{action}")
    };
    match query {
        Some(query) => format!("{normalized}?{query}"),
        None => normalized,
    }
}

fn strip_gemini_content_action(value: &str) -> &str {
    value
        .strip_suffix(":streamGenerateContent")
        .or_else(|| value.strip_suffix(":generateContent"))
        .unwrap_or(value)
}

fn gemini_content_base_url_contains_model_path(value: &str) -> bool {
    value.contains("/v1/models/") || value.contains("/v1beta/models/")
}

pub fn build_gemini_video_predict_long_running_url(
    upstream_base_url: &str,
    model: &str,
    query: Option<&str>,
) -> Option<String> {
    let (trimmed_base_url, base_query) = split_base_url_query(upstream_base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    let trimmed_model = model.trim();
    if trimmed_base_url.is_empty() || trimmed_model.is_empty() {
        return None;
    }

    let mut url = if trimmed_base_url.ends_with("/v1") || trimmed_base_url.ends_with("/v1beta") {
        format!("{trimmed_base_url}/models/{trimmed_model}:predictLongRunning")
    } else if gemini_content_base_url_contains_model_path(trimmed_base_url) {
        format!("{trimmed_base_url}:predictLongRunning")
    } else {
        format!("{trimmed_base_url}/v1beta/models/{trimmed_model}:predictLongRunning")
    };
    append_merged_query(&mut url, base_query, None, query, &["key"]);
    Some(url)
}

pub fn build_passthrough_path_url(
    upstream_base_url: &str,
    path: &str,
    query: Option<&str>,
    blocked_keys: &[&str],
) -> Option<String> {
    let (trimmed_base_url, base_query) = split_base_url_query(upstream_base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    let trimmed_path = path.trim();
    if trimmed_base_url.is_empty() || trimmed_path.is_empty() {
        return None;
    }

    let (trimmed_path, path_query) = split_path_query(trimmed_path);
    let normalized_base_url =
        if trimmed_base_url.ends_with("/v1beta") && trimmed_path.starts_with("/v1beta") {
            trimmed_base_url.trim_end_matches("/v1beta")
        } else {
            trimmed_base_url
        };

    let mut url = format!("{normalized_base_url}{trimmed_path}");
    append_merged_query(&mut url, base_query, path_query, query, blocked_keys);
    Some(url)
}

pub fn build_gemini_files_passthrough_url(
    upstream_base_url: &str,
    path: &str,
    query: Option<&str>,
) -> Option<String> {
    let (trimmed_base_url, base_query) = split_base_url_query(upstream_base_url);
    let trimmed_base_url = trimmed_base_url.trim_end_matches('/');
    let trimmed_path = path.trim();
    if trimmed_base_url.is_empty() || trimmed_path.is_empty() {
        return None;
    }

    let (trimmed_path, path_query) = split_path_query(trimmed_path);
    let normalized_base_url = if trimmed_base_url.ends_with("/v1beta")
        && (trimmed_path.starts_with("/v1beta/") || trimmed_path.starts_with("/upload/v1beta/"))
    {
        trimmed_base_url.trim_end_matches("/v1beta")
    } else {
        trimmed_base_url
    };

    let mut url = format!("{normalized_base_url}{trimmed_path}");
    append_merged_query(&mut url, base_query, path_query, query, &["key"]);
    Some(url)
}

fn split_base_url_query(base_url: &str) -> (&str, Option<&str>) {
    let trimmed = base_url.trim();
    trimmed
        .split_once('?')
        .map(|(base, query)| (base, Some(query)))
        .unwrap_or((trimmed, None))
}

pub(crate) fn google_openai_compat_base_includes_api_root(base_url: &str) -> bool {
    let Ok(parsed) = Url::parse(base_url.trim()) else {
        return false;
    };
    let Some(host) = parsed.host_str().map(|value| value.to_ascii_lowercase()) else {
        return false;
    };
    let path = parsed.path().trim_end_matches('/');

    if host == "generativelanguage.googleapis.com" {
        return path == "/v1beta/openai" || path == "/v1/openai";
    }

    if looks_like_vertex_ai_host(&host) {
        return path.ends_with("/endpoints/openapi");
    }

    false
}

fn looks_like_vertex_ai_host(host: &str) -> bool {
    const VERTEX_AI_HOST: &str = "aiplatform.googleapis.com";
    host == VERTEX_AI_HOST
        || host.ends_with(&format!(".{VERTEX_AI_HOST}"))
        || host.ends_with(&format!("-{VERTEX_AI_HOST}"))
}

fn split_path_query(path: &str) -> (&str, Option<&str>) {
    path.split_once('?')
        .map(|(path, query)| (path, Some(query)))
        .unwrap_or((path, None))
}

fn append_merged_query(
    url: &mut String,
    base_query: Option<&str>,
    path_query: Option<&str>,
    request_query: Option<&str>,
    blocked_keys: &[&str],
) {
    let Some(query) = merge_query_layers(base_query, path_query, request_query, blocked_keys)
    else {
        return;
    };
    if url.contains('?') {
        url.push('&');
    } else {
        url.push('?');
    }
    url.push_str(&query);
}

fn merge_query_layers(
    base_query: Option<&str>,
    path_query: Option<&str>,
    request_query: Option<&str>,
    blocked_keys: &[&str],
) -> Option<String> {
    if blocked_keys.is_empty()
        && path_query.is_none()
        && base_query.is_none()
        && request_query
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    {
        return request_query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
    }

    let mut merged = BTreeMap::new();
    for source in [base_query, path_query, request_query] {
        merge_query_string(&mut merged, source, blocked_keys);
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

fn merge_query_string(
    out: &mut BTreeMap<String, String>,
    query: Option<&str>,
    blocked_keys: &[&str],
) {
    let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };

    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if blocked_keys
            .iter()
            .any(|blocked| key.as_ref().eq_ignore_ascii_case(blocked))
        {
            continue;
        }
        out.insert(key.into_owned(), value.into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_gemini_content_url, build_gemini_files_passthrough_url,
        build_gemini_video_predict_long_running_url, build_openai_chat_url, build_openai_image_url,
        build_openai_responses_url, build_passthrough_path_url,
        normalize_gemini_content_action_path,
    };

    #[test]
    fn merges_base_url_query_for_same_format_urls() {
        assert_eq!(
            build_openai_chat_url(
                "https://api.openai.example/v1?tenant=demo",
                Some("mode=fast&tenant=override")
            ),
            "https://api.openai.example/v1/chat/completions?mode=fast&tenant=override"
        );
    }

    #[test]
    fn openai_chat_url_preserves_google_openai_compat_roots() {
        assert_eq!(
            build_openai_chat_url(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                Some("trace=1")
            ),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions?trace=1"
        );
        assert_eq!(
            build_openai_chat_url(
                "https://aiplatform.googleapis.com/v1/projects/project-1/locations/global/endpoints/openapi",
                None,
            ),
            "https://aiplatform.googleapis.com/v1/projects/project-1/locations/global/endpoints/openapi/chat/completions"
        );
    }

    #[test]
    fn openai_responses_url_preserves_codex_path_prefix() {
        assert_eq!(
            build_openai_responses_url("https://tiger.bookapi.cc/codex", None, false),
            "https://tiger.bookapi.cc/codex/responses"
        );
        assert_eq!(
            build_openai_responses_url("https://tiger.bookapi.cc/codex?tenant=demo", None, true),
            "https://tiger.bookapi.cc/codex/responses/compact?tenant=demo"
        );
    }

    #[test]
    fn openai_image_url_uses_images_surface() {
        assert_eq!(
            build_openai_image_url(
                "https://api.openai.example/v1?tenant=demo",
                Some("/v1/images/generations"),
                Some("trace=1")
            ),
            "https://api.openai.example/v1/images/generations?tenant=demo&trace=1"
        );
        assert_eq!(
            build_openai_image_url("https://api.openai.example", Some("/v1/images/edits"), None),
            "https://api.openai.example/v1/images/edits"
        );
    }

    #[test]
    fn merges_base_url_query_for_dynamic_gemini_content_urls() {
        assert_eq!(
            build_gemini_content_url(
                "https://generativelanguage.googleapis.com/v1beta?alt=sse",
                "gemini-2.5-pro",
                true,
                Some("foo=bar&key=secret")
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse&foo=bar"
            )
        );
    }

    #[test]
    fn gemini_content_urls_rewrite_existing_base_action_for_stream_mode() {
        assert_eq!(
            build_gemini_content_url(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent",
                "ignored-model",
                true,
                Some("foo=bar")
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?foo=bar"
            )
        );
        assert_eq!(
            build_gemini_content_url(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent",
                "ignored-model",
                false,
                Some("foo=bar")
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent?foo=bar"
            )
        );
        assert_eq!(
            build_gemini_content_url(
                "https://generativelanguage.googleapis.com/v1/models/gemini-2.5-pro:generateContent",
                "ignored-model",
                true,
                Some("foo=bar")
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1/models/gemini-2.5-pro:streamGenerateContent?foo=bar"
            )
        );
    }

    #[test]
    fn normalizes_gemini_content_action_in_custom_paths() {
        assert_eq!(
            normalize_gemini_content_action_path(
                "/v1beta/models/gemini-2.5-pro:generateContent?alt=sse",
                true
            ),
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            normalize_gemini_content_action_path(
                "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
                false
            ),
            "/v1beta/models/gemini-2.5-pro:generateContent"
        );
    }

    #[test]
    fn merges_base_path_and_request_query_for_passthrough_paths() {
        assert_eq!(
            build_passthrough_path_url(
                "https://api.openai.example/v1?tenant=demo",
                "/videos/generations?variant=video",
                Some("size=1024"),
                &[]
            )
            .as_deref(),
            Some(
                "https://api.openai.example/v1/videos/generations?size=1024&tenant=demo&variant=video"
            )
        );
    }

    #[test]
    fn merges_base_url_query_for_gemini_files_passthrough_urls() {
        assert_eq!(
            build_gemini_files_passthrough_url(
                "https://generativelanguage.googleapis.com/v1beta?alt=media",
                "/upload/v1beta/files?uploadType=resumable",
                Some("key=secret&pageSize=10")
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/upload/v1beta/files?alt=media&pageSize=10&uploadType=resumable"
            )
        );
    }

    #[test]
    fn merges_base_url_query_for_gemini_video_urls() {
        assert_eq!(
            build_gemini_video_predict_long_running_url(
                "https://generativelanguage.googleapis.com/v1beta?alt=sse",
                "veo-3.0-generate-preview",
                Some("foo=bar&key=secret")
            )
            .as_deref(),
            Some(
                "https://generativelanguage.googleapis.com/v1beta/models/veo-3.0-generate-preview:predictLongRunning?alt=sse&foo=bar"
            )
        );
    }
}
