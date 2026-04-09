use super::{
    admin_payment_operator_id, admin_payment_order_id_from_detail_path,
    admin_payment_order_id_from_suffix_path, build_admin_payment_order_not_found_response,
    build_admin_payment_order_payload, build_admin_payment_orders_page_response,
    build_admin_payments_backend_unavailable_response, build_admin_payments_bad_request_response,
    normalize_admin_payment_currency, normalize_admin_payment_optional_string,
    normalize_admin_payment_positive_number, parse_admin_payments_limit,
    parse_admin_payments_offset, AdminPaymentOrderCreditRequest,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, query_param_value};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_build_local_admin_payment_orders_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match route_kind {
        Some("list_orders") => Ok(Some(
            build_admin_payment_list_orders_response(state, request_context).await?,
        )),
        Some("get_order") => Ok(Some(
            build_admin_payment_get_order_response(state, request_context).await?,
        )),
        Some("expire_order") => Ok(Some(
            build_admin_payment_expire_order_response(state, request_context).await?,
        )),
        Some("credit_order") => Ok(Some(
            build_admin_payment_credit_order_response(state, request_context, request_body).await?,
        )),
        Some("fail_order") => Ok(Some(
            build_admin_payment_fail_order_response(state, request_context).await?,
        )),
        _ => Ok(None),
    }
}

async fn build_admin_payment_list_orders_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let limit = match parse_admin_payments_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let offset = match parse_admin_payments_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let status = query_param_value(query, "status");
    let payment_method = query_param_value(query, "payment_method");

    let Some((items, total)) = state
        .list_admin_payment_orders(status.as_deref(), payment_method.as_deref(), limit, offset)
        .await?
    else {
        return Ok(build_admin_payment_orders_page_response(
            Vec::new(),
            0,
            limit,
            offset,
        ));
    };

    Ok(build_admin_payment_orders_page_response(
        items
            .iter()
            .map(build_admin_payment_order_payload)
            .collect::<Vec<_>>(),
        total,
        limit,
        offset,
    ))
}

async fn build_admin_payment_get_order_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(order_id) = admin_payment_order_id_from_detail_path(request_context.path()) else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    match state.read_admin_payment_order(&order_id).await? {
        crate::AdminWalletMutationOutcome::Applied(order) => Ok(Json(json!({
            "order": build_admin_payment_order_payload(&order),
        }))
        .into_response()),
        crate::AdminWalletMutationOutcome::NotFound => {
            Ok(build_admin_payment_order_not_found_response())
        }
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Payment order read backend unavailable",
            ))
        }
    }
}

async fn build_admin_payment_expire_order_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(order_id) = admin_payment_order_id_from_suffix_path(request_context.path(), "/expire")
    else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    match state.admin_expire_payment_order(&order_id).await? {
        crate::AdminWalletMutationOutcome::Applied((order, expired)) => {
            Ok(attach_admin_audit_response(
                Json(json!({
                    "order": build_admin_payment_order_payload(&order),
                    "expired": expired,
                }))
                .into_response(),
                "admin_payment_order_expired",
                "expire_payment_order",
                "payment_order",
                &order_id,
            ))
        }
        crate::AdminWalletMutationOutcome::NotFound => {
            Ok(build_admin_payment_order_not_found_response())
        }
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Payment order write backend unavailable",
            ))
        }
    }
}

async fn build_admin_payment_credit_order_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(order_id) = admin_payment_order_id_from_suffix_path(request_context.path(), "/credit")
    else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            match serde_json::from_slice::<AdminPaymentOrderCreditRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(build_admin_payments_bad_request_response(
                        "请求数据验证失败",
                    ));
                }
            }
        }
        _ => AdminPaymentOrderCreditRequest::default(),
    };
    let gateway_order_id = match normalize_admin_payment_optional_string(
        payload.gateway_order_id,
        "gateway_order_id",
        128,
    ) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let pay_amount = match normalize_admin_payment_positive_number(payload.pay_amount, "pay_amount")
    {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let pay_currency = match normalize_admin_payment_currency(payload.pay_currency) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
    };
    let exchange_rate =
        match normalize_admin_payment_positive_number(payload.exchange_rate, "exchange_rate") {
            Ok(value) => value,
            Err(detail) => return Ok(build_admin_payments_bad_request_response(detail)),
        };
    if payload
        .gateway_response
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Ok(build_admin_payments_bad_request_response(
            "gateway_response 必须为对象",
        ));
    }
    let operator_id = admin_payment_operator_id(request_context);
    match state
        .admin_credit_payment_order(
            &order_id,
            gateway_order_id.as_deref(),
            pay_amount,
            pay_currency.as_deref(),
            exchange_rate,
            payload.gateway_response,
            operator_id.as_deref(),
        )
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied((order, credited)) => {
            Ok(attach_admin_audit_response(
                Json(json!({
                    "order": build_admin_payment_order_payload(&order),
                    "credited": credited,
                }))
                .into_response(),
                "admin_payment_order_credited",
                "credit_payment_order",
                "payment_order",
                &order_id,
            ))
        }
        crate::AdminWalletMutationOutcome::NotFound => {
            Ok(build_admin_payment_order_not_found_response())
        }
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Payment order write backend unavailable",
            ))
        }
    }
}

async fn build_admin_payment_fail_order_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(order_id) = admin_payment_order_id_from_suffix_path(request_context.path(), "/fail")
    else {
        return Ok(build_admin_payment_order_not_found_response());
    };
    match state.admin_fail_payment_order(&order_id).await? {
        crate::AdminWalletMutationOutcome::Applied(order) => Ok(attach_admin_audit_response(
            Json(json!({
                "order": build_admin_payment_order_payload(&order),
            }))
            .into_response(),
            "admin_payment_order_failed",
            "fail_payment_order",
            "payment_order",
            &order_id,
        )),
        crate::AdminWalletMutationOutcome::NotFound => {
            Ok(build_admin_payment_order_not_found_response())
        }
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_payments_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_payments_backend_unavailable_response(
                "Payment order write backend unavailable",
            ))
        }
    }
}
