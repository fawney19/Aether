mod credentials;
mod gate;
mod principal;
mod resolution;
mod types;

pub(crate) use credentials::extract_requested_model;
pub(crate) use gate::{
    execution_plan_balance_capacity_rejection, request_model_local_rejection,
    should_buffer_request_for_local_auth, trusted_auth_local_rejection, GatewayLocalAuthRejection,
};
pub(crate) use resolution::{
    refresh_execution_runtime_auth_context, resolve_execution_runtime_auth_context,
    GatewayAdminPrincipalContext, GatewayControlAuthContext,
};
pub(super) use resolution::{resolve_control_decision_auth, ControlDecisionAuthResolution};
