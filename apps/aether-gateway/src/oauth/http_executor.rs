use crate::admin_api::AdminAppState;
use crate::{AppState, GatewayError};
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTimeouts, RequestBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
};
use aether_oauth::core::OAuthError;
use aether_oauth::network::{OAuthHttpExecutor, OAuthHttpRequest, OAuthHttpResponse};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::read::{DeflateDecoder, GzDecoder};
use std::collections::BTreeMap;
use std::io::Read;

#[derive(Clone)]
pub(crate) struct GatewayOAuthHttpExecutor<'a> {
    app: AppState,
    _marker: std::marker::PhantomData<&'a AppState>,
}

impl<'a> GatewayOAuthHttpExecutor<'a> {
    pub(crate) fn new(state: AdminAppState<'a>) -> Self {
        Self {
            app: state.cloned_app(),
            _marker: std::marker::PhantomData,
        }
    }

    pub(crate) fn from_app(app: &'a AppState) -> Self {
        Self {
            app: app.clone(),
            _marker: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<'a> OAuthHttpExecutor for GatewayOAuthHttpExecutor<'a> {
    async fn execute(&self, request: OAuthHttpRequest) -> Result<OAuthHttpResponse, OAuthError> {
        let body = if let Some(json_body) = request.json_body {
            RequestBody::from_json(json_body)
        } else {
            RequestBody {
                json_body: None,
                body_bytes_b64: request.body_bytes.map(|bytes| STANDARD.encode(bytes)),
                body_ref: None,
            }
        };
        let timeouts = request.network.timeouts;
        let mut headers = request.headers;
        headers
            .entry(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string())
            .or_insert_with(|| "true".to_string());
        let plan = ExecutionPlan {
            request_id: request.request_id,
            candidate_id: None,
            provider_name: Some("oauth".to_string()),
            provider_id: String::new(),
            endpoint_id: String::new(),
            key_id: String::new(),
            method: request.method.as_str().to_string(),
            url: request.url,
            headers,
            content_type: request.content_type,
            content_encoding: None,
            body,
            stream: false,
            client_api_format: "oauth:exchange".to_string(),
            provider_api_format: "oauth:exchange".to_string(),
            model_name: Some("oauth-exchange".to_string()),
            proxy: request.network.proxy,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(timeouts.connect_ms),
                read_ms: Some(timeouts.read_ms),
                write_ms: Some(timeouts.write_ms),
                pool_ms: Some(timeouts.connect_ms),
                total_ms: Some(timeouts.total_ms),
                ..ExecutionTimeouts::default()
            }),
        };
        let result =
            crate::execution_runtime::execute_execution_runtime_sync_plan(&self.app, None, &plan)
                .await
                .map_err(gateway_error_to_oauth_error)?;
        Ok(OAuthHttpResponse {
            status_code: result.status_code,
            body_text: execution_body_text(&result),
            json_body: execution_json_body(&result),
        })
    }
}

fn execution_json_body(result: &ExecutionResult) -> Option<serde_json::Value> {
    result
        .body
        .as_ref()
        .and_then(|body| body.json_body.clone())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| execution_body_bytes(&result.headers, body))
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        })
}

fn execution_body_text(result: &ExecutionResult) -> String {
    result
        .body
        .as_ref()
        .and_then(|body| execution_body_bytes(&result.headers, body))
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(|value| serde_json::to_string(value).ok())
        })
        .unwrap_or_default()
}

fn execution_body_bytes(
    headers: &BTreeMap<String, String>,
    body: &aether_contracts::ResponseBody,
) -> Option<Vec<u8>> {
    let bytes = body
        .body_bytes_b64
        .as_deref()
        .and_then(|value| STANDARD.decode(value).ok())?;
    decode_response_bytes(&bytes, headers.get("content-encoding").map(String::as_str))
        .or(Some(bytes))
}

fn decode_response_bytes(bytes: &[u8], content_encoding: Option<&str>) -> Option<Vec<u8>> {
    match content_encoding
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("gzip") => {
            let mut decoder = GzDecoder::new(bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        Some("deflate") => {
            let mut decoder = DeflateDecoder::new(bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        _ => None,
    }
}

fn gateway_error_to_oauth_error(error: GatewayError) -> OAuthError {
    match error {
        GatewayError::UpstreamUnavailable { message, .. }
        | GatewayError::ControlUnavailable { message, .. }
        | GatewayError::Client { message, .. }
        | GatewayError::Internal(message) => OAuthError::Transport(message),
    }
}
