use super::super::shared::{
    admin_wallet_id_from_suffix_path, build_admin_wallet_not_found_response,
    build_admin_wallet_summary_payload, build_admin_wallets_bad_request_response,
    parse_admin_wallets_limit, parse_admin_wallets_offset, resolve_admin_wallet_owner_summary,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_transactions_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(wallet_id) = admin_wallet_id_from_suffix_path(request_context.path(), "/transactions")
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
    let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
    let wallet_payload = build_admin_wallet_summary_payload(&wallet, &owner);

    let (transactions, total) = state
        .list_admin_wallet_transactions(&wallet.id, limit, offset)
        .await?;
    let mut items = Vec::with_capacity(transactions.len());
    for transaction in transactions {
        let (operator_name, operator_email) =
            if transaction.operator_name.is_some() || transaction.operator_email.is_some() {
                (transaction.operator_name, transaction.operator_email)
            } else {
                match transaction.operator_id.as_deref() {
                    Some(operator_id) => state
                        .find_user_auth_by_id(operator_id)
                        .await?
                        .map(|user| (Some(user.username), user.email))
                        .unwrap_or((None, None)),
                    None => (None, None),
                }
            };
        items.push(json!({
            "id": transaction.id,
            "wallet_id": transaction.wallet_id,
            "owner_type": owner.owner_type,
            "owner_name": owner.owner_name.clone(),
            "wallet_status": wallet.status.clone(),
            "category": transaction.category,
            "reason_code": transaction.reason_code,
            "amount": transaction.amount,
            "balance_before": transaction.balance_before,
            "balance_after": transaction.balance_after,
            "recharge_balance_before": transaction.recharge_balance_before,
            "recharge_balance_after": transaction.recharge_balance_after,
            "gift_balance_before": transaction.gift_balance_before,
            "gift_balance_after": transaction.gift_balance_after,
            "link_type": transaction.link_type,
            "link_id": transaction.link_id,
            "operator_id": transaction.operator_id,
            "operator_name": operator_name,
            "operator_email": operator_email,
            "description": transaction.description,
            "created_at": transaction.created_at_unix_secs.and_then(unix_secs_to_rfc3339),
        }));
    }

    Ok(Json(json!({
        "wallet": wallet_payload,
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response())
}
