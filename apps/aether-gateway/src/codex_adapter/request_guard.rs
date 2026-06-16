use http::StatusCode;

use super::config::CodexAdapterRuntimeConfig;
use crate::control::{GatewayControlAuthContext, GatewayControlDecision};
use crate::{AppState, GatewayError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexAdapterRequestRejection {
    Disabled,
    RequiresResponses,
    ModelNotConfigured,
    RouteHasNoCandidates,
}

impl CodexAdapterRequestRejection {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Disabled => "codex_adapter_disabled",
            Self::RequiresResponses => "codex_adapter_requires_responses",
            Self::ModelNotConfigured => "codex_adapter_model_not_configured",
            Self::RouteHasNoCandidates => "codex_adapter_route_has_no_candidates",
        }
    }

    pub(crate) const fn status_code(self) -> StatusCode {
        match self {
            Self::RouteHasNoCandidates => StatusCode::SERVICE_UNAVAILABLE,
            Self::Disabled | Self::RequiresResponses | Self::ModelNotConfigured => {
                StatusCode::FORBIDDEN
            }
        }
    }
}

pub(crate) async fn codex_adapter_request_rejection(
    state: &AppState,
    decision: &GatewayControlDecision,
    auth_context: &GatewayControlAuthContext,
    uri: &http::Uri,
    requested_model: Option<&str>,
) -> Result<Option<CodexAdapterRequestRejection>, GatewayError> {
    if !auth_context.api_key_codex_adapter_enabled {
        return Ok(None);
    }

    if !state.system_config_bool_for_codex_adapter_enabled().await? {
        return Ok(Some(CodexAdapterRequestRejection::Disabled));
    }

    if !request_is_openai_responses(decision, uri) {
        return Ok(Some(CodexAdapterRequestRejection::RequiresResponses));
    }

    let Some(requested_model) = requested_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return Ok(Some(CodexAdapterRequestRejection::ModelNotConfigured));
    };
    let config = state.read_codex_adapter_runtime_config().await?;
    Ok(route_rejection_for_model(&config, requested_model))
}

fn request_is_openai_responses(decision: &GatewayControlDecision, uri: &http::Uri) -> bool {
    uri.path() == "/v1/responses"
        && decision.route_class.as_deref() == Some("ai_public")
        && decision.route_family.as_deref() == Some("openai")
        && decision.route_kind.as_deref() == Some("responses")
        && decision.auth_endpoint_signature.as_deref() == Some("openai:responses")
}

fn route_rejection_for_model(
    config: &CodexAdapterRuntimeConfig,
    requested_model: &str,
) -> Option<CodexAdapterRequestRejection> {
    let Some(route) = config
        .routes
        .iter()
        .find(|route| route.codex_model == requested_model)
    else {
        return Some(CodexAdapterRequestRejection::ModelNotConfigured);
    };
    if !route.enabled {
        return Some(CodexAdapterRequestRejection::ModelNotConfigured);
    }
    if !route.candidates.iter().any(|candidate| candidate.enabled) {
        return Some(CodexAdapterRequestRejection::RouteHasNoCandidates);
    }
    None
}
