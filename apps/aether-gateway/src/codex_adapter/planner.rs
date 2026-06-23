use aether_scheduler_core::ClientSessionAffinity;

use super::feature_settings::codex_adapter_feature_enabled;
use super::selection::{ordered_global_models_for_route, CodexAdapterSelectionInput};
use crate::ai_serving::{ExecutionRuntimeAuthContext, GatewayAuthApiKeySnapshot};
use crate::{AppState, GatewayError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAdapterPlannerRoute {
    pub(crate) original_model: String,
    pub(crate) ordered_global_models: Vec<String>,
}

pub(crate) async fn resolve_codex_adapter_planner_route(
    state: &AppState,
    auth_context: &ExecutionRuntimeAuthContext,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    requested_model: &str,
    trace_id: &str,
    client_session_affinity: Option<&ClientSessionAffinity>,
) -> Result<Option<CodexAdapterPlannerRoute>, GatewayError> {
    if auth_snapshot.api_key_is_standalone
        || !codex_adapter_feature_enabled(auth_snapshot.api_key_feature_settings.as_ref())
    {
        return Ok(None);
    }
    if !state.system_config_bool_for_codex_adapter_enabled().await? {
        return Ok(None);
    }

    let requested_model = requested_model.trim();
    if requested_model.is_empty() {
        return Ok(None);
    }
    let config = state.read_codex_adapter_runtime_config().await?;
    let Some(route) = config
        .routes
        .iter()
        .find(|route| route.enabled && route.codex_model == requested_model)
    else {
        return Ok(None);
    };

    let ordered_global_models = ordered_global_models_for_route(
        route,
        CodexAdapterSelectionInput {
            user_id: auth_context.user_id.as_str(),
            api_key_id: auth_context.api_key_id.as_str(),
            codex_model: requested_model,
            trace_id,
            session_key: client_session_affinity
                .and_then(|affinity| affinity.session_key.as_deref())
                .map(str::trim)
                .filter(|session_key| !session_key.is_empty()),
        },
    );
    if ordered_global_models.is_empty() {
        return Ok(None);
    }

    Ok(Some(CodexAdapterPlannerRoute {
        original_model: requested_model.to_string(),
        ordered_global_models,
    }))
}
