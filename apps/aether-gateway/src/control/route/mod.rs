use axum::http::Uri;

use crate::ai_serving::{ApiOperation, ClientSurface};
use crate::headers::header_value_str;
use crate::{AppState, GatewayError};

mod admin;
mod ai;
mod internal;
mod oauth;
mod public_support;

use super::auth::{
    resolve_control_decision_auth_with_trusted_auth, resolve_gateway_credential_carrier,
    ControlDecisionAuthResolution, GatewayCredentialCarrier,
};
use super::{GatewayAdminPrincipalContext, GatewayControlAuthContext, GatewayLocalAuthRejection};

#[derive(Debug, Clone)]
pub(crate) struct GatewayControlDecision {
    pub(crate) public_path: String,
    pub(crate) public_query_string: Option<String>,
    pub(crate) route_class: Option<String>,
    pub(crate) route_family: Option<String>,
    pub(crate) route_kind: Option<String>,
    pub(crate) client_surface: Option<ClientSurface>,
    pub(crate) api_operation: Option<ApiOperation>,
    pub(crate) gateway_credential_carrier: Option<GatewayCredentialCarrier>,
    pub(crate) request_auth_channel: Option<String>,
    pub(crate) auth_endpoint_signature: Option<String>,
    pub(crate) execution_runtime_candidate: bool,
    pub(crate) auth_context: Option<GatewayControlAuthContext>,
    pub(crate) admin_principal: Option<GatewayAdminPrincipalContext>,
    pub(crate) local_auth_rejection: Option<GatewayLocalAuthRejection>,
    pub(crate) model_directive_policy: crate::system_features::ModelDirectivePolicySnapshot,
}

impl GatewayControlDecision {
    pub(crate) fn synthetic(
        public_path: impl Into<String>,
        route_class: Option<String>,
        route_family: Option<String>,
        route_kind: Option<String>,
        auth_endpoint_signature: Option<String>,
    ) -> Self {
        Self {
            public_path: public_path.into(),
            public_query_string: None,
            route_class,
            route_family,
            route_kind,
            client_surface: None,
            api_operation: None,
            gateway_credential_carrier: None,
            request_auth_channel: None,
            auth_endpoint_signature,
            execution_runtime_candidate: false,
            auth_context: None,
            admin_principal: None,
            local_auth_rejection: None,
            model_directive_policy: Default::default(),
        }
    }

    pub(crate) fn proxy_path_and_query(&self) -> String {
        if let Some(query) = self
            .public_query_string
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            format!("{}?{}", self.public_path, query)
        } else {
            self.public_path.clone()
        }
    }

    pub(crate) fn is_execution_runtime_candidate(&self) -> bool {
        self.execution_runtime_candidate
    }

    pub(crate) fn with_execution_runtime_candidate(mut self, value: bool) -> Self {
        self.execution_runtime_candidate = value;
        self
    }
}

#[derive(Debug, Clone)]
pub(super) struct ClassifiedRoute {
    route_class: &'static str,
    route_family: &'static str,
    route_kind: &'static str,
    request_auth_channel: Option<&'static str>,
    client_surface: Option<ClientSurface>,
    api_operation: Option<ApiOperation>,
    auth_endpoint_signature: String,
    execution_runtime_candidate: bool,
}

pub(super) fn classified(
    route_class: &'static str,
    route_family: &'static str,
    route_kind: &'static str,
    auth_endpoint_signature: impl Into<String>,
    execution_runtime_candidate: bool,
) -> ClassifiedRoute {
    ClassifiedRoute {
        route_class,
        route_family,
        route_kind,
        request_auth_channel: None,
        client_surface: None,
        api_operation: None,
        auth_endpoint_signature: auth_endpoint_signature.into(),
        execution_runtime_candidate,
    }
}

pub(super) fn classified_with_request_auth_channel(
    route_class: &'static str,
    route_family: &'static str,
    route_kind: &'static str,
    request_auth_channel: &'static str,
    auth_endpoint_signature: impl Into<String>,
    execution_runtime_candidate: bool,
) -> ClassifiedRoute {
    ClassifiedRoute {
        route_class,
        route_family,
        route_kind,
        request_auth_channel: Some(request_auth_channel),
        client_surface: None,
        api_operation: None,
        auth_endpoint_signature: auth_endpoint_signature.into(),
        execution_runtime_candidate,
    }
}

impl ClassifiedRoute {
    pub(super) fn with_client_surface(mut self, client_surface: ClientSurface) -> Self {
        self.client_surface = Some(client_surface);
        self
    }

    pub(super) fn with_api_operation(mut self, api_operation: ApiOperation) -> Self {
        self.api_operation = Some(api_operation);
        self
    }
}

impl ClassifiedRoute {
    fn into_decision(self, public_path: String) -> GatewayControlDecision {
        GatewayControlDecision {
            public_path,
            public_query_string: None,
            route_class: Some(self.route_class.to_string()),
            route_family: Some(self.route_family.to_string()),
            route_kind: Some(self.route_kind.to_string()),
            client_surface: self.client_surface,
            api_operation: self.api_operation,
            gateway_credential_carrier: None,
            request_auth_channel: self.request_auth_channel.map(str::to_string),
            auth_endpoint_signature: Some(self.auth_endpoint_signature),
            execution_runtime_candidate: self.execution_runtime_candidate,
            auth_context: None,
            admin_principal: None,
            local_auth_rejection: None,
            model_directive_policy: Default::default(),
        }
    }
}

pub(crate) async fn resolve_control_route(
    state: &AppState,
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
    trace_id: &str,
) -> Result<Option<GatewayControlDecision>, GatewayError> {
    resolve_control_route_with_trusted_auth(state, method, uri, headers, trace_id, cfg!(test)).await
}

pub(crate) async fn resolve_control_route_with_trusted_auth(
    state: &AppState,
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
    trace_id: &str,
    trusted_auth_verified: bool,
) -> Result<Option<GatewayControlDecision>, GatewayError> {
    let Some(mut decision) = classify_control_route(method, uri, headers) else {
        return Ok(None);
    };
    decision.public_query_string = uri.query().map(ToOwned::to_owned);
    if decision.route_class.as_deref() == Some("ai_public") {
        decision.model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::load(state).await;
    }

    match resolve_control_decision_auth_with_trusted_auth(
        state,
        headers,
        uri,
        trace_id,
        decision,
        trusted_auth_verified,
    )
    .await?
    {
        ControlDecisionAuthResolution::Resolved(decision) => Ok(Some(decision)),
    }
}

pub(crate) fn classify_control_route(
    method: &http::Method,
    uri: &Uri,
    headers: &http::HeaderMap,
) -> Option<GatewayControlDecision> {
    let path = uri.path();
    let normalized_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };

    let public_models_auth_signature = detect_public_models_auth_signature(uri, headers);

    let classified = public_support::classify_public_support_route(
        method,
        &normalized_path,
        &public_models_auth_signature,
    )
    .or_else(|| oauth::classify_oauth_route(method, &normalized_path))
    .or_else(|| admin::classify_admin_route(method, &normalized_path))
    .or_else(|| internal::classify_internal_route(method, &normalized_path))
    .or_else(|| ai::classify_ai_public_route(method, &normalized_path, uri.query(), headers))?;

    let mut decision = classified.into_decision(normalized_path);
    if let Some(signature) = decision.auth_endpoint_signature.as_deref() {
        decision.gateway_credential_carrier =
            resolve_gateway_credential_carrier(headers, uri, signature);
    }
    if decision.route_family.as_deref() == Some("claude") {
        if let Some(carrier) = decision.gateway_credential_carrier {
            decision.request_auth_channel = Some(carrier.request_auth_channel().to_string());
        }
    }
    Some(decision)
}

pub(super) fn detect_public_models_auth_signature(uri: &Uri, headers: &http::HeaderMap) -> String {
    let has_claude_key = header_value_str(headers, "x-api-key")
        .or_else(|| header_value_str(headers, "api-key"))
        .is_some();
    let has_anthropic_version = header_value_str(headers, "anthropic-version").is_some();
    if has_claude_key && has_anthropic_version {
        return "claude:messages".to_string();
    }

    let has_gemini_key = header_value_str(headers, "x-goog-api-key").is_some()
        || uri.query().is_some_and(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .any(|(key, value)| key == "key" && !value.trim().is_empty())
        });
    if has_gemini_key {
        return "gemini:generate_content".to_string();
    }

    let has_codex_client_version = uri.path() == "/v1/models"
        && uri.query().is_some_and(|query| {
            url::form_urlencoded::parse(query.as_bytes()).any(|(key, _)| key == "client_version")
        });
    if has_codex_client_version {
        return "openai:responses".to_string();
    }

    if uri.path().starts_with("/v1beta/models") {
        return "gemini:generate_content".to_string();
    }

    "openai:chat".to_string()
}

pub(super) fn detect_claude_client_surface(headers: &http::HeaderMap) -> ClientSurface {
    let user_agent = header_value_str(headers, http::header::USER_AGENT.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let x_app_is_cli = header_value_str(headers, "x-app")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("cli"));
    if user_agent.contains("claude-code")
        || user_agent.contains("claude-cli")
        || user_agent.contains("claude code")
        || x_app_is_cli
        || header_value_str(headers, "x-claude-code-session-id").is_some()
    {
        ClientSurface::ClaudeCode
    } else if user_agent.contains("anthropic/")
        || header_value_str(headers, "x-stainless-lang").is_some()
    {
        ClientSurface::AnthropicSdk
    } else {
        ClientSurface::GenericCompatible
    }
}

pub(super) fn is_gemini_cli_request(headers: &http::HeaderMap) -> bool {
    let x_app = header_value_str(headers, "x-app")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if x_app.contains("cli") {
        return true;
    }

    let user_agent = header_value_str(headers, http::header::USER_AGENT.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    user_agent.contains("geminicli") || user_agent.contains("gemini-cli")
}

pub(super) fn is_gemini_models_route(path: &str) -> bool {
    (path.starts_with("/v1/models/") || path.starts_with("/v1beta/models/"))
        && (path.contains(":generateContent")
            || path.contains(":streamGenerateContent")
            || path.contains(":embedContent")
            || path.contains(":batchEmbedContents")
            || path.contains(":predictLongRunning"))
}

pub(super) fn is_gemini_operation_route(path: &str) -> bool {
    (path.starts_with("/v1beta/models/") && path.contains("/operations/"))
        || path == "/v1beta/operations"
        || path.starts_with("/v1beta/operations/")
}
