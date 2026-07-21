use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::data::candidate_selection::{
    enumerate_minimal_candidate_selection_with_required_capabilities_for_request_operation,
    MinimalCandidateSelectionRowSource,
};
use crate::GatewayError;

pub(super) async fn enumerate_scheduler_candidates(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    enable_model_directives: bool,
    request_operation: Option<&str>,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    enumerate_minimal_candidate_selection_with_required_capabilities_for_request_operation(
        selection_row_source,
        api_format,
        global_model_name,
        require_streaming,
        auth_snapshot,
        required_capabilities,
        enable_model_directives,
        request_operation,
    )
    .await
    .map_err(|err| GatewayError::Internal(err.to_string()))
}
