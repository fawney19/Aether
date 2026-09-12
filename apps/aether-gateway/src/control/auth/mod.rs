mod credentials;
mod gate;
mod principal;
mod resolution;
mod types;

pub(crate) use credentials::extract_requested_model;
pub(super) use credentials::resolve_gateway_credential_carrier;
pub(crate) use gate::{
    estimate_execution_plan_cost_upper_bound_usd, execution_plan_balance_capacity_rejection,
    request_model_local_rejection, should_buffer_request_for_local_auth,
    trusted_auth_local_rejection, GatewayLocalAuthRejection,
};
pub(crate) use resolution::{
    refresh_execution_runtime_auth_context, refresh_execution_runtime_auth_context_with_snapshot,
    resolve_execution_runtime_auth_context, resolve_local_admin_session_principal,
    GatewayAdminPrincipalContext, GatewayControlAuthContext,
};
pub(super) use resolution::{
    resolve_control_decision_auth_with_trusted_auth, ControlDecisionAuthResolution,
};
pub(crate) use types::GatewayCredentialCarrier;
