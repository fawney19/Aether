use super::mutations::{
    build_admin_wallet_adjust_response, build_admin_wallet_complete_refund_response,
    build_admin_wallet_fail_refund_response, build_admin_wallet_process_refund_response,
    build_admin_wallet_recharge_response,
};
use super::reads::{
    build_admin_wallet_detail_response, build_admin_wallet_ledger_response,
    build_admin_wallet_list_response, build_admin_wallet_refund_requests_response,
    build_admin_wallet_refunds_response, build_admin_wallet_transactions_response,
};
use super::shared::build_admin_wallets_data_unavailable_response;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, http, response::Response};

pub(super) async fn maybe_build_local_admin_wallets_routes_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("wallets_manage") {
        return Ok(None);
    }

    let path = request_context.path();
    let is_wallets_route = (request_context.method() == http::Method::GET
        && matches!(path, "/api/admin/wallets" | "/api/admin/wallets/"))
        || (request_context.method() == http::Method::GET
            && matches!(
                path,
                "/api/admin/wallets/ledger" | "/api/admin/wallets/ledger/"
            ))
        || (request_context.method() == http::Method::GET
            && matches!(
                path,
                "/api/admin/wallets/refund-requests" | "/api/admin/wallets/refund-requests/"
            ))
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/wallets/")
            && path.ends_with("/transactions"))
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/wallets/")
            && path.ends_with("/refunds"))
        || (request_context.method() == http::Method::GET
            && path.starts_with("/api/admin/wallets/")
            && !path.ends_with("/transactions")
            && !path.ends_with("/refunds")
            && path.matches('/').count() == 4)
        || (request_context.method() == http::Method::POST
            && matches!(
                decision.route_kind.as_deref(),
                Some(
                    "adjust_balance"
                        | "recharge_balance"
                        | "process_refund"
                        | "complete_refund"
                        | "fail_refund"
                )
            ));

    if !is_wallets_route {
        return Ok(None);
    }

    if decision.route_kind.as_deref() == Some("wallet_detail")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_admin_wallet_detail_response(state, request_context).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("list_wallets")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_admin_wallet_list_response(state, request_context).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("ledger")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_admin_wallet_ledger_response(state, request_context).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("list_refund_requests")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_admin_wallet_refund_requests_response(state, request_context).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("list_wallet_transactions")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_admin_wallet_transactions_response(state, request_context).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("list_wallet_refunds")
        && request_context.method() == http::Method::GET
    {
        return Ok(Some(
            build_admin_wallet_refunds_response(state, request_context).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("adjust_balance")
        && request_context.method() == http::Method::POST
    {
        return Ok(Some(
            build_admin_wallet_adjust_response(state, request_context, request_body).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("recharge_balance")
        && request_context.method() == http::Method::POST
    {
        return Ok(Some(
            build_admin_wallet_recharge_response(state, request_context, request_body).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("process_refund")
        && request_context.method() == http::Method::POST
    {
        return Ok(Some(
            build_admin_wallet_process_refund_response(state, request_context).await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("complete_refund")
        && request_context.method() == http::Method::POST
    {
        return Ok(Some(
            build_admin_wallet_complete_refund_response(state, request_context, request_body)
                .await?,
        ));
    }
    if decision.route_kind.as_deref() == Some("fail_refund")
        && request_context.method() == http::Method::POST
    {
        return Ok(Some(
            build_admin_wallet_fail_refund_response(state, request_context, request_body).await?,
        ));
    }

    Ok(Some(build_admin_wallets_data_unavailable_response()))
}
