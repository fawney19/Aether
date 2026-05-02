use super::execution::{
    estimate_admin_provider_oauth_batch_import_total,
    execute_admin_provider_oauth_batch_import_for_provider_type,
};
use super::parse::{
    build_admin_provider_oauth_batch_import_response,
    parse_admin_provider_oauth_batch_import_request, AdminProviderOAuthBatchImportRequest,
};
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    is_fixed_provider_type_for_provider_oauth,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_batch_import_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Bytes,
    http,
    response::{IntoResponse, Response},
    Json,
};

pub(in super::super) async fn handle_admin_provider_oauth_batch_import(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response, GatewayError> {
    let raw_state = state.cloned_app();
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) = admin_provider_oauth_batch_import_provider_id(request_context.path())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_batch_import_request(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if provider_type != "kiro" && admin_provider_oauth_template(&provider_type).is_none() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }

    let total = estimate_admin_provider_oauth_batch_import_total(
        &provider_type,
        payload.credentials.as_str(),
    );
    if total == 0 {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "未找到有效的 Token 数据",
        ));
    }

    let outcome = execute_admin_provider_oauth_batch_import_for_provider_type(
        &AdminAppState::new(&raw_state),
        &provider_id,
        &provider_type,
        payload.credentials.as_str(),
        payload.proxy_node_id.as_deref(),
        None,
    )
    .await?;
    Ok(build_admin_provider_oauth_batch_import_response(&outcome).into_response())
}
