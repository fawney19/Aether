use crate::handlers::admin::shared::attach_admin_audit_response;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
};

pub(super) fn attach_admin_provider_oauth_audit_response(
    response: Response<Body>,
    event_name: &'static str,
    action: &'static str,
    target_type: &'static str,
    target_id: Option<String>,
) -> Response<Body> {
    if !response.status().is_success() {
        return response;
    }
    let Some(target_id) = target_id else {
        return response;
    };
    attach_admin_audit_response(response, event_name, action, target_type, &target_id)
}
