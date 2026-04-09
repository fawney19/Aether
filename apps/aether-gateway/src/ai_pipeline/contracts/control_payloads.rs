use tracing::warn;

use crate::ai_pipeline::{
    generic_decision_missing_exact_provider_request as generic_decision_missing_exact_provider_request_impl,
    GatewayControlSyncDecisionResponse,
};

pub(crate) fn generic_decision_missing_exact_provider_request(
    payload: &GatewayControlSyncDecisionResponse,
) -> bool {
    if !generic_decision_missing_exact_provider_request_impl(payload) {
        return false;
    }

    warn!(
        decision_kind = payload.decision_kind.as_deref().unwrap_or_default(),
        provider_api_format = payload.provider_api_format.as_deref().unwrap_or_default(),
        client_api_format = payload.client_api_format.as_deref().unwrap_or_default(),
        "gateway generic decision missing exact provider request; falling back to plan"
    );
    true
}
