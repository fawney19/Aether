use axum::body::Bytes;
use axum::http::Uri;

use super::super::GatewayControlDecision;
use super::credentials::{contains_string, extract_requested_model};
use crate::{AppState, GatewayError};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GatewayLocalAuthRejection {
    InvalidApiKey,
    LockedApiKey,
    WalletUnavailable,
    BalanceDenied { remaining: Option<f64> },
    ProviderNotAllowed { provider: String },
    ApiFormatNotAllowed { api_format: String },
    ModelNotAllowed { model: String },
}

pub(crate) fn trusted_auth_local_rejection(
    decision: Option<&GatewayControlDecision>,
    _headers: &http::HeaderMap,
) -> Option<GatewayLocalAuthRejection> {
    let decision = decision?;
    if decision.route_class.as_deref() != Some("ai_public") {
        return None;
    }

    decision
        .local_auth_rejection
        .clone()
        .or_else(|| decision.auth_context.as_ref()?.local_rejection.clone())
}

pub(crate) fn should_buffer_request_for_local_auth(
    decision: Option<&GatewayControlDecision>,
    headers: &http::HeaderMap,
) -> bool {
    let Some(decision) = decision else {
        return false;
    };
    decision.route_class.as_deref() == Some("ai_public")
        && decision.route_kind.as_deref() != Some("files")
        && crate::headers::is_json_request(headers)
}

pub(crate) async fn request_model_local_rejection(
    state: &AppState,
    decision: Option<&GatewayControlDecision>,
    uri: &Uri,
    headers: &http::HeaderMap,
    body: &Bytes,
) -> Result<Option<GatewayLocalAuthRejection>, GatewayError> {
    let Some(decision) = decision else {
        return Ok(None);
    };
    if decision.route_class.as_deref() != Some("ai_public") {
        return Ok(None);
    }
    let Some(auth_context) = decision.auth_context.as_ref() else {
        return Ok(None);
    };
    let Some(allowed_models) = auth_context.allowed_models.as_deref() else {
        return Ok(None);
    };
    let Some(requested_model) = extract_requested_model(decision, uri, headers, body) else {
        return Ok(None);
    };
    if contains_string(allowed_models, &requested_model) {
        return Ok(None);
    }
    if request_model_resolves_to_allowed_model(state, decision, &requested_model, allowed_models)
        .await?
    {
        return Ok(None);
    }

    Ok(Some(GatewayLocalAuthRejection::ModelNotAllowed {
        model: requested_model,
    }))
}

async fn request_model_resolves_to_allowed_model(
    state: &AppState,
    decision: &GatewayControlDecision,
    requested_model: &str,
    allowed_models: &[String],
) -> Result<bool, GatewayError> {
    let Some(client_api_format) = decision
        .auth_endpoint_signature
        .as_deref()
        .map(crate::ai_serving::normalize_api_format_alias)
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(false);
    };

    for api_format in candidate_api_formats_for_model_resolution(&client_api_format) {
        let rows = state
            .list_minimal_candidate_selection_rows_for_api_format(&api_format)
            .await?;
        let matching_rows = rows
            .into_iter()
            .filter(|row| {
                aether_scheduler_core::row_supports_requested_model(
                    row,
                    requested_model,
                    &api_format,
                )
            })
            .collect::<Vec<_>>();
        let Some(resolved_global_model) =
            aether_scheduler_core::resolve_requested_global_model_name(
                &matching_rows,
                requested_model,
                &api_format,
            )
        else {
            continue;
        };
        if contains_string(allowed_models, &resolved_global_model) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn candidate_api_formats_for_model_resolution(client_api_format: &str) -> Vec<String> {
    let mut api_formats = Vec::new();
    push_unique_api_format(&mut api_formats, client_api_format);
    for api_format in crate::ai_serving::request_candidate_api_formats(client_api_format, false) {
        push_unique_api_format(&mut api_formats, api_format);
    }
    api_formats
}

fn push_unique_api_format(api_formats: &mut Vec<String>, api_format: &str) {
    let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
    if api_format.is_empty() || api_formats.iter().any(|value| value == &api_format) {
        return;
    }
    api_formats.push(api_format);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data_contracts::repository::candidate_selection::{
        StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
    };
    use axum::body::Bytes;
    use axum::http::{HeaderMap, Uri};

    use super::{request_model_local_rejection, GatewayLocalAuthRejection};
    use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
    use crate::data::GatewayDataState;
    use crate::AppState;

    fn sample_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-1".to_string(),
            provider_name: "Provider 1".to_string(),
            provider_type: "openai".to_string(),
            provider_priority: 0,
            provider_is_active: true,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: "key-1".to_string(),
            key_name: "key".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 0,
            key_global_priority_by_format: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: Some(vec!["gpt-5(?:\\.\\d+)?".to_string()]),
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_row_for_api_format(api_format: &str) -> StoredMinimalCandidateSelectionRow {
        let mut row = sample_row();
        let api_family = api_format
            .split_once(':')
            .map(|(family, _)| family)
            .unwrap_or(api_format);
        row.provider_id = format!("provider-{api_family}");
        row.provider_name = format!("Provider {api_family}");
        row.provider_type = api_family.to_string();
        row.endpoint_id = format!("endpoint-{api_family}");
        row.endpoint_api_format = api_format.to_string();
        row.endpoint_api_family = Some(api_family.to_string());
        row.key_id = format!("key-{api_family}");
        row.key_api_formats = Some(vec![api_format.to_string()]);
        if let Some(mappings) = row.model_provider_model_mappings.as_mut() {
            for mapping in mappings {
                mapping.api_formats = Some(vec![api_format.to_string()]);
            }
        }
        row
    }

    fn decision_with_allowed_models(allowed_models: Vec<String>) -> GatewayControlDecision {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            local_rejection: None,
            allowed_models: Some(allowed_models),
        });
        decision
    }

    fn state_with_rows(rows: Vec<StoredMinimalCandidateSelectionRow>) -> AppState {
        let repository = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows));
        let data = GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository);
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data)
    }

    fn state_with_model_mapping() -> AppState {
        state_with_rows(vec![sample_row()])
    }

    fn json_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json"
                .parse()
                .expect("content type should parse"),
        );
        headers
    }

    #[tokio::test]
    async fn model_rejection_allows_requested_model_that_resolves_to_allowed_global_model() {
        let state = state_with_model_mapping();
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gpt-5.2","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_rejection_allows_cross_format_provider_mapping_to_allowed_global_model() {
        let mut row = sample_row_for_api_format("gemini:generate_content");
        row.model_provider_model_name = "gemini-2.5-pro-upstream".to_string();
        row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
            name: "gemini-2.5-pro-alias".to_string(),
            priority: 1,
            api_formats: Some(vec!["gemini:generate_content".to_string()]),
        }]);
        let state = state_with_rows(vec![row]);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gemini-2.5-pro-alias","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_rejection_allows_cross_format_regex_mapping_to_allowed_global_model() {
        let state = state_with_rows(vec![sample_row_for_api_format("claude:messages")]);
        let decision = decision_with_allowed_models(vec!["gpt-5".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gpt-5.2","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(rejection, None);
    }

    #[tokio::test]
    async fn model_rejection_denies_requested_model_outside_allowed_global_models() {
        let state = state_with_model_mapping();
        let decision = decision_with_allowed_models(vec!["gpt-4.1".to_string()]);
        let uri: Uri = "/v1/chat/completions".parse().expect("uri should parse");
        let body = Bytes::from_static(br#"{"model":"gpt-5.2","messages":[]}"#);

        let rejection =
            request_model_local_rejection(&state, Some(&decision), &uri, &json_headers(), &body)
                .await
                .expect("model rejection should resolve");

        assert_eq!(
            rejection,
            Some(GatewayLocalAuthRejection::ModelNotAllowed {
                model: "gpt-5.2".to_string(),
            })
        );
    }
}
