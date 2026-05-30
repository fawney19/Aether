use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::ai_serving::transport::{
    build_gemini_cli_v1internal_request, build_standard_provider_request_headers,
    GatewayProviderTransportSnapshot, GeminiCliRequestAuth, GeminiCliRequestAuthSupport,
    GeminiCliRequestEnvelopeSupport, StandardProviderRequestHeaders,
    StandardProviderRequestHeadersInput, GEMINI_CLI_USER_AGENT,
};
use crate::AppState;

pub(crate) enum GeminiCliV1InternalRequestError {
    ProjectUnavailable,
    EnvelopeUnsupported,
    UpstreamUrlUnavailable,
    HeaderRulesApplyFailed,
}

pub(crate) struct GeminiCliV1InternalRequestInput<'a> {
    pub(crate) state: &'a AppState,
    pub(crate) parts: &'a http::request::Parts,
    pub(crate) transport: &'a Arc<GatewayProviderTransportSnapshot>,
    pub(crate) trace_id: &'a str,
    pub(crate) mapped_model: &'a str,
    pub(crate) provider_api_format: &'a str,
    pub(crate) auth_header: &'a str,
    pub(crate) auth_value: &'a str,
    pub(crate) request_headers: &'a http::HeaderMap,
    pub(crate) original_request_body: &'a Value,
    pub(crate) gemini_request_body: &'a Value,
    pub(crate) upstream_is_stream: bool,
}

pub(crate) struct GeminiCliV1InternalRequest {
    pub(crate) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(crate) body: Value,
    pub(crate) headers: StandardProviderRequestHeaders,
    pub(crate) upstream_url: String,
}

pub(crate) async fn build_gemini_cli_v1internal_provider_request(
    input: GeminiCliV1InternalRequestInput<'_>,
) -> Result<GeminiCliV1InternalRequest, GeminiCliV1InternalRequestError> {
    let payload = build_gemini_cli_v1internal_payload(
        input.state,
        input.transport,
        input.trace_id,
        input.mapped_model,
        input.gemini_request_body,
    )
    .await?;

    let upstream_url = crate::ai_serving::build_provider_transport_request_url_for_request_body(
        &payload.transport,
        input.provider_api_format,
        Some(input.mapped_model),
        input.upstream_is_stream,
        input.parts.uri.query(),
        None,
        Some(&payload.body),
    )
    .ok_or(GeminiCliV1InternalRequestError::UpstreamUrlUnavailable)?;

    let extra_headers =
        BTreeMap::from([("user-agent".to_string(), GEMINI_CLI_USER_AGENT.to_string())]);
    let headers = build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
        transport: &payload.transport,
        provider_api_format: input.provider_api_format,
        same_format: false,
        headers: input.request_headers,
        auth_header: input.auth_header,
        auth_value: input.auth_value,
        extra_headers: &extra_headers,
        header_rules: payload.transport.endpoint.header_rules.as_ref(),
        provider_request_body: &payload.body,
        original_request_body: input.original_request_body,
        upstream_is_stream: input.upstream_is_stream,
    })
    .ok_or(GeminiCliV1InternalRequestError::HeaderRulesApplyFailed)?;

    Ok(GeminiCliV1InternalRequest {
        transport: payload.transport,
        body: payload.body,
        headers,
        upstream_url,
    })
}

struct GeminiCliV1InternalPayload {
    transport: Arc<GatewayProviderTransportSnapshot>,
    body: Value,
}

async fn build_gemini_cli_v1internal_payload(
    state: &AppState,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    trace_id: &str,
    mapped_model: &str,
    gemini_request_body: &Value,
) -> Result<GeminiCliV1InternalPayload, GeminiCliV1InternalRequestError> {
    let mut resolved_transport = Arc::clone(transport);
    let mut auth = match crate::ai_serving::transport::resolve_local_gemini_cli_request_auth(
        &resolved_transport,
    ) {
        GeminiCliRequestAuthSupport::Supported(auth) => auth,
        GeminiCliRequestAuthSupport::Unsupported(_) => {
            return Err(GeminiCliV1InternalRequestError::ProjectUnavailable);
        }
    };
    if auth.project_id.is_none() {
        auth = match state
            .hydrate_gemini_cli_project_metadata_for_transport(&resolved_transport)
            .await
        {
            Some(hydrated) => {
                resolved_transport = Arc::new(hydrated);
                match crate::ai_serving::transport::resolve_local_gemini_cli_request_auth(
                    &resolved_transport,
                ) {
                    GeminiCliRequestAuthSupport::Supported(auth) => auth,
                    GeminiCliRequestAuthSupport::Unsupported(_) => GeminiCliRequestAuth::default(),
                }
            }
            None => GeminiCliRequestAuth::default(),
        };
    }
    let body = match build_gemini_cli_v1internal_request(
        &auth,
        trace_id,
        mapped_model,
        gemini_request_body,
    ) {
        GeminiCliRequestEnvelopeSupport::Supported(envelope) => envelope,
        GeminiCliRequestEnvelopeSupport::Unsupported(_) => {
            return Err(GeminiCliV1InternalRequestError::EnvelopeUnsupported);
        }
    };

    Ok(GeminiCliV1InternalPayload {
        transport: resolved_transport,
        body,
    })
}
