mod credentials;
mod gate;
mod principal;
mod resolution;
mod types;

pub(crate) use credentials::extract_requested_model;
pub(super) use credentials::resolve_gateway_credential_carrier;
pub(crate) use gate::{
    execution_plan_balance_capacity_rejection, execution_plan_cost_is_proven_zero,
    request_model_local_rejection, should_buffer_request_for_local_auth,
    trusted_auth_local_rejection, GatewayLocalAuthRejection,
};
pub(crate) use resolution::{
    refresh_execution_runtime_auth_context, resolve_execution_runtime_auth_context,
    GatewayAdminPrincipalContext, GatewayControlAuthContext,
};
pub(super) use resolution::{resolve_control_decision_auth, ControlDecisionAuthResolution};
pub(crate) use types::GatewayCredentialCarrier;
