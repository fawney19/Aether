use super::support::{
    admin_billing_collector_id_from_path, build_admin_billing_bad_request_response,
    build_admin_billing_collector_payload_from_record, build_admin_billing_not_found_response,
    build_admin_billing_read_only_response, parse_admin_billing_collector_request,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn build_admin_create_dimension_collector_response(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let input = match parse_admin_billing_collector_request(state, request_body, None).await {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    match state.create_admin_billing_collector(&input).await? {
        crate::LocalMutationOutcome::Applied(record) => {
            Ok(Json(build_admin_billing_collector_payload_from_record(&record)).into_response())
        }
        crate::LocalMutationOutcome::Invalid(detail) => {
            Ok(build_admin_billing_bad_request_response(detail))
        }
        crate::LocalMutationOutcome::NotFound => Ok(build_admin_billing_not_found_response(
            "Dimension collector not found",
        )),
        crate::LocalMutationOutcome::Unavailable => Ok(build_admin_billing_read_only_response(
            "当前为只读模式，无法创建维度采集器",
        )),
    }
}

pub(super) async fn build_admin_update_dimension_collector_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(collector_id) = admin_billing_collector_id_from_path(request_context.path()) else {
        return Ok(build_admin_billing_bad_request_response(
            "缺少 collector_id",
        ));
    };
    let input =
        match parse_admin_billing_collector_request(state, request_body, Some(&collector_id)).await
        {
            Ok(value) => value,
            Err(response) => return Ok(response),
        };
    match state
        .update_admin_billing_collector(&collector_id, &input)
        .await?
    {
        crate::LocalMutationOutcome::Applied(record) => {
            Ok(Json(build_admin_billing_collector_payload_from_record(&record)).into_response())
        }
        crate::LocalMutationOutcome::NotFound => Ok(build_admin_billing_not_found_response(
            "Dimension collector not found",
        )),
        crate::LocalMutationOutcome::Invalid(detail) => {
            Ok(build_admin_billing_bad_request_response(detail))
        }
        crate::LocalMutationOutcome::Unavailable => Ok(build_admin_billing_read_only_response(
            "当前为只读模式，无法更新维度采集器",
        )),
    }
}
