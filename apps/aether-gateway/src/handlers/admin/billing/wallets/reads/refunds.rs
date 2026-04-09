use super::super::shared::{
    admin_wallet_id_from_suffix_path, build_admin_wallet_not_found_response,
    build_admin_wallet_refund_payload, build_admin_wallet_summary_payload,
    build_admin_wallets_bad_request_response, parse_admin_wallets_limit,
    parse_admin_wallets_offset, resolve_admin_wallet_owner_summary,
    ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_refunds_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(wallet_id) = admin_wallet_id_from_suffix_path(request_context.path(), "/refunds")
    else {
        return Ok(build_admin_wallets_bad_request_response("wallet_id 无效"));
    };
    let limit = match parse_admin_wallets_limit(request_context.query_string()) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let offset = match parse_admin_wallets_offset(request_context.query_string()) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };

    let Some(wallet) = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::WalletId(
            &wallet_id,
        ))
        .await?
    else {
        return Ok(build_admin_wallet_not_found_response());
    };
    if wallet.api_key_id.is_some() {
        return Ok(build_admin_wallets_bad_request_response(
            ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
        ));
    }

    let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
    let wallet_payload = build_admin_wallet_summary_payload(&wallet, &owner);
    let (refunds, total) = state
        .list_admin_wallet_refunds(&wallet.id, limit, offset)
        .await?;
    let items = refunds
        .into_iter()
        .map(|refund| build_admin_wallet_refund_payload(&wallet, &owner, &refund))
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "wallet": wallet_payload,
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response())
}
