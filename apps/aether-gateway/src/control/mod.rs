#[cfg(test)]
use axum::http::Uri;

mod auth;
mod execute;
mod management_token_permissions;
mod public;
mod route;

pub(crate) use auth::{
    execution_plan_balance_capacity_rejection, extract_requested_model,
    refresh_execution_runtime_auth_context, request_model_local_rejection,
    resolve_execution_runtime_auth_context, should_buffer_request_for_local_auth,
    trusted_auth_local_rejection, GatewayAdminPrincipalContext, GatewayControlAuthContext,
    GatewayLocalAuthRejection,
};
pub(crate) use execute::{allows_control_execute_emergency, maybe_execute_via_control};
pub(crate) use management_token_permissions::{
    all_assignable_management_token_permissions,
    audit_admin_read_only_management_token_permissions,
    management_token_permission_catalog_payload, management_token_permission_keys_from_value,
    management_token_permission_mode_and_summary,
    management_token_permissions_cover_all_assignable_permissions,
    management_token_required_permission, normalize_assignable_management_token_permissions,
    read_only_management_token_permissions, validate_management_token_admin_route_permission,
};
pub(crate) use public::{resolve_public_request_context, GatewayPublicRequestContext};
#[cfg(test)]
pub(crate) use route::classify_control_route;
pub(crate) use route::{resolve_control_route, GatewayControlDecision};

#[cfg(test)]
mod tests;
