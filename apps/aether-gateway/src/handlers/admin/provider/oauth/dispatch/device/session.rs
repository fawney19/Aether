use crate::handlers::admin::provider::oauth::state::{
    default_kiro_device_region, default_kiro_device_start_url,
};
use crate::handlers::admin::shared::attach_admin_audit_response;
use axum::{body::Body, response::Response};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOAuthDeviceAuthorizePayload {
    #[serde(default = "default_kiro_device_start_url")]
    pub(super) start_url: String,
    #[serde(default = "default_kiro_device_region")]
    pub(super) region: String,
    pub(super) proxy_node_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOAuthDevicePollPayload {
    pub(super) session_id: String,
}

pub(super) fn attach_admin_provider_oauth_device_poll_terminal_response(
    session_id: &str,
    status: &str,
    response: Response<Body>,
) -> Response<Body> {
    match status {
        "authorized" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_device_authorization_completed",
            "poll_provider_oauth_device_authorization_terminal_state",
            "provider_oauth_device_session",
            session_id,
        ),
        "expired" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_device_authorization_expired",
            "poll_provider_oauth_device_authorization_terminal_state",
            "provider_oauth_device_session",
            session_id,
        ),
        "error" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_device_authorization_failed",
            "poll_provider_oauth_device_authorization_terminal_state",
            "provider_oauth_device_session",
            session_id,
        ),
        _ => response,
    }
}
