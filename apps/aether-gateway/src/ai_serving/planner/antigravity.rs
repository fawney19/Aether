use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::ai_serving::transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestAuth, AntigravityRequestAuthUnsupportedReason,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
    AntigravityRequestSideUnsupportedReason,
};
use crate::ai_serving::transport::{
    build_standard_provider_request_headers, GatewayProviderTransportSnapshot,
    StandardProviderRequestHeaders, StandardProviderRequestHeadersInput,
};
use crate::AppState;

pub(crate) const ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME: &str = "antigravity:v1internal";

pub(crate) enum AntigravityV1InternalRequestError {
    TransportUnsupported,
    EnvelopeUnsupported,
    UpstreamUrlUnavailable,
    HeaderRulesApplyFailed,
}

pub(crate) struct AntigravityV1InternalRequestInput<'a> {
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
    pub(crate) same_format: bool,
}

pub(crate) struct AntigravityV1InternalRequest {
    pub(crate) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(crate) body: Value,
    pub(crate) headers: StandardProviderRequestHeaders,
    pub(crate) upstream_url: String,
}

pub(crate) async fn build_antigravity_v1internal_provider_request(
    input: AntigravityV1InternalRequestInput<'_>,
) -> Result<AntigravityV1InternalRequest, AntigravityV1InternalRequestError> {
    let payload = build_antigravity_v1internal_payload(
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
    .ok_or(AntigravityV1InternalRequestError::UpstreamUrlUnavailable)?;

    let extra_headers: BTreeMap<String, String> =
        build_antigravity_static_identity_headers(&payload.auth);
    let mut headers =
        build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
            transport: &payload.transport,
            provider_api_format: input.provider_api_format,
            same_format: input.same_format,
            headers: input.request_headers,
            auth_header: input.auth_header,
            auth_value: input.auth_value,
            extra_headers: &extra_headers,
            header_rules: payload.transport.endpoint.header_rules.as_ref(),
            provider_request_body: &payload.body,
            original_request_body: input.original_request_body,
            upstream_is_stream: input.upstream_is_stream,
        })
        .ok_or(AntigravityV1InternalRequestError::HeaderRulesApplyFailed)?;
    headers
        .headers
        .insert("accept".to_string(), "text/event-stream".to_string());

    Ok(AntigravityV1InternalRequest {
        transport: payload.transport,
        body: payload.body,
        headers,
        upstream_url,
    })
}

struct AntigravityV1InternalPayload {
    transport: Arc<GatewayProviderTransportSnapshot>,
    auth: AntigravityRequestAuth,
    body: Value,
}

async fn build_antigravity_v1internal_payload(
    state: &AppState,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    trace_id: &str,
    mapped_model: &str,
    gemini_request_body: &Value,
) -> Result<AntigravityV1InternalPayload, AntigravityV1InternalRequestError> {
    let mut resolved_transport = Arc::clone(transport);
    let mut antigravity_support = classify_local_antigravity_request_support(
        &resolved_transport,
        gemini_request_body,
        AntigravityEnvelopeRequestType::Agent,
    );

    if matches!(
        antigravity_support,
        AntigravityRequestSideSupport::Unsupported(
            AntigravityRequestSideUnsupportedReason::UnsupportedAuth(
                AntigravityRequestAuthUnsupportedReason::MissingProjectId
            )
        )
    ) {
        if let Some(hydrated) = state
            .hydrate_antigravity_project_metadata_for_transport(&resolved_transport)
            .await
        {
            resolved_transport = Arc::new(hydrated);
            antigravity_support = classify_local_antigravity_request_support(
                &resolved_transport,
                gemini_request_body,
                AntigravityEnvelopeRequestType::Agent,
            );
        }
    }

    let auth = match antigravity_support {
        AntigravityRequestSideSupport::Supported(spec) => spec.auth,
        AntigravityRequestSideSupport::Unsupported(_) => {
            return Err(AntigravityV1InternalRequestError::TransportUnsupported);
        }
    };

    let body = match build_antigravity_safe_v1internal_request(
        &auth,
        trace_id,
        mapped_model,
        gemini_request_body,
        AntigravityEnvelopeRequestType::Agent,
    ) {
        AntigravityRequestEnvelopeSupport::Supported(envelope) => envelope,
        AntigravityRequestEnvelopeSupport::Unsupported(_) => {
            return Err(AntigravityV1InternalRequestError::EnvelopeUnsupported);
        }
    };

    Ok(AntigravityV1InternalPayload {
        transport: resolved_transport,
        auth,
        body,
    })
}
