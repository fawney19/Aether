use serde_json::Value;

pub const UPSTREAM_IS_STREAM_KEY: &str = "upstream_is_stream";

pub fn resolve_openai_prompt_cache_ttl_minutes(
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body: &Value,
) -> Option<i64> {
    OpenAiPromptCacheApi::parse(provider_api_format)?;
    let request = body.as_object()?;
    let explicit_ttl = request
        .get("prompt_cache_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("ttl"))
        .and_then(Value::as_str);
    if explicit_ttl == Some("30m") {
        return Some(30);
    }

    let capability_model =
        crate::model::openai_model_capability_identity(provider_model, source_model);
    crate::openai_model_supports_prompt_cache_options(&capability_model).then_some(30)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiPromptCacheApi {
    Chat,
    Responses,
}

impl OpenAiPromptCacheApi {
    pub fn parse(api_format: &str) -> Option<Self> {
        match crate::normalize_api_format_alias(api_format).as_str() {
            "openai:chat" => Some(Self::Chat),
            "openai:responses" | "openai:responses:compact" => Some(Self::Responses),
            _ => None,
        }
    }
}

pub fn sanitize_request_path(path: &str) -> Option<String> {
    let path = path
        .trim()
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or_else(|| path.trim())
        .trim();
    if path.is_empty() {
        return None;
    }
    if path
        .strip_prefix("/v1/live/")
        .is_some_and(|call_id| !call_id.is_empty())
    {
        // A Live call id identifies an in-progress WebRTC session. Keep this
        // bearer-like capability out of logs and persisted request metadata,
        // including for malformed routes that will later be rejected.
        return Some("/v1/live/{call_id}".to_string());
    }
    Some(sanitize_sensitive_request_path(path))
}

fn sanitize_sensitive_request_path(path: &str) -> String {
    for prefix in ["/install-tunnel/", "/install/", "/i/"] {
        if path
            .strip_prefix(prefix)
            .is_some_and(|secret| !secret.is_empty())
        {
            return format!("{prefix}[redacted]");
        }
    }
    path.to_string()
}

pub fn sanitize_request_query_string(query: &str) -> Option<String> {
    let query = query.trim().trim_start_matches('?').trim();
    if query.is_empty() {
        return None;
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if request_query_key_is_safe_to_trace(key.as_ref()) {
            serializer.append_pair(key.as_ref(), value.as_ref());
        }
    }
    let sanitized = serializer.finish();
    (!sanitized.is_empty()).then_some(sanitized)
}

pub fn sanitize_request_path_and_query(path: &str, query: Option<&str>) -> Option<String> {
    let trimmed = path.trim();
    let (path, embedded_query) = trimmed
        .split_once('?')
        .map(|(path, query)| (path.trim(), Some(query)))
        .unwrap_or((trimmed, None));
    if path.is_empty() {
        return None;
    }

    let sanitized_path = sanitize_request_path(path)?;
    let sanitized_query = query
        .and_then(|query| sanitize_request_query_string_for_path(path, query))
        .or_else(|| {
            embedded_query.and_then(|query| sanitize_request_query_string_for_path(path, query))
        });
    Some(match sanitized_query {
        Some(query) => format!("{sanitized_path}?{query}"),
        None => sanitized_path,
    })
}

fn sanitize_request_query_string_for_path(path: &str, query: &str) -> Option<String> {
    if path != "/v1/realtime" {
        return sanitize_request_query_string(query);
    }

    let query = query.trim().trim_start_matches('?').trim();
    if query.is_empty() {
        return None;
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    let mut redacted_call_id = false;
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if key.eq_ignore_ascii_case("call_id") {
            if !value.trim().is_empty() && !redacted_call_id {
                serializer.append_pair("call_id", "{call_id}");
                redacted_call_id = true;
            }
        } else if request_query_key_is_safe_to_trace(key.as_ref()) {
            serializer.append_pair(key.as_ref(), value.as_ref());
        }
    }
    let sanitized = serializer.finish();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn request_query_key_is_safe_to_trace(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "alt" | "view" | "pagesize" | "page_size" | "limit" | "offset" | "model"
    )
}
