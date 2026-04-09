use super::super::shared::{
    admin_wallet_id_from_suffix_path, admin_wallet_operator_id,
    build_admin_wallet_not_found_response, build_admin_wallet_payment_order_payload,
    build_admin_wallet_summary_payload, build_admin_wallets_bad_request_response,
    build_admin_wallets_data_unavailable_response, normalize_admin_wallet_description,
    normalize_admin_wallet_payment_method, normalize_admin_wallet_positive_amount,
    resolve_admin_wallet_owner_summary, AdminWalletRechargeRequest,
    ADMIN_WALLETS_API_KEY_RECHARGE_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, unix_secs_to_rfc3339};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_recharge_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(wallet_id) = admin_wallet_id_from_suffix_path(request_context.path(), "/recharge")
    else {
        return Ok(build_admin_wallets_bad_request_response("wallet_id 无效"));
    };
    let Some(request_body) = request_body else {
        return Ok(build_admin_wallets_bad_request_response("请求体不能为空"));
    };
    let payload = match serde_json::from_slice::<AdminWalletRechargeRequest>(request_body) {
        Ok(value) => value,
        Err(_) => return Ok(build_admin_wallets_bad_request_response("请求体格式无效")),
    };
    let amount_usd = match normalize_admin_wallet_positive_amount(payload.amount_usd, "amount_usd")
    {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let payment_method = match normalize_admin_wallet_payment_method(payload.payment_method) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let description = match normalize_admin_wallet_description(payload.description) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };

    let Some(existing_wallet) = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::WalletId(
            &wallet_id,
        ))
        .await?
    else {
        return Ok(build_admin_wallet_not_found_response());
    };
    if existing_wallet.api_key_id.is_some() {
        return Ok(build_admin_wallets_bad_request_response(
            ADMIN_WALLETS_API_KEY_RECHARGE_DETAIL,
        ));
    }
    let operator_id = admin_wallet_operator_id(request_context);
    let has_postgres = state.has_postgres_pool();
    let Some((wallet, payment_order)) = state
        .admin_create_manual_wallet_recharge(
            &wallet_id,
            amount_usd,
            &payment_method,
            operator_id.as_deref(),
            description.as_deref(),
        )
        .await?
    else {
        return if has_postgres {
            Ok(build_admin_wallet_not_found_response())
        } else {
            Ok(build_admin_wallets_data_unavailable_response())
        };
    };
    let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
    let response = Json(json!({
        "wallet": build_admin_wallet_summary_payload(&wallet, &owner),
        "payment_order": build_admin_wallet_payment_order_payload(
            payment_order.id,
            payment_order.order_no,
            payment_order.amount_usd,
            payment_order.payment_method,
            payment_order.status,
            unix_secs_to_rfc3339(payment_order.created_at_unix_secs),
            payment_order
                .credited_at_unix_secs
                .and_then(unix_secs_to_rfc3339),
        ),
    }))
    .into_response();
    Ok(attach_admin_audit_response(
        response,
        "admin_wallet_manual_recharge_created",
        "create_manual_wallet_recharge",
        "wallet",
        &wallet_id,
    ))
}
