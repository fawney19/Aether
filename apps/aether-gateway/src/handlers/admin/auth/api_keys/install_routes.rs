use super::shared::{
    admin_api_key_install_session_id_from_path, build_admin_api_keys_bad_request_response,
    build_admin_api_keys_data_unavailable_response, build_admin_api_keys_not_found_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::handlers::public::{
    build_api_key_install_session_response, CreateApiKeyInstallSessionRequest,
};
use crate::GatewayError;
use axum::{body::Body, http, response::Response};

pub(super) async fn build_admin_create_api_key_install_session_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_headers: &http::HeaderMap,
    remote_addr: &std::net::SocketAddr,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_data_reader() {
        return Ok(build_admin_api_keys_data_unavailable_response());
    }

    let Some(api_key_id) = admin_api_key_install_session_id_from_path(request_context.path())
    else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };
    let Some(request_body) = request_body else {
        return Ok(build_admin_api_keys_bad_request_response(
            "请求数据验证失败",
        ));
    };
    let payload = match serde_json::from_slice::<CreateApiKeyInstallSessionRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return Ok(build_admin_api_keys_bad_request_response(
                "请求数据验证失败",
            ));
        }
    };

    let Some(record) = state
        .find_auth_api_key_export_standalone_record_by_id(&api_key_id)
        .await?
    else {
        return Ok(build_admin_api_keys_not_found_response());
    };

    let response = build_api_key_install_session_response(
        state.app(),
        request_context.public(),
        request_headers,
        remote_addr,
        &record,
        payload,
    )
    .await;

    if response.status().is_success() {
        Ok(attach_admin_audit_response(
            response,
            "admin_standalone_api_key_install_session_created",
            "create_standalone_api_key_install_session",
            "api_key",
            &api_key_id,
        ))
    } else {
        Ok(response)
    }
}
