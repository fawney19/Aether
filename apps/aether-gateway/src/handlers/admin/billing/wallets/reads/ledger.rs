use super::super::shared::{
    build_admin_wallets_bad_request_response, parse_admin_wallets_limit,
    parse_admin_wallets_offset, parse_admin_wallets_owner_type_filter,
    wallet_owner_summary_from_fields,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_value, unix_secs_to_rfc3339};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_ledger_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let limit = match parse_admin_wallets_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let offset = match parse_admin_wallets_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let category = query_param_value(query, "category");
    let reason_code = query_param_value(query, "reason_code");
    let owner_type = parse_admin_wallets_owner_type_filter(query);

    let (ledger, total) = state
        .list_admin_wallet_ledger(
            category.as_deref(),
            reason_code.as_deref(),
            owner_type.as_deref(),
            limit,
            offset,
        )
        .await?;
    let items = ledger
        .into_iter()
        .map(|entry| {
            let owner = wallet_owner_summary_from_fields(
                entry.wallet_user_id.as_deref(),
                entry.wallet_user_name.clone(),
                entry.wallet_api_key_id.as_deref(),
                entry.api_key_name.clone(),
            );
            json!({
                "id": entry.id,
                "wallet_id": entry.wallet_id,
                "owner_type": owner.owner_type,
                "owner_name": owner.owner_name,
                "wallet_status": entry.wallet_status,
                "category": entry.category,
                "reason_code": entry.reason_code,
                "amount": entry.amount,
                "balance_before": entry.balance_before,
                "balance_after": entry.balance_after,
                "recharge_balance_before": entry.recharge_balance_before,
                "recharge_balance_after": entry.recharge_balance_after,
                "gift_balance_before": entry.gift_balance_before,
                "gift_balance_after": entry.gift_balance_after,
                "link_type": entry.link_type,
                "link_id": entry.link_id,
                "operator_id": entry.operator_id,
                "operator_name": entry.operator_name,
                "operator_email": entry.operator_email,
                "description": entry.description,
                "created_at": entry.created_at_unix_secs.and_then(unix_secs_to_rfc3339),
            })
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response())
}
