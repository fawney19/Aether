use super::support::{
    admin_billing_collector_id_from_path, admin_billing_optional_bool_filter,
    admin_billing_optional_filter, admin_billing_pages, admin_billing_parse_page,
    admin_billing_parse_page_size, build_admin_billing_bad_request_response,
    build_admin_billing_collector_payload_from_record, build_admin_billing_not_found_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_list_dimension_collectors_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let page = match admin_billing_parse_page(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };
    let page_size = match admin_billing_parse_page_size(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };
    let api_format = admin_billing_optional_filter(query, "api_format");
    let task_type = admin_billing_optional_filter(query, "task_type");
    let dimension_name = admin_billing_optional_filter(query, "dimension_name");
    let is_enabled = match admin_billing_optional_bool_filter(query, "is_enabled") {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_billing_bad_request_response(detail)),
    };

    let (items, total) = state
        .list_admin_billing_collectors(
            api_format.as_deref(),
            task_type.as_deref(),
            dimension_name.as_deref(),
            is_enabled,
            page,
            page_size,
        )
        .await?
        .unwrap_or_default();

    Ok(Json(json!({
        "items": items
            .iter()
            .map(build_admin_billing_collector_payload_from_record)
            .collect::<Vec<_>>(),
        "total": total,
        "page": page,
        "page_size": page_size,
        "pages": admin_billing_pages(total, page_size),
    }))
    .into_response())
}

pub(super) async fn build_admin_get_dimension_collector_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(collector_id) = admin_billing_collector_id_from_path(request_context.path()) else {
        return Ok(build_admin_billing_bad_request_response(
            "缺少 collector_id",
        ));
    };

    match state.read_admin_billing_collector(&collector_id).await? {
        Some(record) => {
            Ok(Json(build_admin_billing_collector_payload_from_record(&record)).into_response())
        }
        None => Ok(build_admin_billing_not_found_response(
            "Dimension collector not found",
        )),
    }
}
