use std::collections::BTreeMap;

use serde_json::Value;
use url::form_urlencoded;

const UNSAFE_AUTH_CONFIG_HEADER_NAMES: &[&str] = &["content-length", "host", "proxy-authorization"];
const RUNTIME_ONLY_AUTH_CONFIG_HEADER_NAMES: &[&str] = &[
    "api-key",
    "authorization",
    "content-type",
    "cookie",
    "x-api-key",
    "x-goog-api-key",
];
const UNSAFE_AUTH_CONFIG_QUERY_NAMES: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "authorization",
    "key",
    "token",
];
const SENSITIVE_AUTH_CONFIG_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "apikey",
    "authorization",
    "client_email",
    "client_id",
    "client_secret",
    "expires_at",
    "id_token",
    "key",
    "private_key",
    "refresh_token",
    "service_account",
    "token",
    "token_uri",
];
const IGNORABLE_AUTH_CONFIG_METADATA_KEYS: &[&str] = &[
    "account_id",
    "account_name",
    "account_user_id",
    "auth_method",
    "access_token_import_temporary",
    "email",
    "expires_at",
    "is_fedramp",
    "model_regions",
    "organizations",
    "plan_type",
    "project_id",
    "provider_type",
    "refresh_token_import_error",
    "region",
    "scope",
    "tier",
    "token_type",
    "updated_at",
    "user_id",
    "workspace_id",
    "workspace_name",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAuthConfigSafeSubset {
    pub headers: BTreeMap<String, String>,
    pub query: BTreeMap<String, String>,
    pub path: Option<String>,
}

impl LocalAuthConfigSafeSubset {
    fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.query.is_empty() && self.path.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalAuthConfigAbsorption {
    Missing,
    Unsupported,
    Absorbed {
        base_url: String,
        header_rules: Option<Value>,
        custom_path: Option<String>,
    },
}

pub fn apply_local_auth_config_header_overrides(
    headers: &mut BTreeMap<String, String>,
    raw_auth_config: Option<&str>,
) {
    let Some(raw_auth_config) = raw_auth_config
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(raw_auth_config) else {
        return;
    };
    let Some(object) = parsed.as_object() else {
        return;
    };

    let mut overrides = BTreeMap::new();
    collect_auth_config_header_overrides(object, &mut overrides);
    for (key, value) in overrides {
        headers.insert(key, value);
    }
}

pub fn absorb_local_auth_config_safe_subset(
    base_url: &str,
    header_rules: Option<Value>,
    custom_path: Option<String>,
    raw_auth_config: Option<&str>,
) -> LocalAuthConfigAbsorption {
    let Some(raw_auth_config) = raw_auth_config
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return LocalAuthConfigAbsorption::Missing;
    };

    let subset = match parse_local_auth_config_safe_subset(raw_auth_config) {
        Ok(subset) => subset,
        Err(()) => return LocalAuthConfigAbsorption::Unsupported,
    };
    if subset.is_empty() {
        return LocalAuthConfigAbsorption::Unsupported;
    }
    let header_rules = match merge_auth_config_header_rules(header_rules, &subset.headers) {
        Some(rules) => rules,
        None => return LocalAuthConfigAbsorption::Unsupported,
    };
    let base_url = match merge_auth_config_base_url(base_url, &subset.query) {
        Some(value) => value,
        None => return LocalAuthConfigAbsorption::Unsupported,
    };
    let custom_path = match merge_auth_config_custom_path(custom_path, subset.path) {
        Some(path) => path,
        None => return LocalAuthConfigAbsorption::Unsupported,
    };

    LocalAuthConfigAbsorption::Absorbed {
        base_url,
        header_rules,
        custom_path,
    }
}

fn parse_local_auth_config_safe_subset(raw: &str) -> Result<LocalAuthConfigSafeSubset, ()> {
    let parsed: Value = serde_json::from_str(raw).map_err(|_| ())?;
    let object = parsed.as_object().ok_or(())?;

    let mut headers = BTreeMap::new();
    let mut query = BTreeMap::new();
    let mut path = None;

    parse_local_auth_config_object(object, &mut headers, &mut query, &mut path, true)?;
    if headers
        .keys()
        .any(|key| RUNTIME_ONLY_AUTH_CONFIG_HEADER_NAMES.contains(&key.as_str()))
    {
        return Err(());
    }

    Ok(LocalAuthConfigSafeSubset {
        headers,
        query,
        path,
    })
}

fn collect_auth_config_header_overrides(
    object: &serde_json::Map<String, Value>,
    out: &mut BTreeMap<String, String>,
) {
    for (key, value) in object {
        let normalized = key.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "headers" | "extra_headers" | "extraheaders" => {
                merge_header_string_map_lenient(out, value);
            }
            "transport" | "request" => {
                if let Some(nested) = value.as_object() {
                    collect_auth_config_header_overrides(nested, out);
                }
            }
            _ => {}
        }
    }
}

fn parse_local_auth_config_object(
    object: &serde_json::Map<String, Value>,
    headers: &mut BTreeMap<String, String>,
    query: &mut BTreeMap<String, String>,
    path: &mut Option<String>,
    allow_metadata: bool,
) -> Result<(), ()> {
    for (key, value) in object {
        let normalized = key.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "headers" | "extra_headers" | "extraheaders" => {
                merge_header_string_map(headers, value)?
            }
            "query" | "query_params" | "queryparams" => {
                merge_string_map(query, value, normalize_auth_config_query_key)?
            }
            "path" | "custom_path" => {
                let value = value.as_str().ok_or(())?;
                let normalized = normalize_auth_config_path(value).ok_or(())?;
                *path = Some(normalized);
            }
            "custompath" => {
                let value = value.as_str().ok_or(())?;
                let normalized = normalize_auth_config_path(value).ok_or(())?;
                *path = Some(normalized);
            }
            "transport" | "request" => {
                let nested = value.as_object().ok_or(())?;
                parse_local_auth_config_object(nested, headers, query, path, false)?;
            }
            _ if allow_metadata && is_ignorable_auth_config_metadata_key(&normalized) => {}
            _ if allow_metadata && is_sensitive_auth_config_key(&normalized) => return Err(()),
            _ => return Err(()),
        }
    }
    Ok(())
}

fn merge_string_map(
    out: &mut BTreeMap<String, String>,
    value: &Value,
    normalize_key: fn(&str) -> Option<String>,
) -> Result<(), ()> {
    let object = value.as_object().ok_or(())?;
    for (raw_key, raw_value) in object {
        let key = normalize_key(raw_key).ok_or(())?;
        let value = parse_static_auth_config_value(raw_value).ok_or(())?;
        out.insert(key, value);
    }
    Ok(())
}

fn parse_static_auth_config_value(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => {
            let normalized = raw.trim();
            if normalized.is_empty() {
                None
            } else {
                Some(normalized.to_string())
            }
        }
        Value::Number(raw) => Some(raw.to_string()),
        Value::Bool(raw) => Some(raw.to_string()),
        _ => None,
    }
}

fn merge_header_string_map(out: &mut BTreeMap<String, String>, value: &Value) -> Result<(), ()> {
    let object = value.as_object().ok_or(())?;
    for (raw_key, raw_value) in object {
        let key = normalize_auth_config_header_name(raw_key).ok_or(())?;
        let value = parse_static_auth_config_header_value(raw_value).ok_or(())?;
        out.insert(key, value);
    }
    Ok(())
}

fn merge_header_string_map_lenient(out: &mut BTreeMap<String, String>, value: &Value) {
    let Some(object) = value.as_object() else {
        return;
    };
    for (raw_key, raw_value) in object {
        let Some(key) = normalize_auth_config_header_name(raw_key) else {
            continue;
        };
        let Some(value) = parse_static_auth_config_header_value(raw_value) else {
            continue;
        };
        out.insert(key, value);
    }
}

fn parse_static_auth_config_header_value(value: &Value) -> Option<String> {
    let value = parse_static_auth_config_value(value)?;
    http::header::HeaderValue::from_str(&value)
        .is_ok()
        .then_some(value)
}

fn normalize_auth_config_header_name(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.chars().any(|char| char.is_ascii_control())
        || UNSAFE_AUTH_CONFIG_HEADER_NAMES.contains(&value.as_str())
    {
        return None;
    }
    http::header::HeaderName::from_bytes(value.as_bytes())
        .ok()
        .map(|name| name.as_str().to_string())
}

fn normalize_auth_config_query_key(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty()
        || value.chars().any(|char| matches!(char, '&' | '=' | '#'))
        || value.chars().any(|char| char.is_ascii_control())
        || UNSAFE_AUTH_CONFIG_QUERY_NAMES
            .iter()
            .any(|blocked| value.eq_ignore_ascii_case(blocked))
    {
        return None;
    }
    Some(value.to_string())
}

fn is_sensitive_auth_config_key(key: &str) -> bool {
    SENSITIVE_AUTH_CONFIG_KEYS
        .iter()
        .any(|blocked| key.eq_ignore_ascii_case(blocked))
}

fn is_ignorable_auth_config_metadata_key(key: &str) -> bool {
    IGNORABLE_AUTH_CONFIG_METADATA_KEYS
        .iter()
        .any(|allowed| key.eq_ignore_ascii_case(allowed))
}

fn normalize_auth_config_path(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty()
        || !value.starts_with('/')
        || value.contains("://")
        || value
            .chars()
            .any(|char| matches!(char, '{' | '}' | '$' | '#'))
        || value.chars().any(|char| char.is_ascii_control())
    {
        return None;
    }
    Some(value.to_string())
}

fn merge_auth_config_header_rules(
    existing_rules: Option<Value>,
    headers: &BTreeMap<String, String>,
) -> Option<Option<Value>> {
    if headers.is_empty() {
        return Some(existing_rules);
    }

    let mut merged = match existing_rules {
        Some(Value::Array(items)) => items,
        Some(_) => return None,
        None => Vec::new(),
    };
    for (key, value) in headers {
        merged.push(serde_json::json!({
            "action": "set",
            "key": key,
            "value": value,
        }));
    }
    Some(Some(Value::Array(merged)))
}

fn merge_auth_config_custom_path(
    existing_custom_path: Option<String>,
    path_override: Option<String>,
) -> Option<Option<String>> {
    let base_path = path_override.or(existing_custom_path);
    let Some(base_path) = base_path else {
        return Some(None);
    };

    let (path_only, query) = split_path_and_query(&base_path)?;
    if query.is_empty() {
        return Some(Some(path_only));
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in query {
        serializer.append_pair(&key, &value);
    }
    Some(Some(format!("{path_only}?{}", serializer.finish())))
}

fn merge_auth_config_base_url(base_url: &str, query: &BTreeMap<String, String>) -> Option<String> {
    if query.is_empty() {
        return Some(base_url.to_string());
    }

    let raw_base_url = base_url.trim();
    let had_implicit_root = raw_base_url
        .split_once("://")
        .map(|(_, rest)| {
            let authority = rest.split_once('?').map(|(head, _)| head).unwrap_or(rest);
            !authority.contains('/')
        })
        .unwrap_or(false);

    let mut url = url::Url::parse(raw_base_url).ok()?;
    let mut merged = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        merged.insert(key.into_owned(), value.to_string());
    }
    for (key, value) in query {
        merged.insert(key.clone(), value.clone());
    }

    if merged.is_empty() {
        url.set_query(None);
        return Some(url.to_string());
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in merged {
        serializer.append_pair(&key, &value);
    }
    url.set_query(Some(&serializer.finish()));

    let mut normalized = url.to_string();
    if had_implicit_root {
        normalized = normalized.replacen("/?", "?", 1);
    }
    Some(normalized)
}

fn split_path_and_query(path: &str) -> Option<(String, BTreeMap<String, String>)> {
    let normalized = normalize_auth_config_path(path)?;
    let (path_only, query_part) = if let Some((path, query)) = normalized.split_once('?') {
        (path.to_string(), Some(query))
    } else {
        (normalized, None)
    };
    if path_only.is_empty() {
        return None;
    }

    let mut query = BTreeMap::new();
    if let Some(query_part) = query_part.filter(|value| !value.trim().is_empty()) {
        for (key, value) in form_urlencoded::parse(query_part.as_bytes()) {
            let key = normalize_auth_config_query_key(key.as_ref())?;
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            query.insert(key, value.to_string());
        }
    }

    Some((path_only, query))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        absorb_local_auth_config_safe_subset, apply_local_auth_config_header_overrides,
        LocalAuthConfigAbsorption,
    };

    #[test]
    fn absorbs_static_headers_and_query_into_existing_transport_fields() {
        let result = absorb_local_auth_config_safe_subset(
            "https://api.openai.example/v1",
            Some(json!([{"action":"set","key":"x-base","value":"1"}])),
            None,
            Some(
                r#"{
                    "headers": {"x-account-id": "acc-1"},
                    "query": {"tenant": "demo"}
                }"#,
            ),
        );

        let LocalAuthConfigAbsorption::Absorbed {
            base_url,
            header_rules,
            custom_path,
        } = result
        else {
            panic!("auth_config should be absorbed");
        };

        assert_eq!(base_url, "https://api.openai.example/v1?tenant=demo");
        assert_eq!(
            header_rules,
            Some(json!([
                {"action":"set","key":"x-base","value":"1"},
                {"action":"set","key":"x-account-id","value":"acc-1"}
            ]))
        );
        assert_eq!(custom_path, None);
    }

    #[test]
    fn absorbs_path_override_and_query_aliases() {
        let result = absorb_local_auth_config_safe_subset(
            "https://generativelanguage.googleapis.com/v1beta",
            None,
            Some("/v1beta/models/original:generateContent".to_string()),
            Some(
                r#"{
                    "extra_headers": {"x-tenant": "demo"},
                    "query_params": {"alt": "sse"},
                    "custom_path": "/v1beta/models/gemini-2.5-pro:streamGenerateContent"
                }"#,
            ),
        );

        let LocalAuthConfigAbsorption::Absorbed {
            base_url,
            header_rules,
            custom_path,
        } = result
        else {
            panic!("auth_config should be absorbed");
        };

        assert_eq!(
            base_url,
            "https://generativelanguage.googleapis.com/v1beta?alt=sse"
        );
        assert_eq!(
            header_rules,
            Some(json!([{"action":"set","key":"x-tenant","value":"demo"}]))
        );
        assert_eq!(
            custom_path.as_deref(),
            Some("/v1beta/models/gemini-2.5-pro:streamGenerateContent")
        );
    }

    #[test]
    fn rejects_unknown_keys_and_reserved_headers() {
        assert_eq!(
            absorb_local_auth_config_safe_subset(
                "https://api.openai.example/v1",
                None,
                None,
                Some(r#"{"provider_type":"custom"}"#),
            ),
            LocalAuthConfigAbsorption::Unsupported
        );
        assert_eq!(
            absorb_local_auth_config_safe_subset(
                "https://api.openai.example/v1",
                None,
                None,
                Some(r#"{"headers":{"host":"api.example.test"}}"#),
            ),
            LocalAuthConfigAbsorption::Unsupported
        );
        assert_eq!(
            absorb_local_auth_config_safe_subset(
                "https://api.openai.example/v1",
                None,
                None,
                Some(r#"{"query":{"key":"secret"}}"#),
            ),
            LocalAuthConfigAbsorption::Unsupported
        );
    }

    #[test]
    fn applies_header_overrides_even_when_auth_config_has_refresh_token() {
        let mut headers = std::collections::BTreeMap::from([
            (
                "authorization".to_string(),
                "Bearer direct-token".to_string(),
            ),
            ("content-type".to_string(), "application/json".to_string()),
        ]);

        apply_local_auth_config_header_overrides(
            &mut headers,
            Some(
                r#"{
                    "refresh_token": "rt-1",
                    "headers": {
                        "authorization": "Bearer imported-session",
                        "content-type": "text/plain",
                        "host": "blocked.example"
                    }
                }"#,
            ),
        );

        assert_eq!(
            headers.get("authorization"),
            Some(&"Bearer imported-session".to_string())
        );
        assert_eq!(headers.get("content-type"), Some(&"text/plain".to_string()));
        assert!(!headers.contains_key("host"));
    }

    #[test]
    fn ignores_invalid_auth_config_header_values_when_applying_overrides() {
        let mut headers = std::collections::BTreeMap::new();

        apply_local_auth_config_header_overrides(
            &mut headers,
            Some(r#"{"headers":{"authorization":"Bearer ok","x-bad":"line\nbreak"}}"#),
        );

        assert_eq!(headers.get("authorization"), Some(&"Bearer ok".to_string()));
        assert!(!headers.contains_key("x-bad"));
    }

    #[test]
    fn keeps_imported_authorization_headers_for_runtime_override() {
        assert_eq!(
            absorb_local_auth_config_safe_subset(
                "https://api.openai.example/v1",
                None,
                None,
                Some(
                    r#"{
                        "provider_type": "codex",
                        "access_token_import_temporary": true,
                        "headers": {
                            "authorization": "Bearer imported-session",
                            "chatgpt-account-id": "acct-1"
                        }
                    }"#,
                ),
            ),
            LocalAuthConfigAbsorption::Unsupported
        );

        let mut headers = std::collections::BTreeMap::from([(
            "authorization".to_string(),
            "Bearer direct-token".to_string(),
        )]);
        apply_local_auth_config_header_overrides(
            &mut headers,
            Some(
                r#"{
                    "provider_type": "codex",
                    "access_token_import_temporary": true,
                    "headers": {
                        "authorization": "Bearer imported-session",
                        "chatgpt-account-id": "acct-1"
                    }
                }"#,
            ),
        );

        assert_eq!(
            headers.get("authorization"),
            Some(&"Bearer imported-session".to_string())
        );
        assert_eq!(
            headers.get("chatgpt-account-id"),
            Some(&"acct-1".to_string())
        );
    }

    #[test]
    fn absorbs_query_only_configs_into_base_url_for_dynamic_path_formats() {
        let result = absorb_local_auth_config_safe_subset(
            "https://generativelanguage.googleapis.com/v1beta",
            None,
            None,
            Some(r#"{"query":{"alt":"sse"}}"#),
        );
        let LocalAuthConfigAbsorption::Absorbed {
            base_url,
            header_rules,
            custom_path,
        } = result
        else {
            panic!("query-only auth_config should be absorbed");
        };

        assert_eq!(
            base_url,
            "https://generativelanguage.googleapis.com/v1beta?alt=sse"
        );
        assert_eq!(header_rules, None);
        assert_eq!(custom_path, None);
    }

    #[test]
    fn absorbs_camel_case_transport_keys_with_ignorable_metadata() {
        let result = absorb_local_auth_config_safe_subset(
            "https://api.openai.example/v1",
            None,
            None,
            Some(
                r#"{
                    "email": "user@example.com",
                    "plan_type": "plus",
                    "request": {
                        "extraHeaders": {"x-org-id": "org-1"},
                        "queryParams": {"tenant": "demo", "retry": 2, "stream": true},
                        "customPath": "/v1/responses"
                    }
                }"#,
            ),
        );
        let LocalAuthConfigAbsorption::Absorbed {
            base_url,
            header_rules,
            custom_path,
        } = result
        else {
            panic!("camelCase auth_config should be absorbed");
        };

        assert_eq!(
            base_url,
            "https://api.openai.example/v1?retry=2&stream=true&tenant=demo"
        );
        assert_eq!(
            header_rules,
            Some(json!([{"action":"set","key":"x-org-id","value":"org-1"}]))
        );
        assert_eq!(custom_path.as_deref(), Some("/v1/responses"));
    }

    #[test]
    fn rejects_sensitive_oauth_fields_even_with_transport_subset() {
        assert_eq!(
            absorb_local_auth_config_safe_subset(
                "https://api.openai.example/v1",
                None,
                None,
                Some(
                    r#"{
                        "headers": {"x-org-id": "org-1"},
                        "refresh_token": "rt-1"
                    }"#,
                ),
            ),
            LocalAuthConfigAbsorption::Unsupported
        );
        assert_eq!(
            absorb_local_auth_config_safe_subset(
                "https://api.openai.example/v1",
                None,
                None,
                Some(
                    r#"{
                        "query": {"tenant": "demo"},
                        "access_token": "at-1"
                    }"#,
                ),
            ),
            LocalAuthConfigAbsorption::Unsupported
        );
    }

    #[test]
    fn rejects_metadata_only_auth_config_without_transport_subset() {
        assert_eq!(
            absorb_local_auth_config_safe_subset(
                "https://api.openai.example/v1",
                None,
                None,
                Some(
                    r#"{
                        "email": "user@example.com",
                        "plan_type": "plus",
                        "workspace_name": "demo"
                    }"#,
                ),
            ),
            LocalAuthConfigAbsorption::Unsupported
        );
    }
}
