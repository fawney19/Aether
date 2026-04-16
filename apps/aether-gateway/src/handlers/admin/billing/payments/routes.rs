use super::{
    build_admin_payments_data_unavailable_response,
    callbacks::maybe_build_local_admin_payment_callbacks_response,
    orders::maybe_build_local_admin_payment_orders_response,
    redeem_codes::maybe_build_local_admin_redeem_codes_response,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, http, response::Response};

pub(super) async fn maybe_build_local_admin_payments_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("payments_manage") {
        return Ok(None);
    }

    let normalized_path = request_context.path().trim_end_matches('/');
    let path = if normalized_path.is_empty() {
        request_context.path()
    } else {
        normalized_path
    };
    let is_payments_route = (request_context.method() == http::Method::GET
        && path == "/api/admin/payments/orders")
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/payments/orders/")
            && path.matches('/').count() == 5)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/payments/orders/")
            && path.ends_with("/expire")
            && path.matches('/').count() == 6)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/payments/orders/")
            && path.ends_with("/credit")
            && path.matches('/').count() == 6)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/payments/orders/")
            && path.ends_with("/fail")
            && path.matches('/').count() == 6)
        || (request_context.method() == http::Method::GET
            && path == "/api/admin/payments/callbacks")
        || (request_context.method() == http::Method::GET
            && path == "/api/admin/payments/redeem-codes/batches")
        || (request_context.method() == http::Method::POST
            && path == "/api/admin/payments/redeem-codes/batches")
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/payments/redeem-codes/batches/")
            && path.matches('/').count() == 6)
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/payments/redeem-codes/batches/")
            && path.ends_with("/codes")
            && path.matches('/').count() == 7)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/payments/redeem-codes/batches/")
            && path.ends_with("/disable")
            && path.matches('/').count() == 7)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/payments/redeem-codes/batches/")
            && path.ends_with("/delete")
            && path.matches('/').count() == 7)
        || (request_context.method() == http::Method::POST
            && path.starts_with("/api/admin/payments/redeem-codes/codes/")
            && path.ends_with("/disable")
            && path.matches('/').count() == 7);

    if !is_payments_route {
        return Ok(None);
    }

    let route_kind = decision.route_kind.as_deref();
    if let Some(response) = maybe_build_local_admin_payment_orders_response(
        state,
        request_context,
        request_body,
        route_kind,
    )
    .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) =
        maybe_build_local_admin_payment_callbacks_response(state, request_context, route_kind)
            .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) = maybe_build_local_admin_redeem_codes_response(
        state,
        request_context,
        request_body,
        route_kind,
    )
    .await?
    {
        return Ok(Some(response));
    }

    Ok(Some(build_admin_payments_data_unavailable_response()))
}
