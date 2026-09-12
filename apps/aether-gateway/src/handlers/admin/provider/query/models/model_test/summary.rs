use std::collections::{BTreeMap, BTreeSet};

use super::super::provider_query_key_display_name;
use super::{ProviderQueryExecutionOutcome, ProviderQueryTestCandidate};
use aether_admin::provider::redaction::admin_secret_safe_url;
use serde_json::{json, Value};

pub(super) fn provider_query_test_attempt_payload(
    candidate_index: usize,
    candidate: &ProviderQueryTestCandidate,
    execution: &ProviderQueryExecutionOutcome,
) -> Value {
    let endpoint_route = provider_query_endpoint_route_payload(candidate, execution);
    let response_body = provider_query_success_response_body(execution);
    let endpoint_product = endpoint_route
        .get("product")
        .cloned()
        .unwrap_or(Value::Null);
    let endpoint_variant = endpoint_route
        .get("variant")
        .cloned()
        .unwrap_or(Value::Null);
    let endpoint_action = endpoint_route.get("action").cloned().unwrap_or(Value::Null);
    let endpoint_batch_strategy = endpoint_route
        .get("batch_strategy")
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "candidate_index": candidate_index,
        "retry_index": 0,
        "endpoint_api_format": candidate.endpoint.api_format,
        "endpoint_base_url": admin_secret_safe_url(Some(&candidate.endpoint.base_url)),
        "endpoint_product": endpoint_product,
        "endpoint_variant": endpoint_variant,
        "endpoint_action": endpoint_action,
        "endpoint_batch_strategy": endpoint_batch_strategy,
        "key_name": provider_query_key_display_name(&candidate.key),
        "key_id": candidate.key.id,
        "auth_type": candidate.key.auth_type,
        "effective_model": candidate.effective_model,
        "status": execution.status,
        "skip_reason": execution.skip_reason,
        "error_message": provider_query_error_projection(execution),
        "status_code": execution.status_code,
        "latency_ms": execution.latency_ms,
        "request_url": admin_secret_safe_url(Some(&execution.request_url)),
        "request_headers": redacted_provider_query_headers(&execution.request_headers),
        "request_body": redacted_provider_query_value(&execution.request_body),
        "response_headers": redacted_provider_query_headers(&execution.response_headers),
        "response_body": response_body,
    })
}

pub(super) fn provider_query_error_projection(
    execution: &ProviderQueryExecutionOutcome,
) -> Option<String> {
    execution.error_message.as_ref().map(|_| {
        execution
            .status_code
            .map(|status| format!("HTTP {status}"))
            .unwrap_or_else(|| "Provider request failed".to_string())
    })
}

fn provider_query_endpoint_route_payload(
    candidate: &ProviderQueryTestCandidate,
    execution: &ProviderQueryExecutionOutcome,
) -> Value {
    let api_format = crate::ai_serving::normalize_api_format_alias(&candidate.endpoint.api_format);
    let request_url = execution.request_url.to_ascii_lowercase();
    let base_url = candidate.endpoint.base_url.to_ascii_lowercase();
    let is_vertex = request_url.contains("aiplatform.googleapis.com")
        || base_url.contains("aiplatform.googleapis.com");
    let is_gemini_api = request_url.contains("generativelanguage.googleapis.com")
        || base_url.contains("generativelanguage.googleapis.com");
    let is_openai_compat =
        request_url.contains("/endpoints/openapi") || request_url.contains("/openai/");
    let is_batch = execution
        .request_body
        .get("requests")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    let vertex_instance_count = execution
        .request_body
        .get("instances")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    let (product, variant, action, batch_strategy) = match api_format.as_str() {
        "gemini:embedding" if is_vertex => (
            "Vertex AI",
            "vertex_native",
            "predict",
            if vertex_instance_count > 1 {
                "predict_instances"
            } else {
                "single_instance"
            },
        ),
        "gemini:embedding" if is_gemini_api => (
            "Gemini API",
            "gemini_native",
            if is_batch {
                "batchEmbedContents"
            } else {
                "embedContent"
            },
            if is_batch {
                "native_batch"
            } else {
                "single_native"
            },
        ),
        "gemini:embedding" => (
            "Gemini native",
            "gemini_native",
            if is_batch {
                "batchEmbedContents"
            } else {
                "embedContent"
            },
            if is_batch {
                "native_batch"
            } else {
                "single_native"
            },
        ),
        "gemini:generate_content" if is_vertex => {
            ("Vertex AI", "vertex_native", "generateContent", "")
        }
        "gemini:generate_content" if is_gemini_api => {
            ("Gemini API", "gemini_native", "generateContent", "")
        }
        "gemini:generate_content" => ("Gemini native", "gemini_native", "generateContent", ""),
        "gemini:interactions" if is_gemini_api => {
            ("Gemini API", "gemini_native", "interactions", "")
        }
        "gemini:interactions" => ("Gemini native", "gemini_native", "interactions", ""),
        "openai:embedding" if is_vertex && is_openai_compat => (
            "Vertex AI OpenAI-compatible",
            "openai_compatible",
            "embeddings",
            "openai_batch",
        ),
        "openai:embedding" if is_gemini_api && is_openai_compat => (
            "Gemini API OpenAI-compatible",
            "openai_compatible",
            "embeddings",
            "openai_batch",
        ),
        "openai:embedding" => (
            "OpenAI-compatible",
            "openai_compatible",
            "embeddings",
            "openai_batch",
        ),
        "aliyun:multimodal_embedding" => (
            "Aliyun DashScope",
            "dashscope_native",
            "multimodal-embedding",
            "dashscope_contents",
        ),
        "openai:chat" if is_vertex && is_openai_compat => (
            "Vertex AI OpenAI-compatible",
            "openai_compatible",
            "chat/completions",
            "",
        ),
        "openai:chat" if is_gemini_api && is_openai_compat => (
            "Gemini API OpenAI-compatible",
            "openai_compatible",
            "chat/completions",
            "",
        ),
        "openai:chat" => (
            "OpenAI-compatible",
            "openai_compatible",
            "chat/completions",
            "",
        ),
        _ => (
            "Provider endpoint",
            "provider_native",
            "provider_request",
            "",
        ),
    };

    json!({
        "product": product,
        "variant": variant,
        "action": action,
        "batch_strategy": batch_strategy,
    })
}

fn redacted_provider_query_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(key, value)| {
            if provider_query_field_is_sensitive(key) {
                (key.clone(), "[REDACTED]".to_string())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect()
}

pub(super) fn redacted_provider_query_value(value: &Value) -> Value {
    redacted_provider_query_value_with_sensitive_values(value, &BTreeSet::new())
}

pub(super) fn provider_query_success_response_body(
    execution: &ProviderQueryExecutionOutcome,
) -> Option<Value> {
    if execution.status != "success" {
        return None;
    }

    let sensitive_values = provider_query_execution_sensitive_values(execution);
    execution
        .response_body
        .as_ref()
        .map(|value| redacted_provider_query_value_with_sensitive_values(value, &sensitive_values))
}

fn redacted_provider_query_value_with_sensitive_values(
    value: &Value,
    sensitive_values: &BTreeSet<String>,
) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    if provider_query_field_is_sensitive(key) {
                        (key.clone(), Value::String("[REDACTED]".to_string()))
                    } else {
                        (
                            key.clone(),
                            redacted_provider_query_value_with_sensitive_values(
                                value,
                                sensitive_values,
                            ),
                        )
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|value| {
                    redacted_provider_query_value_with_sensitive_values(value, sensitive_values)
                })
                .collect::<Vec<_>>(),
        ),
        Value::String(value)
            if provider_query_string_contains_sensitive_material(value, sensitive_values) =>
        {
            Value::String("[REDACTED]".to_string())
        }
        other => other.clone(),
    }
}

fn provider_query_execution_sensitive_values(
    execution: &ProviderQueryExecutionOutcome,
) -> BTreeSet<String> {
    let mut sensitive_values = BTreeSet::new();
    for (key, value) in &execution.request_headers {
        if provider_query_field_is_sensitive(key) {
            provider_query_insert_sensitive_value(&mut sensitive_values, value);
        }
    }
    provider_query_collect_sensitive_field_values(&execution.request_body, &mut sensitive_values);
    provider_query_collect_sensitive_url_values(&execution.request_url, &mut sensitive_values);
    sensitive_values
}

fn provider_query_collect_sensitive_field_values(
    value: &Value,
    sensitive_values: &mut BTreeSet<String>,
) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if provider_query_field_is_sensitive(key) {
                    provider_query_collect_string_values(value, sensitive_values);
                } else {
                    provider_query_collect_sensitive_field_values(value, sensitive_values);
                }
            }
        }
        Value::Array(items) => {
            for value in items {
                provider_query_collect_sensitive_field_values(value, sensitive_values);
            }
        }
        _ => {}
    }
}

fn provider_query_collect_string_values(value: &Value, sensitive_values: &mut BTreeSet<String>) {
    match value {
        Value::String(value) => provider_query_insert_sensitive_value(sensitive_values, value),
        Value::Object(object) => {
            for value in object.values() {
                provider_query_collect_string_values(value, sensitive_values);
            }
        }
        Value::Array(items) => {
            for value in items {
                provider_query_collect_string_values(value, sensitive_values);
            }
        }
        _ => {}
    }
}

fn provider_query_collect_sensitive_url_values(
    value: &str,
    sensitive_values: &mut BTreeSet<String>,
) {
    let Ok(parsed) = url::Url::parse(value) else {
        return;
    };
    if !parsed.username().is_empty() {
        provider_query_insert_sensitive_value(sensitive_values, parsed.username());
    }
    if let Some(password) = parsed.password() {
        provider_query_insert_sensitive_value(sensitive_values, password);
    }
    for (key, value) in parsed.query_pairs() {
        if provider_query_url_query_field_is_sensitive(&key) {
            provider_query_insert_sensitive_value(sensitive_values, &value);
        }
    }
}

fn provider_query_insert_sensitive_value(sensitive_values: &mut BTreeSet<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() || value == "[REDACTED]" {
        return;
    }
    sensitive_values.insert(value.to_string());

    let lower = value.to_ascii_lowercase();
    for scheme in ["bearer ", "basic ", "token "] {
        if lower.starts_with(scheme) {
            let credential = value[scheme.len()..].trim();
            if !credential.is_empty() {
                sensitive_values.insert(credential.to_string());
            }
        }
    }
}

fn provider_query_string_contains_sensitive_material(
    value: &str,
    sensitive_values: &BTreeSet<String>,
) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if sensitive_values
        .iter()
        .any(|secret| value == secret || (secret.len() >= 8 && value.contains(secret.as_str())))
    {
        return true;
    }

    let lower = value.to_ascii_lowercase();
    if provider_query_contains_credential_scheme(&lower, "bearer")
        || provider_query_contains_credential_scheme(&lower, "basic")
    {
        return true;
    }

    [
        "authorization",
        "proxy-authorization",
        "api_key",
        "api-key",
        "api key",
        "apikey",
        "x-api-key",
        "x-goog-api-key",
        "access_token",
        "access token",
        "refresh_token",
        "refresh token",
        "id_token",
        "id token",
        "client_secret",
        "client secret",
        "password",
        "passwd",
        "secret",
    ]
    .iter()
    .any(|marker| provider_query_contains_secret_assignment(&lower, marker))
        || provider_query_contains_known_token_prefix(&lower)
}

fn provider_query_contains_credential_scheme(value: &str, scheme: &str) -> bool {
    value.match_indices(scheme).any(|(index, _)| {
        let before_is_boundary =
            index == 0 || !value.as_bytes()[index.saturating_sub(1)].is_ascii_alphanumeric();
        if !before_is_boundary {
            return false;
        }
        let credential = value[index + scheme.len()..].trim_start();
        let credential = credential
            .strip_prefix(':')
            .or_else(|| credential.strip_prefix('='))
            .unwrap_or(credential)
            .trim_start();
        credential
            .split(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ';'))
            .next()
            .is_some_and(|token| token.len() >= 6)
    })
}

fn provider_query_contains_secret_assignment(value: &str, marker: &str) -> bool {
    value.match_indices(marker).any(|(index, _)| {
        let before_is_boundary =
            index == 0 || !value.as_bytes()[index.saturating_sub(1)].is_ascii_alphanumeric();
        if !before_is_boundary {
            return false;
        }
        let remainder = value[index + marker.len()..]
            .trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\''));
        let Some(remainder) = remainder
            .strip_prefix(':')
            .or_else(|| remainder.strip_prefix('='))
        else {
            return false;
        };
        let assigned =
            remainder.trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\''));
        !assigned.is_empty()
            && !assigned.starts_with("null")
            && !assigned.starts_with("none")
            && !assigned.starts_with("[redacted]")
    })
}

fn provider_query_contains_known_token_prefix(value: &str) -> bool {
    [
        "sk-",
        "sk_",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "aiza",
    ]
    .iter()
    .any(|prefix| {
        value.match_indices(*prefix).any(|(index, _)| {
            value[index..]
                .split(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ';'))
                .next()
                .is_some_and(|token| token.len() >= 12)
        })
    })
}

fn provider_query_url_query_field_is_sensitive(key: &str) -> bool {
    if provider_query_field_is_sensitive(key) {
        return true;
    }
    matches!(
        provider_query_normalized_field_key(key).as_str(),
        "key" | "credential" | "signature" | "sig"
    )
}

fn provider_query_field_is_sensitive(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    let normalized = provider_query_normalized_field_key(&key);
    if matches!(
        normalized.as_str(),
        "maxtokens"
            | "maxoutputtokens"
            | "inputtokens"
            | "outputtokens"
            | "prompttokens"
            | "completiontokens"
            | "totaltokens"
    ) {
        return false;
    }
    matches!(
        key.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "api_key"
            | "apikey"
            | "api-key"
            | "x-api-key"
            | "x-goog-api-key"
            | "anthropic-api-key"
            | "openai-api-key"
            | "x-codeium-csrf-token"
            | "access_token"
            | "refresh_token"
            | "id_token"
            | "password"
            | "secret"
    ) || normalized.ends_with("token")
        || normalized.contains("secret")
        || normalized.contains("apikey")
        || normalized.contains("authorization")
}

fn provider_query_normalized_field_key(key: &str) -> String {
    key.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

pub(super) fn provider_query_candidate_summary_payload(
    total_candidates: usize,
    total_attempts: usize,
    attempts: &[Value],
) -> Value {
    let success_count = attempts
        .iter()
        .filter(|attempt| attempt.get("status").and_then(Value::as_str) == Some("success"))
        .count();
    let failed_count = attempts
        .iter()
        .filter(|attempt| {
            matches!(
                attempt.get("status").and_then(Value::as_str),
                Some("failed") | Some("cancelled") | Some("stream_interrupted")
            )
        })
        .count();
    let skipped_count = attempts
        .iter()
        .filter(|attempt| attempt.get("status").and_then(Value::as_str) == Some("skipped"))
        .count();
    let pending_count = attempts
        .iter()
        .filter(|attempt| {
            matches!(
                attempt.get("status").and_then(Value::as_str),
                Some("pending") | Some("streaming")
            )
        })
        .count();
    let available_count = attempts
        .iter()
        .filter(|attempt| attempt.get("status").and_then(Value::as_str) == Some("available"))
        .count();
    let unused_count = if success_count > 0 {
        total_candidates.saturating_sub(success_count + failed_count + skipped_count)
    } else {
        0
    };
    let stop_reason = if total_candidates == 0 {
        "no_candidate"
    } else if success_count > 0 {
        "first_success"
    } else if total_attempts == 0 && skipped_count > 0 {
        "all_skipped"
    } else if failed_count > 0 || skipped_count > 0 {
        "exhausted"
    } else {
        "pending"
    };
    let winning_attempt = attempts
        .iter()
        .find(|attempt| attempt.get("status").and_then(Value::as_str) == Some("success"));

    json!({
        "total_candidates": total_candidates,
        "attempted": total_attempts,
        "success": success_count,
        "failed": failed_count,
        "skipped": skipped_count,
        "unused": unused_count,
        "pending": pending_count,
        "available": available_count,
        "completed": success_count + failed_count + skipped_count + unused_count,
        "stop_reason": stop_reason,
        "winning_candidate_index": winning_attempt
            .and_then(|attempt| attempt.get("candidate_index"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_key_name": winning_attempt
            .and_then(|attempt| attempt.get("key_name"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_key_id": winning_attempt
            .and_then(|attempt| attempt.get("key_id"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_auth_type": winning_attempt
            .and_then(|attempt| attempt.get("auth_type"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_effective_model": winning_attempt
            .and_then(|attempt| attempt.get("effective_model"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_endpoint_api_format": winning_attempt
            .and_then(|attempt| attempt.get("endpoint_api_format"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_endpoint_base_url": winning_attempt
            .and_then(|attempt| attempt.get("endpoint_base_url"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_latency_ms": winning_attempt
            .and_then(|attempt| attempt.get("latency_ms"))
            .cloned()
            .unwrap_or(Value::Null),
        "winning_status_code": winning_attempt
            .and_then(|attempt| attempt.get("status_code"))
            .cloned()
            .unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        provider_query_test_attempt_payload, redacted_provider_query_headers,
        redacted_provider_query_value, ProviderQueryExecutionOutcome, ProviderQueryTestCandidate,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn attempt_payload_strips_credentials_and_queries_from_urls() {
        let mut endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint");
        endpoint.base_url =
            "https://base-user:base-password@api.example.test/v1?base-secret=1#fragment"
                .to_string();
        let candidate = ProviderQueryTestCandidate {
            endpoint,
            key: StoredProviderCatalogKey::new(
                "key-1".to_string(),
                "provider-1".to_string(),
                "key".to_string(),
                "api_key".to_string(),
                None,
                true,
            )
            .expect("key"),
            effective_model: "gpt-test".to_string(),
            scheduler_skip_reason: None,
        };
        let execution = ProviderQueryExecutionOutcome {
            status: "success",
            skip_reason: None,
            error_message: None,
            status_code: Some(200),
            latency_ms: Some(1),
            request_url:
                "https://request-user:request-password@api.example.test/v1/chat?key=request-secret#fragment"
                    .to_string(),
            request_headers: BTreeMap::new(),
            request_body: json!({}),
            response_headers: BTreeMap::new(),
            response_body: None,
        };

        let payload = provider_query_test_attempt_payload(0, &candidate, &execution);
        assert_eq!(payload["endpoint_base_url"], "https://api.example.test/v1");
        assert_eq!(payload["request_url"], "https://api.example.test/v1/chat");
        let serialized = payload.to_string();
        for secret in [
            "base-user",
            "base-password",
            "base-secret",
            "request-user",
            "request-password",
            "request-secret",
            "fragment",
        ] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }
    }

    #[test]
    fn attempt_payload_does_not_reflect_upstream_error_text_or_response_secrets() {
        let candidate = ProviderQueryTestCandidate {
            endpoint: StoredProviderCatalogEndpoint::new(
                "endpoint-1".to_string(),
                "provider-1".to_string(),
                "openai:chat".to_string(),
                Some("openai".to_string()),
                Some("chat".to_string()),
                true,
            )
            .expect("endpoint"),
            key: StoredProviderCatalogKey::new(
                "key-1".to_string(),
                "provider-1".to_string(),
                "key".to_string(),
                "api_key".to_string(),
                None,
                true,
            )
            .expect("key"),
            effective_model: "gpt-test".to_string(),
            scheduler_skip_reason: None,
        };
        let mut execution = ProviderQueryExecutionOutcome {
            status: "failed",
            skip_reason: None,
            error_message: Some(
                "authorization=Bearer upstream-secret https://user:pass@example.test?q=secret"
                    .to_string(),
            ),
            status_code: Some(502),
            latency_ms: Some(1),
            request_url: "https://api.example.test/v1/chat".to_string(),
            request_headers: BTreeMap::new(),
            request_body: json!({}),
            response_headers: BTreeMap::new(),
            response_body: Some(json!({
                "error": {"message": "authorization=Bearer response-message-secret"},
                "content": "password=response-content-secret",
                "access_token": "response-secret",
                "nested": {"password": "private-password"}
            })),
        };

        let payload = provider_query_test_attempt_payload(0, &candidate, &execution);
        assert_eq!(payload["error_message"], json!("HTTP 502"));
        assert!(payload["response_body"].is_null());
        let serialized = payload.to_string();
        for secret in [
            "upstream-secret",
            "response-message-secret",
            "response-content-secret",
            "response-secret",
            "private-password",
            "user:pass",
        ] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }

        execution.status_code = None;
        let network_failure_payload =
            provider_query_test_attempt_payload(0, &candidate, &execution);
        assert_eq!(
            network_failure_payload["error_message"],
            json!("Provider request failed")
        );
        assert!(network_failure_payload["response_body"].is_null());
    }

    #[test]
    fn successful_attempt_keeps_diagnostics_but_redacts_embedded_credentials() {
        let candidate = ProviderQueryTestCandidate {
            endpoint: StoredProviderCatalogEndpoint::new(
                "endpoint-1".to_string(),
                "provider-1".to_string(),
                "openai:chat".to_string(),
                Some("openai".to_string()),
                Some("chat".to_string()),
                true,
            )
            .expect("endpoint"),
            key: StoredProviderCatalogKey::new(
                "key-1".to_string(),
                "provider-1".to_string(),
                "key".to_string(),
                "api_key".to_string(),
                None,
                true,
            )
            .expect("key"),
            effective_model: "gpt-test".to_string(),
            scheduler_skip_reason: None,
        };
        let execution = ProviderQueryExecutionOutcome {
            status: "success",
            skip_reason: None,
            error_message: None,
            status_code: Some(200),
            latency_ms: Some(1),
            request_url: "https://api.example.test/v1/chat?key=query-credential-token-9012"
                .to_string(),
            request_headers: BTreeMap::from([(
                "authorization".to_string(),
                "Bearer request-credential-token-1234".to_string(),
            )]),
            request_body: json!({
                "metadata": {"apiKey": "body-credential-token-5678"}
            }),
            response_headers: BTreeMap::new(),
            response_body: Some(json!({
                "choices": [
                    {"message": {"content": "request-credential-token-1234"}},
                    {"message": {"content": "Normal model response"}}
                ],
                "echoed_body": "body-credential-token-5678",
                "echoed_query": "query-credential-token-9012",
                "warning": {"message": "password=upstream-private-password"},
                "notice": {"content": "authorization: Bearer upstream-auth-secret"},
                "api_key_warning": {"message": "api_key=upstream-api-key-secret"},
                "token_warning": {"content": "access_token=upstream-access-token"},
                "api_key": "direct-response-secret",
                "usage": {"input_tokens": 3, "output_tokens": 2}
            })),
        };

        let payload = provider_query_test_attempt_payload(0, &candidate, &execution);
        assert_eq!(
            payload.pointer("/response_body/choices/0/message/content"),
            Some(&json!("[REDACTED]"))
        );
        assert_eq!(
            payload.pointer("/response_body/choices/1/message/content"),
            Some(&json!("Normal model response"))
        );
        assert_eq!(
            payload.pointer("/response_body/usage/input_tokens"),
            Some(&json!(3))
        );
        assert_eq!(
            payload.pointer("/response_body/usage/output_tokens"),
            Some(&json!(2))
        );
        let serialized = payload.to_string();
        for secret in [
            "request-credential-token-1234",
            "body-credential-token-5678",
            "query-credential-token-9012",
            "upstream-private-password",
            "upstream-auth-secret",
            "upstream-api-key-secret",
            "upstream-access-token",
            "direct-response-secret",
        ] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }
    }

    #[test]
    fn redacts_sensitive_provider_query_headers() {
        let headers = BTreeMap::from([
            ("cookie".to_string(), "sso=secret".to_string()),
            (
                "authorization".to_string(),
                "Bearer secret-token".to_string(),
            ),
            ("x-goog-api-key".to_string(), "secret".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            (
                "x-codeium-csrf-token".to_string(),
                "csrf-secret".to_string(),
            ),
        ]);

        let redacted = redacted_provider_query_headers(&headers);

        assert_eq!(
            redacted.get("cookie").map(String::as_str),
            Some("[REDACTED]")
        );
        assert_eq!(
            redacted.get("authorization").map(String::as_str),
            Some("[REDACTED]")
        );
        assert_eq!(
            redacted.get("x-goog-api-key").map(String::as_str),
            Some("[REDACTED]")
        );
        assert_eq!(
            redacted.get("x-codeium-csrf-token").map(String::as_str),
            Some("[REDACTED]")
        );
        assert_eq!(
            redacted.get("content-type").map(String::as_str),
            Some("application/json")
        );
    }

    #[test]
    fn redacts_sensitive_provider_query_request_body_fields() {
        let body = json!({
            "metadata": {
                "apiKey": "devin-session-token$secret",
                "ideName": "windsurf"
            },
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true
        });

        let redacted = redacted_provider_query_value(&body);

        assert_eq!(
            redacted.pointer("/metadata/apiKey"),
            Some(&json!("[REDACTED]"))
        );
        assert_eq!(
            redacted.pointer("/metadata/ideName"),
            Some(&json!("windsurf"))
        );
        assert_eq!(redacted.pointer("/stream"), Some(&json!(true)));
    }

    #[test]
    fn keeps_non_secret_token_count_fields_visible() {
        let body = json!({
            "maxTokens": 64,
            "usage": {
                "inputTokens": 10,
                "outputTokens": 2,
                "accessToken": "secret"
            }
        });

        let redacted = redacted_provider_query_value(&body);

        assert_eq!(redacted.pointer("/maxTokens"), Some(&json!(64)));
        assert_eq!(redacted.pointer("/usage/inputTokens"), Some(&json!(10)));
        assert_eq!(redacted.pointer("/usage/outputTokens"), Some(&json!(2)));
        assert_eq!(
            redacted.pointer("/usage/accessToken"),
            Some(&json!("[REDACTED]"))
        );
    }
}
